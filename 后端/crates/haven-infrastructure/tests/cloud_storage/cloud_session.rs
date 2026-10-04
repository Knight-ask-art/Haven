//! 云盘 PDF 的应用层消费接缝：真实 `ResourceService` 能力投影 +
//! `SessionService` 固定会话与 `read_cloud` 读取。
//!
//! 复用 `support` 的真实 SQLite（完整迁移）与假 Drive；只断言既有生产路径的对外行为，
//! 不引入新 API，也不改动任何生产代码。

use std::sync::Arc;

use crate::support::*;
use haven_application::services::ports::RemoteByteRange;
use haven_application::services::{
    ComicPageBody, ComicPageProvider, ComicPageService, PreparedComicPage, PreparedSession,
    PreparedSessionSource, ResourceService, SessionService,
};
use haven_application::wire::{
    ResourceListByMediaItemRequest, ResourceSummaryDto, SessionEngineDto, SessionOpenRequest,
};
use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::contracts::{MediaItemRepository, ResourceRepository, StorageLocationRepository};
use haven_domain::entities::{Resource, ResourceLocator, StorageLocation};
use haven_domain::enums::{
    Availability, AvailabilitySource, MediaType, ResourceType, StorageProviderType, StorageStatus,
};
use haven_domain::ids::{MediaItemId, ResourceId, StorageLocationId};
use haven_infrastructure::db::repos::SqliteRepositories;

/// 非 Comic 引擎不会调用漫画 provider；显式失败避免测试悄悄依赖漫画实现。
struct NoComicPages;

impl ComicPageProvider for NoComicPages {
    fn inspect(&self, _session: &PreparedSession) -> Result<Vec<PreparedComicPage>, AppError> {
        Err(comic_pages_unavailable())
    }

    fn read_page(
        &self,
        _session: &PreparedSession,
        _page: &PreparedComicPage,
    ) -> Result<ComicPageBody, AppError> {
        Err(comic_pages_unavailable())
    }
}

fn comic_pages_unavailable() -> AppError {
    AppError::new(
        "COMIC_PAGE_UNAVAILABLE",
        ErrorKind::Unsupported,
        "测试不提供漫画页面",
        false,
    )
}

fn repositories(harness: &Harness) -> Arc<SqliteRepositories> {
    Arc::new(SqliteRepositories::new(harness.db.clone()))
}

fn session_service(harness: &Harness, repos: &Arc<SqliteRepositories>) -> SessionService {
    SessionService::new(repos.clone(), ComicPageService::new(Arc::new(NoComicPages)))
        .with_cloud_storage(harness.core.clone())
}

fn resource_service(harness: &Harness, repos: &Arc<SqliteRepositories>) -> ResourceService {
    ResourceService::new(repos.clone()).with_cloud_storage(harness.core.clone())
}

async fn summaries(
    harness: &Harness,
    repos: &Arc<SqliteRepositories>,
    media_item_id: &str,
) -> Vec<ResourceSummaryDto> {
    resource_service(harness, repos)
        .list_by_media_item(ResourceListByMediaItemRequest {
            media_item_id: media_item_id.to_owned(),
        })
        .await
        .expect("列出资源")
        .items
}

async fn prepare_reader(
    session: &SessionService,
    media_item_id: &str,
) -> Result<PreparedSession, AppError> {
    session
        .prepare(SessionOpenRequest {
            media_item_id: media_item_id.to_owned(),
            engine: SessionEngineDto::Reader,
        })
        .await
}

/// 被拒绑定的共同断言：投影失败关闭，且拿不到任何云盘会话。
async fn assert_cloud_denied(
    harness: &Harness,
    repos: &Arc<SqliteRepositories>,
    media_item_id: &str,
) {
    let items = summaries(harness, repos, media_item_id).await;
    assert_eq!(items.len(), 1);
    assert!(!items[0].is_local);
    assert!(!items[0].can_online_read, "被拒绑定不得投影在线阅读");
    let error = prepare_reader(&session_service(harness, repos), media_item_id)
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "SECURITY_POLICY_DENIED");
}

#[tokio::test]
async fn resource_listing_projects_online_read_and_the_session_pins_the_cloud_object() {
    let harness = Harness::new(vec![credential("access-token-1", "refresh-token-1")]);
    let (account, _, _, location_id, _, binding) = harness.connect_register_import().await;
    let repos = repositories(&harness);

    let items = summaries(&harness, &repos, &binding.media_item_id).await;
    assert_eq!(items.len(), 1, "导入后媒体条目应只有一条云盘资源");
    let summary = &items[0];
    assert_eq!(summary.resource_id, binding.resource_id);
    assert_eq!(summary.mime_type.as_deref(), Some("application/pdf"));
    assert_eq!(summary.storage_display_name.as_deref(), Some(LOCATION_NAME));
    assert!(summary.can_online_read, "已导入的云盘 PDF 必须投影在线阅读");
    assert!(!summary.is_local, "云盘绑定不得被投影成本地文件");
    assert!(!summary.can_download, "只读切片不提供下载能力");
    let json = serde_json::to_string(summary).expect("序列化摘要");
    for forbidden in [
        PDF_ID,
        FOLDER_ID,
        PROFILE_ACCOUNT_ID,
        "access-token",
        "refresh-token",
    ] {
        assert!(!json.contains(forbidden), "摘要泄漏 {forbidden}: {json}");
    }

    // 会话固定完整快照，而不是一个可以跟随新授权的对象 ID。
    let session = session_service(&harness, &repos);
    let prepared = prepare_reader(&session, &binding.media_item_id)
        .await
        .expect("准备云盘会话");
    assert_eq!(prepared.resource_id.to_string(), binding.resource_id);
    assert_eq!(prepared.engine, SessionEngineDto::Reader);
    assert_eq!(prepared.media_type, MediaType::Document);
    assert!(prepared.canonical_root.is_none());
    assert!(prepared.canonical_file.is_none());
    assert!(prepared.storage_location_id.is_none());
    let PreparedSessionSource::CloudObject { snapshot } = &prepared.source else {
        panic!("应为固定的云盘对象会话: {:?}", prepared.source);
    };
    assert_eq!(snapshot.object.id, binding.object_id);
    assert_eq!(snapshot.object.size_bytes, PDF_LEN as u64);
    assert_eq!(snapshot.account_id, account.id);
    assert_eq!(snapshot.account_generation, account.generation);
    assert_eq!(snapshot.folder.location_id, location_id);
    assert_eq!(snapshot.location_status, StorageStatus::Connected);

    // 读取必须经会话自身的固定快照，而不是绕过会话直接调用核心服务。
    let reads_before = harness.drive.read_calls();
    let body = session
        .read_cloud(&prepared, None)
        .await
        .expect("读取云盘 PDF");
    assert_eq!(body.mime_type, "application/pdf");
    assert_eq!(body.total_size, PDF_LEN as u64);
    assert_eq!(body.bytes, pdf_bytes());
    assert_eq!(harness.drive.read_calls(), reads_before + 1);

    let sliced = session
        .read_cloud(
            &prepared,
            Some(RemoteByteRange {
                start: 10,
                end: Some(49),
            }),
        )
        .await
        .expect("读取云盘 PDF 分片");
    assert_eq!(sliced.bytes, pdf_bytes()[10..=49].to_vec());
}

#[tokio::test]
async fn disconnect_and_reconnect_stale_a_session_pinned_to_the_old_snapshot() {
    let harness = Harness::new(vec![
        credential("access-token-1", "refresh-token-1"),
        credential("access-token-2", "refresh-token-2"),
    ]);
    let (account, _, _, _, _, binding) = harness.connect_register_import().await;
    let repos = repositories(&harness);
    let session = session_service(&harness, &repos);

    let pinned = prepare_reader(&session, &binding.media_item_id)
        .await
        .expect("准备云盘会话");
    assert!(session.read_cloud(&pinned, None).await.is_ok());

    let tombstone = harness
        .core
        .disconnect_account(&account.id, account.generation)
        .await
        .expect("断开账户");
    assert!(!tombstone.connected);
    assert_eq!(
        session
            .read_cloud(&pinned, None)
            .await
            .unwrap_err()
            .code()
            .as_str(),
        "CLOUD_BINDING_STALE"
    );
    let items = summaries(&harness, &repos, &binding.media_item_id).await;
    assert!(!items[0].can_online_read, "断开后不得再投影在线阅读");

    let refreshed = harness.reconnect(&account.id).await;
    assert_eq!(refreshed.generation, tombstone.generation + 1);
    assert_eq!(
        session
            .read_cloud(&pinned, None)
            .await
            .unwrap_err()
            .code()
            .as_str(),
        "CLOUD_BINDING_STALE",
        "重新授权换代后旧快照仍然失效"
    );

    // 重新取快照后恢复可读：旧会话不会被自动追随到新授权。
    let current = prepare_reader(&session, &binding.media_item_id)
        .await
        .expect("重新准备云盘会话");
    let PreparedSessionSource::CloudObject { snapshot } = &current.source else {
        panic!("应为固定的云盘对象会话: {:?}", current.source);
    };
    assert_eq!(snapshot.account_generation, refreshed.generation);
    assert_eq!(
        session.read_cloud(&current, None).await.unwrap().bytes,
        pdf_bytes()
    );
}

#[tokio::test]
async fn non_pdf_media_and_other_cloud_providers_never_reach_the_cloud_reader() {
    // (a) 同一绑定被非 Document 媒介消费：云盘准入要求 PDF 文档。
    let harness = Harness::new(vec![credential("access-token-1", "refresh-token-1")]);
    let (_, _, _, _, _, binding) = harness.connect_register_import().await;
    let repos = repositories(&harness);
    let media_item_id: MediaItemId = binding.media_item_id.parse().unwrap();
    let mut item = repos
        .media_item
        .get(media_item_id)
        .await
        .unwrap()
        .expect("媒体条目");
    item.media_type = MediaType::Book;
    repos.media_item.save(&item).await.unwrap();
    assert_cloud_denied(&harness, &repos, &binding.media_item_id).await;

    // (b) 非本切片的云盘位置（WebDAV）：绝不借云盘读取路径变成成功会话。
    let harness = Harness::new(vec![credential("access-token-1", "refresh-token-1")]);
    let (_, _, _, _, _, binding) = harness.connect_register_import().await;
    let repos = repositories(&harness);
    let now = UtcMillis(1);
    let webdav = StorageLocation {
        id: StorageLocationId::new(),
        provider_type: StorageProviderType::WebDav,
        display_name: "WebDAV 库".to_owned(),
        root_ref: "webdav://example.invalid/library".to_owned(),
        credential_ref: None,
        status: StorageStatus::Connected,
        created_at: now,
        updated_at: now,
    };
    repos.storage_location.save(&webdav).await.unwrap();
    let resource_id: ResourceId = binding.resource_id.parse().unwrap();
    let mut resource = repos
        .resource
        .get(resource_id)
        .await
        .unwrap()
        .expect("资源");
    resource.storage_location_id = Some(webdav.id);
    resource.locator = ResourceLocator::StorageObject {
        provider_id: webdav.id,
        object_id: binding.object_id.clone(),
        path_hint: None,
    };
    repos.resource.save(&resource).await.unwrap();
    assert_cloud_denied(&harness, &repos, &binding.media_item_id).await;
}

#[tokio::test]
async fn a_local_pdf_session_still_opens_while_cloud_consumption_is_attached() {
    let root = tempfile::tempdir().expect("临时本地库");
    std::fs::write(root.path().join("local.pdf"), pdf_bytes()).expect("写入本地 PDF");

    let harness = Harness::new(vec![credential("access-token-1", "refresh-token-1")]);
    let (_, _, _, _, _, binding) = harness.connect_register_import().await;
    let repos = repositories(&harness);
    let now = UtcMillis(1);

    let local = StorageLocation {
        id: StorageLocationId::new(),
        provider_type: StorageProviderType::Local,
        display_name: "本地库".to_owned(),
        root_ref: root.path().to_string_lossy().into_owned(),
        credential_ref: None,
        status: StorageStatus::Connected,
        created_at: now,
        updated_at: now,
    };
    repos.storage_location.save(&local).await.unwrap();
    let local_resource_id = ResourceId::new();
    repos
        .resource
        .save(&Resource {
            id: local_resource_id,
            media_item_id: binding.media_item_id.parse().unwrap(),
            resource_type: ResourceType::PublicationFile,
            source_id: None,
            storage_location_id: Some(local.id),
            locator: ResourceLocator::LocalPath {
                path: "local.pdf".to_owned(),
            },
            mime_type: Some("application/pdf".to_owned()),
            size: Some(PDF_LEN as u64),
            hash: None,
            // 离线副本优先于云端候选：会话必须仍然落到本地文件。
            availability: Availability::OfflineAvailable,
            availability_source: AvailabilitySource::User,
            modified_ms: None,
            fingerprint_first: None,
            fingerprint_last: None,
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();

    let session = session_service(&harness, &repos);
    let prepared = prepare_reader(&session, &binding.media_item_id)
        .await
        .expect("准备本地会话");
    assert!(matches!(prepared.source, PreparedSessionSource::Local));
    assert_eq!(prepared.resource_id, local_resource_id);
    let canonical = prepared.canonical_file.as_ref().expect("本地会话应有文件");
    assert!(canonical.ends_with("local.pdf"));
    // 本地会话不能被云盘读取路径消费。
    assert_eq!(
        session
            .read_cloud(&prepared, None)
            .await
            .unwrap_err()
            .code()
            .as_str(),
        "SOURCE_UNAVAILABLE"
    );

    let items = summaries(&harness, &repos, &binding.media_item_id).await;
    assert_eq!(items.len(), 2, "云盘与本地候选并存");
    let summary = items
        .iter()
        .find(|item| item.resource_id == local_resource_id.to_string())
        .expect("本地资源摘要");
    assert!(summary.is_local);
    assert!(summary.is_offline);
    assert!(summary.can_online_read);
}
