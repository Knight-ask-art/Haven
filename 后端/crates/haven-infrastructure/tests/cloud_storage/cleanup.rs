//! 前台维护尽力而为、显式清理仍可失败可重试的回归。
//!
//! 真实 SQLite（内存库 + 完整迁移）与真实仓储；keystore 用可控失败的假实现，
//! 不触网、不睡眠。断言只落在「DB 变更事实」与「outbox 是否仍可重试」上。

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use haven_application::services::cloud_storage::browse::CloudBrowseService;
use haven_application::services::cloud_storage::ports::credential_ref;
use haven_application::services::cloud_storage::service::CloudStorageService;
use haven_application::services::cloud_storage::state::{
    CLOUD_CREDENTIAL_CLEANUP_BATCH, CloudStorageRepository,
};
use haven_application::services::cloud_storage::views::{
    CloudConnectBeginRequest, CloudConnectStatus,
};
use haven_common::{AppError, ErrorKind};
use haven_domain::credential::{CredentialStore, SecretString};
use haven_domain::ids::CredentialRef;

use crate::support::{Harness, MemoryStore, credential};

/// 可控失败的 keystore：委托原内存凭据存储，只对指定引用注入 delete 失败。
struct FlakyStore {
    inner: Arc<MemoryStore>,
    failing_deletes: Mutex<HashSet<String>>,
}

impl FlakyStore {
    fn new(inner: Arc<MemoryStore>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            failing_deletes: Mutex::new(HashSet::new()),
        })
    }

    fn fail_delete_for(&self, credential_ref: &str) {
        self.failing_deletes
            .lock()
            .unwrap()
            .insert(credential_ref.to_owned());
    }

    fn stop_failing(&self) {
        self.failing_deletes.lock().unwrap().clear();
    }
}

fn store_delete_failed() -> AppError {
    AppError::new(
        "FAKE_STORE_DELETE_FAILED",
        ErrorKind::Network,
        "模拟密钥库删除失败",
        true,
    )
}

#[async_trait]
impl CredentialStore for FlakyStore {
    async fn set(&self, target: &CredentialRef, secret: &SecretString) -> Result<(), AppError> {
        self.inner.set(target, secret).await
    }

    async fn get(&self, target: &CredentialRef) -> Result<Option<SecretString>, AppError> {
        self.inner.get(target).await
    }

    async fn delete(&self, target: &CredentialRef) -> Result<bool, AppError> {
        if self
            .failing_deletes
            .lock()
            .unwrap()
            .contains(target.as_str())
        {
            return Err(store_delete_failed());
        }
        self.inner.delete(target).await
    }
}

/// 用真实仓储 / 假 Drive / 假 OAuth + 可控失败 keystore 组装核心服务。
fn service_with(harness: &Harness, store: Arc<FlakyStore>) -> Arc<CloudStorageService> {
    Arc::new(CloudStorageService::new(
        harness.repo.clone(),
        harness.drive.clone(),
        harness.auth.clone(),
        store,
    ))
}

async fn stored_credential_ref(harness: &Harness, account_id: &str) -> String {
    harness
        .repo
        .get_account(account_id)
        .await
        .unwrap()
        .unwrap()
        .credential_ref
        .expect("账户持有凭据引用")
        .as_str()
        .to_owned()
}

#[tokio::test]
async fn disconnect_commits_the_tombstone_even_when_cleanup_fails_and_keeps_the_outbox() {
    let harness = Harness::new(vec![credential("access-token-1", "refresh-token-1")]);
    let (account, _, _, _, _, _) = harness.connect_register_import().await;
    let original_ref = stored_credential_ref(&harness, &account.id).await;

    let store = FlakyStore::new(harness.store.clone());
    store.fail_delete_for(&original_ref);
    let core = service_with(&harness, store.clone());

    // 墓碑已在 DB 提交：即使 keystore 删除失败，断开也必须如实返回成功。
    let tombstone = core
        .disconnect_account(&account.id, account.generation)
        .await
        .expect("断开成功只由 DB 变更事实决定");
    assert!(!tombstone.connected);
    assert_eq!(tombstone.generation, account.generation + 1);

    // 失败的清理行仍是 ready，保留可重试的 outbox 引用。
    let pending = harness
        .repo
        .pending_credentials(CLOUD_CREDENTIAL_CLEANUP_BATCH)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].as_str(), original_ref);

    // 显式清理仍可失败：失败如实返回，不伪造成功。
    let error = core.sweep_credential_cleanup().await.unwrap_err();
    assert_eq!(error.code().as_str(), "FAKE_STORE_DELETE_FAILED");

    // 故障解除后可重试成功，引用被清理。
    store.stop_failing();
    assert_eq!(core.sweep_credential_cleanup().await.unwrap(), 1);
    assert!(
        harness
            .repo
            .pending_credentials(CLOUD_CREDENTIAL_CLEANUP_BATCH)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn begin_connect_and_refresh_proceed_while_cleanup_keeps_failing() {
    let harness = Harness::new(vec![
        credential("access-token-1", "refresh-token-1"),
        credential("access-token-2", "refresh-token-2"),
    ]);
    let account = harness.connect_new().await;
    let original_ref = stored_credential_ref(&harness, &account.id).await;

    let store = FlakyStore::new(harness.store.clone());
    store.fail_delete_for(&original_ref);
    let core = service_with(&harness, store.clone());

    // 先制造一条删除失败、但保留可重试的 ready 引用。
    let tombstone = core
        .disconnect_account(&account.id, account.generation)
        .await
        .expect("断开成功");
    assert!(!tombstone.connected);

    // 容量维护失败绝不能阻断授权发起与完成。
    let attempt = core
        .begin_connect(CloudConnectBeginRequest {
            account_id: Some(account.id.clone()),
        })
        .await
        .expect("清理失败不阻断 begin_connect");
    assert_eq!(
        core.poll_connect(&attempt.attempt_id).await.unwrap().status,
        CloudConnectStatus::Authorized
    );
    let reconnected = core
        .complete_connect(&attempt.attempt_id)
        .await
        .expect("清理失败不阻断 complete_connect");
    assert!(reconnected.connected);

    // 重启（新服务、空缓存）：清理仍失败，但刷新必须成功并交出可用凭据。
    let restarted = service_with(&harness, store.clone());
    let browse = CloudBrowseService::new(restarted);
    let page = browse
        .browse_root(&reconnected.id)
        .await
        .expect("清理失败不阻断凭据刷新");
    assert!(page.folder_handle.is_some());
    assert_eq!(harness.auth.refresh_calls(), 1);

    // 失败的引用始终留在 outbox，等待显式 / 后台重试。
    let pending = harness
        .repo
        .pending_credentials(CLOUD_CREDENTIAL_CLEANUP_BATCH)
        .await
        .unwrap();
    assert!(
        pending.iter().any(|item| item.as_str() == original_ref),
        "删除失败的引用必须保留可重试"
    );
}

#[tokio::test]
async fn explicit_sweep_continues_past_one_failing_reference() {
    let harness = Harness::new(Vec::new());
    let failing = credential_ref(&uuid::Uuid::new_v4().to_string()).unwrap();
    let healthy = credential_ref(&uuid::Uuid::new_v4().to_string()).unwrap();
    for reference in [&failing, &healthy] {
        harness.repo.stage_credential(reference).await.unwrap();
        harness
            .repo
            .retire_staged_credential(reference)
            .await
            .unwrap();
    }

    let store = FlakyStore::new(harness.store.clone());
    store.fail_delete_for(failing.as_str());
    let core = service_with(&harness, store);

    // 单条失败如实返回错误，但不中断同批其余条目的清理。
    let error = core.sweep_credential_cleanup().await.unwrap_err();
    assert_eq!(error.code().as_str(), "FAKE_STORE_DELETE_FAILED");

    let pending = harness
        .repo
        .pending_credentials(CLOUD_CREDENTIAL_CLEANUP_BATCH)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].as_str(), failing.as_str());
}
