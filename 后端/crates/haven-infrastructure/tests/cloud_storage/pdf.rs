//! 对象快照与 PDF 只读分片的精确边界。

use crate::support::*;
use haven_application::services::ports::{RemoteByteRange, RemoteContentRange};
use haven_domain::enums::StorageStatus;

#[tokio::test]
async fn snapshot_and_exact_range_read_return_provider_bytes() {
    let harness = Harness::new(vec![credential("access-token-1", "refresh-token-1")]);
    let (account, _, _, location_id, _, binding) = harness.connect_register_import().await;

    let snapshot = harness
        .core
        .object_snapshot(&binding.object_id)
        .await
        .unwrap();
    assert_eq!(snapshot.object.id, binding.object_id);
    assert_eq!(snapshot.account_id, account.id);
    assert_eq!(snapshot.account_generation, account.generation);
    assert_eq!(snapshot.folder.location_id, location_id);
    assert_eq!(snapshot.location_status, StorageStatus::Connected);

    let full = harness
        .core
        .read_pdf(&binding.object_id, &snapshot, None)
        .await
        .unwrap();
    assert_eq!(full.mime_type, "application/pdf");
    assert_eq!(full.total_size, PDF_LEN as u64);
    assert_eq!(full.bytes, pdf_bytes());
    assert!(full.content_range.is_none());
    assert!(full.accept_ranges);

    let requested = RemoteByteRange {
        start: 10,
        end: Some(49),
    };
    let sliced = harness
        .core
        .read_pdf(&binding.object_id, &snapshot, Some(requested))
        .await
        .unwrap();
    assert_eq!(sliced.bytes, pdf_bytes()[10..=49].to_vec());
    assert_eq!(sliced.total_size, PDF_LEN as u64);
    assert_eq!(
        sliced.content_range,
        Some(RemoteContentRange {
            start: 10,
            end: 49,
            total: PDF_LEN as u64
        })
    );

    // 越界范围在触网前被拒绝。
    let calls = harness.drive.read_calls();
    let invalid = harness
        .core
        .read_pdf(
            &binding.object_id,
            &snapshot,
            Some(RemoteByteRange {
                start: PDF_LEN as u64,
                end: None,
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(invalid.code().as_str(), "CLOUD_PDF_RANGE_INVALID");
    assert_eq!(
        harness.drive.read_calls(),
        calls,
        "非法范围不得触发 Provider 读取"
    );
}

#[tokio::test]
async fn provider_read_error_is_never_turned_into_bytes() {
    let harness = Harness::new(vec![credential("access-token-1", "refresh-token-1")]);
    let (_, _, _, _, _, binding) = harness.connect_register_import().await;
    let snapshot = harness
        .core
        .object_snapshot(&binding.object_id)
        .await
        .unwrap();

    harness.drive.fail_read(PDF_ID);
    let error = harness
        .core
        .read_pdf(&binding.object_id, &snapshot, None)
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "FAKE_DRIVE_ERROR");
    assert_eq!(harness.drive.read_calls(), 1, "确实调用了 Provider 读取");

    harness.drive.clear_failures();
    assert!(
        harness
            .core
            .read_pdf(&binding.object_id, &snapshot, None)
            .await
            .is_ok()
    );
}

/// 同 ID 远端变大：旧读取报明确错误，显式重导更新同一内容链，新快照可读，旧快照仍失效。
#[tokio::test]
async fn same_id_remote_growth_requires_explicit_reimport_and_reopen() {
    let harness = Harness::new(vec![credential("access-token-1", "refresh-token-1")]);
    let (_, _, _, location_id, page, binding) = harness.connect_register_import().await;
    let object_id = binding.object_id.clone();
    let old_snapshot = harness.core.object_snapshot(&object_id).await.unwrap();
    assert_eq!(old_snapshot.object.size_bytes, PDF_LEN as u64);
    let handle = find_entry(&page, PDF_NAME)
        .handle
        .clone()
        .expect("PDF 句柄");

    let grown = PDF_LEN * 4;
    harness.drive.replace_pdf_len(grown);
    let reads_before = harness.drive.read_calls();
    let changed = harness
        .core
        .read_pdf(&object_id, &old_snapshot, None)
        .await
        .unwrap_err();
    assert_eq!(changed.code().as_str(), "CLOUD_OBJECT_CHANGED");
    assert_eq!(
        harness.drive.read_calls(),
        reads_before,
        "大小变更须在读取字节前拒绝"
    );

    let refreshed = harness
        .browse
        .import_pdf(location_id, &handle)
        .await
        .expect("重新导入");
    assert_eq!(refreshed.object_id, binding.object_id);
    assert_eq!(refreshed.resource_id, binding.resource_id);
    assert_eq!(refreshed.media_item_id, binding.media_item_id);
    assert_eq!(refreshed.size_bytes, grown as u64);

    let new_snapshot = harness.core.object_snapshot(&object_id).await.unwrap();
    let body = harness
        .core
        .read_pdf(&object_id, &new_snapshot, None)
        .await
        .expect("重导后可读");
    assert_eq!(body.total_size, grown as u64);
    assert_eq!(body.bytes.len(), grown);
    assert!(body.bytes.starts_with(b"%PDF-"));

    let stale = harness
        .core
        .read_pdf(&object_id, &old_snapshot, None)
        .await
        .unwrap_err();
    assert_eq!(
        stale.code().as_str(),
        "CLOUD_BINDING_STALE",
        "旧会话不能追随新绑定"
    );
}
