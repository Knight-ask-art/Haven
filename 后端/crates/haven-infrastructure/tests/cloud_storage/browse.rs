//! 浏览 / 选择 / 登记 / 导入的跨层链路与选择句柄拒绝语义。

use crate::support::*;
use haven_application::services::cloud_storage::ports::CLOUD_DRIVE_ROOT_ID;
use haven_domain::contracts::{
    EditionRepository, MediaItemRepository, ResourceRepository, StorageLocationRepository,
    WorkRepository,
};
use haven_domain::entities::ResourceLocator;
use haven_domain::enums::{
    Availability, AvailabilitySource, MediaType, ResourceType, StorageProviderType, StorageStatus,
};
use haven_domain::ids::StorageLocationId;
use haven_infrastructure::db::repos::{
    SqliteEditionRepository, SqliteMediaItemRepository, SqliteResourceRepository,
    SqliteStorageLocationRepository, SqliteWorkRepository,
};

#[tokio::test]
async fn connect_choose_register_browse_import_builds_the_real_resource_chain() {
    let harness = Harness::new(vec![credential("access-token-1", "refresh-token-1")]);
    let account = harness.connect_new().await;
    assert!(account.connected);
    assert_eq!(account.generation, 1);
    assert_eq!(
        harness.auth.issued_scopes(),
        vec![READONLY_SCOPE.to_owned()]
    );

    // 选择：浏览根目录拿到不透明目录句柄，再列举该目录。
    let root = harness.browse.browse_root(&account.id).await.unwrap();
    let folder_handle = find_entry(&root, FOLDER_NAME)
        .handle
        .clone()
        .expect("目录句柄");
    let chosen = harness.browse.browse_folder(&folder_handle).await.unwrap();
    assert_eq!(chosen.entries.len(), 4);
    // 未登记目录里的 PDF 如实展示，但不得投影可导入能力或签发文件句柄。
    assert!(!find_entry(&chosen, PDF_NAME).pdf_supported);
    assert!(find_entry(&chosen, PDF_NAME).handle.is_none());

    // 登记目录 → 浏览已登记目录 → 导入 PDF。
    let location_id = harness
        .browse
        .register_folder(&folder_handle, LOCATION_NAME)
        .await
        .unwrap();
    let page = harness.browse.browse_location(location_id).await.unwrap();
    assert!(find_entry(&page, PDF_NAME).pdf_supported);
    assert!(find_entry(&page, PDF_NAME).handle.is_some());
    let pdf_handle = find_entry(&page, PDF_NAME)
        .handle
        .clone()
        .expect("PDF 句柄");
    let binding = harness
        .browse
        .import_pdf(location_id, &pdf_handle)
        .await
        .unwrap();
    assert_eq!(binding.size_bytes, PDF_LEN as u64);
    assert_eq!(binding.object_id.len(), 36);
    let result_text = format!("{binding:?}");
    for denied in [
        PDF_ID,
        FOLDER_ID,
        PROFILE_ACCOUNT_ID,
        "access-token",
        "refresh-token",
        "haven:google_drive",
    ] {
        assert!(!result_text.contains(denied));
    }

    // 不可选条目如实展示但没有句柄。
    for name in [ZIP_NAME, TRASH_NAME, UNKNOWN_NAME] {
        let entry = find_entry(&page, name);
        assert!(!entry.pdf_supported, "{name} 不投影 PDF 能力");
        assert!(entry.handle.is_none(), "{name} 不可选");
    }

    // 真实内容链：Resource → MediaItem → Edition → Work，locator 只含内部 ID。
    let db = harness.db.clone();
    let resources = SqliteResourceRepository::new(db.clone());
    let items = SqliteMediaItemRepository::new(db.clone());
    let editions = SqliteEditionRepository::new(db.clone());
    let works = SqliteWorkRepository::new(db.clone());
    let locations = SqliteStorageLocationRepository::new(db);

    let resource = resources
        .get(binding.resource_id.parse().unwrap())
        .await
        .unwrap()
        .expect("资源存在");
    assert_eq!(resource.resource_type, ResourceType::PublicationFile);
    assert_eq!(resource.mime_type.as_deref(), Some("application/pdf"));
    assert_eq!(resource.storage_location_id, Some(location_id));
    assert_eq!(resource.availability, Availability::Available);
    assert_eq!(resource.availability_source, AvailabilitySource::Storage);
    match &resource.locator {
        ResourceLocator::StorageObject {
            provider_id,
            object_id,
            path_hint,
        } => {
            assert_eq!(*provider_id, location_id);
            assert_eq!(object_id, &binding.object_id, "locator 只含内部对象 UUID");
            assert_ne!(object_id, PDF_ID);
            assert!(path_hint.is_none());
        }
        other => panic!("应为 StorageObject: {other:?}"),
    }

    let media = items
        .get(binding.media_item_id.parse().unwrap())
        .await
        .unwrap()
        .expect("条目");
    assert_eq!(media.media_type, MediaType::Document);
    let edition = editions.get(media.edition_id).await.unwrap().expect("版本");
    assert_eq!(edition.title, PDF_NAME);
    assert!(
        works.get(edition.work_id).await.unwrap().is_some(),
        "真实内容链"
    );

    let location = locations.get(location_id).await.unwrap().expect("位置");
    assert_eq!(location.provider_type, StorageProviderType::GoogleDrive);
    assert_eq!(location.status, StorageStatus::Connected);
    assert_eq!(location.display_name, LOCATION_NAME);
    assert!(location.credential_ref.is_none(), "位置行不携带凭据");

    // 面向前端的投影不得含 Provider ID / 账户身份 / 令牌。
    for page in [&root, &chosen, &page] {
        let json = serde_json::to_string(page).unwrap();
        for forbidden in [
            CLOUD_DRIVE_ROOT_ID,
            FOLDER_ID,
            PDF_ID,
            ZIP_ID,
            TRASH_ID,
            UNKNOWN_ID,
            PROFILE_ACCOUNT_ID,
            "access-token",
            "refresh-token",
            "haven:google_drive",
        ] {
            assert!(
                !json.contains(forbidden),
                "页面 JSON 泄漏 {forbidden}: {json}"
            );
        }
    }
}

#[tokio::test]
async fn handles_reject_invalid_wrong_type_and_unregistered_selections() {
    let harness = Harness::new(vec![credential("access-token-1", "refresh-token-1")]);
    let (_, _, folder_handle, location_id, page, _binding) =
        harness.connect_register_import().await;

    // browse_folder 只接受目录句柄，import_pdf 只接受文件句柄。
    let invalid = harness
        .browse
        .browse_folder("not-a-handle")
        .await
        .unwrap_err();
    assert_eq!(invalid.code().as_str(), "CLOUD_BROWSE_HANDLE_INVALID");
    let wrong_type = harness
        .browse
        .import_pdf(location_id, &folder_handle)
        .await
        .unwrap_err();
    assert_eq!(wrong_type.code().as_str(), "CLOUD_BROWSE_HANDLE_INVALID");
    let unknown = harness
        .browse
        .import_pdf(location_id, &uuid::Uuid::new_v4().to_string())
        .await
        .unwrap_err();
    assert_eq!(unknown.code().as_str(), "CLOUD_BROWSE_HANDLE_INVALID");

    // 未登记位置不能借道 Provider ID 变成成功导入。
    let unregistered = StorageLocationId::new();
    assert!(
        harness
            .browse
            .import_pdf(unregistered, PDF_ID)
            .await
            .is_err(),
        "未登记位置必须拒绝"
    );

    // 已登记目录里三类不可选文件也都不签发句柄。
    for name in [ZIP_NAME, TRASH_NAME, UNKNOWN_NAME] {
        assert!(find_entry(&page, name).handle.is_none(), "{name} 不可选");
    }
}

#[tokio::test]
async fn provider_stat_error_surfaces_instead_of_a_fake_page() {
    let harness = Harness::new(vec![credential("access-token-1", "refresh-token-1")]);
    let (_, _, _, location_id, _, _) = harness.connect_register_import().await;

    harness.drive.fail_stat(FOLDER_ID);
    let error = harness
        .browse
        .browse_location(location_id)
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "FAKE_DRIVE_ERROR");

    harness.drive.clear_failures();
    assert!(harness.browse.browse_location(location_id).await.is_ok());
}
