//! 账户生命周期：断开、重新授权与进程重启后的凭据刷新。

use crate::support::*;
use haven_application::services::cloud_storage::state::{
    CLOUD_CREDENTIAL_CLEANUP_BATCH, CloudStorageRepository,
};

#[tokio::test]
async fn disconnect_clears_secret_and_outbox_but_keeps_remote_files() {
    let harness = Harness::new(vec![credential("access-token-1", "refresh-token-1")]);
    let (account, _, _, _, _, binding) = harness.connect_register_import().await;
    let snapshot = harness
        .core
        .object_snapshot(&binding.object_id)
        .await
        .unwrap();
    assert!(
        !harness.store.is_empty(),
        "连接后 keystore 有 refresh token"
    );
    let remote_before = harness.drive.remote_file_ids();

    let tombstone = harness
        .core
        .disconnect_account(&account.id, account.generation)
        .await
        .unwrap();
    assert!(!tombstone.connected);
    assert_eq!(tombstone.generation, account.generation + 1);

    // 固定快照的会话被拒绝；断开后的账户也不能再浏览。
    let stale = harness
        .core
        .read_pdf(&binding.object_id, &snapshot, None)
        .await
        .unwrap_err();
    assert_eq!(stale.code().as_str(), "CLOUD_BINDING_STALE");
    let browse = harness.browse.browse_root(&account.id).await.unwrap_err();
    assert_eq!(browse.code().as_str(), "CLOUD_BINDING_STALE");

    // 秘密与 outbox 清空；账户行不再持有凭据引用。
    assert!(harness.store.is_empty());
    let pending = harness
        .repo
        .pending_credentials(CLOUD_CREDENTIAL_CLEANUP_BATCH)
        .await
        .unwrap();
    assert!(pending.is_empty(), "断开后 outbox 必须清空");
    let row = harness
        .repo
        .get_account(&account.id)
        .await
        .unwrap()
        .unwrap();
    assert!(row.credential_ref.is_none());

    // 远端文件原样保留，且断开未触发任何远端读取（只读切片不删远端）。
    assert_eq!(harness.drive.remote_file_ids(), remote_before);
    assert_eq!(harness.drive.read_calls(), 0);
}

#[tokio::test]
async fn reconnect_bumps_generation_and_stales_the_pinned_snapshot() {
    let harness = Harness::new(vec![
        credential("access-token-1", "refresh-token-1"),
        credential("access-token-2", "refresh-token-2"),
    ]);
    let (account, _, _, _, _, binding) = harness.connect_register_import().await;
    let pinned = harness
        .core
        .object_snapshot(&binding.object_id)
        .await
        .unwrap();
    assert_eq!(pinned.account_generation, 1);

    let refreshed = harness.reconnect(&account.id).await;
    assert!(refreshed.connected);
    assert_eq!(refreshed.generation, pinned.account_generation + 1);

    let stale = harness
        .core
        .read_pdf(&binding.object_id, &pinned, None)
        .await
        .unwrap_err();
    assert_eq!(stale.code().as_str(), "CLOUD_BINDING_STALE");

    let current = harness
        .core
        .object_snapshot(&binding.object_id)
        .await
        .unwrap();
    assert_eq!(current.account_generation, refreshed.generation);
    let body = harness
        .core
        .read_pdf(&binding.object_id, &current, None)
        .await
        .unwrap();
    assert_eq!(body.total_size, PDF_LEN as u64);
}

#[tokio::test]
async fn restart_refreshes_from_the_stored_refresh_token_exactly_once() {
    let harness = Harness::new(vec![credential("access-token-1", "refresh-token-1")]);
    let (_, _, _, _, _, binding) = harness.connect_register_import().await;
    let snapshot = harness
        .core
        .object_snapshot(&binding.object_id)
        .await
        .unwrap();
    assert_eq!(harness.auth.refresh_calls(), 0, "首次连接不经过 refresh");

    // 重启：新服务、新内存缓存，只有 keystore 里的 refresh token 可用。
    let restarted = harness.restart();
    let body = restarted
        .read_pdf(&binding.object_id, &snapshot, None)
        .await
        .unwrap();
    assert_eq!(body.bytes, pdf_bytes());
    assert_eq!(harness.auth.refresh_calls(), 1);
    assert_eq!(
        harness.auth.last_refresh_token().as_deref(),
        Some("refresh-token-1")
    );
    for value in harness.store.values() {
        assert!(
            value.starts_with("refresh-token"),
            "keystore 只保存 refresh token: {value}"
        );
    }

    // 命中缓存后不再重复刷新。
    assert!(
        restarted
            .read_pdf(&binding.object_id, &snapshot, None)
            .await
            .is_ok()
    );
    assert_eq!(harness.auth.refresh_calls(), 1);
}
