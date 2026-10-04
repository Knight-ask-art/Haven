//! 云盘存储 Repository 的离线 SQLite 集成测试：真实临时库 / 真实事务，不触网、不访问 keystore。

use std::sync::{Arc, Barrier};

use haven_application::services::cloud_storage::ports::credential_ref;
use haven_application::services::cloud_storage::state::{
    CLOUD_CREDENTIAL_CLEANUP_BATCH, CLOUD_CREDENTIAL_STAGE_TTL_MS,
};
use haven_domain::contracts::{
    EditionRepository, MediaItemRepository, ResourceRepository, StorageLocationRepository,
    WorkRepository,
};
use haven_domain::entities::{Resource, ResourceLocator, StorageLocation};
use haven_domain::enums::{
    Availability, AvailabilitySource, MediaType, ResourceType, StorageProviderType, StorageStatus,
};
use haven_domain::ids::{MediaItemId, ResourceId};

use super::*;
use crate::db::repos::{
    SqliteEditionRepository, SqliteMediaItemRepository, SqliteResourceRepository,
    SqliteStorageLocationRepository, SqliteWorkRepository,
};

fn fresh_cred() -> CredentialRef {
    credential_ref(&uuid::Uuid::new_v4().to_string()).expect("合法 CredentialRef")
}

fn connect_request(provider_account_id: &str) -> CloudConnectRequest {
    CloudConnectRequest {
        provider: "google_drive".into(),
        provider_account_id: provider_account_id.into(),
        display_name: "Google Drive".into(),
    }
}

fn memory_db() -> Arc<Db> {
    Arc::new(Db::open_in_memory().unwrap())
}

async fn connect_new(
    repo: &SqliteCloudStorageRepository,
    provider_account_id: &str,
) -> (CloudAccount, CredentialRef) {
    let cred = fresh_cred();
    repo.stage_credential(&cred).await.unwrap();
    match repo
        .connect(&connect_request(provider_account_id), None, &cred)
        .await
        .unwrap()
    {
        CloudCasOutcome::Applied(account) => (account, cred),
        other => panic!("connect 应成功: {other:?}"),
    }
}

async fn register_folder(
    repo: &SqliteCloudStorageRepository,
    account: &CloudAccount,
    provider_folder_id: &str,
    name: &str,
) -> CloudFolderBinding {
    let request = CloudFolderRequest {
        account_id: account.id.clone(),
        provider_folder_id: provider_folder_id.into(),
    };
    match repo
        .register_folder(&request, account.generation, name)
        .await
        .unwrap()
    {
        CloudCasOutcome::Applied(binding) => binding,
        other => panic!("register_folder 应成功: {other:?}"),
    }
}

async fn import_new(
    repo: &SqliteCloudStorageRepository,
    folder: &CloudFolderBinding,
    generation: i64,
    file: &CloudPdfCandidate,
) -> CloudObjectBinding {
    match repo.import_pdf(folder, generation, file).await.unwrap() {
        CloudCasOutcome::Applied(CloudPdfImportOutcome::Created(binding)) => binding,
        other => panic!("import_pdf 应新建: {other:?}"),
    }
}

fn pdf(file_id: &str, size_bytes: u64) -> CloudPdfCandidate {
    CloudPdfCandidate {
        provider_file_id: file_id.into(),
        display_name: format!("{file_id}.pdf"),
        size_bytes,
    }
}

/// 本地文件资源样本；云盘场景可在返回后改写 locator / availability。
fn sample_resource(media_item_id: MediaItemId, storage_location_id: StorageLocationId) -> Resource {
    Resource {
        id: ResourceId::new(),
        media_item_id,
        resource_type: ResourceType::LocalFile,
        source_id: None,
        storage_location_id: Some(storage_location_id),
        locator: ResourceLocator::LocalPath {
            path: "D:\\local\\v.mkv".into(),
        },
        mime_type: None,
        size: None,
        hash: None,
        availability: Availability::Available,
        availability_source: AvailabilitySource::Unknown,
        modified_ms: None,
        fingerprint_first: None,
        fingerprint_last: None,
        created_at: UtcMillis::now(),
        updated_at: UtcMillis::now(),
    }
}

#[path = "tests/concurrency.rs"]
mod concurrency;
#[path = "tests/content.rs"]
mod content;
#[path = "tests/lifecycle.rs"]
mod lifecycle;
