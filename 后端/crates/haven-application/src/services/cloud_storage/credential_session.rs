//! 云盘凭据会话：只持久化 refresh token、按账户单飞加载、代际化缓存与刷新轮换。
//!
//! 不变量：
//! - 缓存键只会是**已从仓储读到并逐字段校验过**的账户内部 UUID；调用方传入的任意字符串
//!   在进入任何 map 之前先做规范化 UUID 校验，因此不存在可被外部撑大的表。
//! - 命中缓存也必须先重读账户行：`connected`、代际与**当前 credential_ref** 三者同时与
//!   缓存一致才返回缓存里的 `Arc<CloudDriveCredential>`；只比代际会放过「同代际换了凭据」。
//! - 同一账户的加载单飞，同时进行的 flight 有硬上限，超限显式失败而不是排队。
//! - 同步 `Mutex` 只覆盖极短读改写，绝不跨 `.await` 持有；跨等待的点用 `tokio::sync::Mutex`。
//! - 轮换写入遵守 ADR-001 顺序：stage → `store.set` → CAS；任何 Err / Stale / Missing 都必须
//!   补偿（删除新秘密 + 把 staged 转 ready），因此迟到写入的清理始终可重试。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use haven_common::{AppError, ErrorKind, UtcMillis, validation};
use haven_domain::credential::{CredentialStore, SecretString};
use haven_domain::ids::CredentialRef;

use super::ports::{
    CLOUD_DRIVE_PROVIDER_ID, CloudDriveAuthPort, CloudDriveCredential, credential_ref,
};
use super::state::{
    CLOUD_CREDENTIAL_CLEANUP_BATCH, CloudAccount, CloudCasOutcome, CloudStorageRepository,
};

/// 固定只读 scope：必须与 Infrastructure 握手实现的常量完全一致，调用方不得自定义。
pub(crate) const CLOUD_DRIVE_READONLY_SCOPE: &str =
    "https://www.googleapis.com/auth/drive.readonly";
/// 同时进行的凭据加载上限；超过即显式失败（可重试），绝不建立无界等待队列。
pub(crate) const MAX_CREDENTIAL_FLIGHTS: usize = 8;
/// 缓存条目上限；键只来自已加载账户，因此天然不超过账户总数。
const MAX_CREDENTIAL_CACHE_ENTRIES: usize = 8;
/// 到期前提前刷新的余量（毫秒）。
pub(crate) const CREDENTIAL_EXPIRY_SKEW_MS: i64 = 60_000;
/// 持久化 refresh token 字节上限；非法值在存取前拒绝。
const MAX_STORED_CREDENTIAL_BYTES: usize = 8 * 1024;

/// `haven:` 命名空间下的规范化小写连字符 UUID。
pub(crate) fn is_canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && uuid::Uuid::parse_str(value)
            .map(|parsed| parsed.hyphenated().to_string() == value)
            .unwrap_or(false)
}

struct CachedCredential {
    seq: u64,
    generation: i64,
    credential_ref: CredentialRef,
    credential: Arc<CloudDriveCredential>,
}

struct FlightEntry {
    seq: u64,
    waiters: usize,
    gate: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Default)]
struct SessionInner {
    cache: HashMap<String, CachedCredential>,
    flights: HashMap<String, FlightEntry>,
    next_seq: u64,
}

/// 一次通过仓储校验的凭据会话：账户内部 UUID + 代际 + 当前引用 + 凭据本身。
pub(crate) struct AccountCredential {
    pub(crate) account_id: String,
    pub(crate) generation: i64,
    pub(crate) credential_ref: CredentialRef,
    pub(crate) credential: Arc<CloudDriveCredential>,
}

/// 已写入 keystore、且在 outbox 中仍为 staged 的新引用。
pub(crate) struct StagedCredential {
    pub(crate) credential_ref: CredentialRef,
}

pub(crate) struct CredentialSessions {
    store: Arc<dyn CredentialStore>,
    auth: Arc<dyn CloudDriveAuthPort>,
    repo: Arc<dyn CloudStorageRepository>,
    inner: Mutex<SessionInner>,
}

impl CredentialSessions {
    pub(crate) fn new(
        store: Arc<dyn CredentialStore>,
        auth: Arc<dyn CloudDriveAuthPort>,
        repo: Arc<dyn CloudStorageRepository>,
    ) -> Self {
        Self {
            store,
            auth,
            repo,
            inner: Mutex::new(SessionInner::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, SessionInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 取得某账户当前可用的凭据。校验顺序固定：先规范化内部 UUID，再读账户行，
    /// 两者都通过之后才允许触碰缓存 / flight 表。
    pub(crate) async fn acquire(&self, account_id: &str) -> Result<AccountCredential, AppError> {
        if !is_canonical_uuid(account_id) {
            return Err(account_id_invalid());
        }
        let account = self.load_account(account_id).await?;
        let now = UtcMillis::now();
        let reference = account
            .credential_ref
            .clone()
            .ok_or_else(credential_unavailable)?;
        if let Some(credential) = self.cached(&account.id, account.generation, &reference, now) {
            return Ok(AccountCredential {
                account_id: account.id,
                generation: account.generation,
                credential_ref: reference,
                credential,
            });
        }
        let (gate, seq) = self.enter_flight(&account.id)?;
        let _lease = FlightLease {
            sessions: self,
            account_id: account.id.clone(),
            seq,
        };
        let guard = gate.lock_owned().await;
        let result = self.load_or_refresh(&account.id, UtcMillis::now()).await;
        drop(guard);
        result
    }

    async fn load_or_refresh(
        &self,
        account_id: &str,
        now: UtcMillis,
    ) -> Result<AccountCredential, AppError> {
        let account = self.load_account(account_id).await?;
        let reference = account
            .credential_ref
            .clone()
            .ok_or_else(credential_unavailable)?;
        if let Some(credential) = self.cached(&account.id, account.generation, &reference, now) {
            return Ok(AccountCredential {
                account_id: account.id,
                generation: account.generation,
                credential_ref: reference,
                credential,
            });
        }
        let secret = self
            .store
            .get(&reference)
            .await?
            .ok_or_else(credential_unavailable)?;
        if !valid_refresh_token(secret.expose()) {
            return Err(credential_unusable());
        }
        // 重启后只读取 refresh token，access token 始终仅存在于内存。
        // 容量维护尽力而为：outbox 清理失败不阻塞本次刷新（行仍是 ready，可重试）。
        let _ = self.sweep_ready(CLOUD_CREDENTIAL_CLEANUP_BATCH).await;
        let refreshed = Arc::new(self.auth.refresh(&secret).await?);
        let staged = self.stage_and_write(&refreshed).await?;
        match self
            .repo
            .replace_credential(
                &account.id,
                account.generation,
                &reference,
                &staged.credential_ref,
            )
            .await
        {
            Ok(CloudCasOutcome::Applied(updated)) => {
                self.remember_connected(&updated, &staged, Arc::clone(&refreshed));
                Ok(AccountCredential {
                    account_id: updated.id,
                    generation: updated.generation,
                    credential_ref: staged.credential_ref,
                    credential: refreshed,
                })
            }
            Ok(CloudCasOutcome::Stale) => Err(self.compensate(&staged, account_stale()).await),
            Ok(CloudCasOutcome::Missing) => Err(self.compensate(&staged, account_missing()).await),
            Err(err) => Err(self.compensate(&staged, err).await),
        }
    }

    /// 账户必须存在、属于本 Provider、`connected = true` 且登记了凭据引用。
    async fn load_account(&self, account_id: &str) -> Result<CloudAccount, AppError> {
        let account = self
            .repo
            .get_account(account_id)
            .await?
            .ok_or_else(account_missing)?;
        if !is_canonical_uuid(&account.id) || account.id != account_id {
            return Err(account_missing());
        }
        if account.provider != CLOUD_DRIVE_PROVIDER_ID {
            return Err(account_missing());
        }
        if !account.connected {
            return Err(account_not_connected());
        }
        if account.credential_ref.is_none() {
            return Err(credential_unavailable());
        }
        Ok(account)
    }

    /// 缓存的唯一读入口：必须同时匹配代际、当前引用与未过期，才交出缓存的 Arc。
    fn cached(
        &self,
        account_id: &str,
        generation: i64,
        reference: &CredentialRef,
        now: UtcMillis,
    ) -> Option<Arc<CloudDriveCredential>> {
        let inner = self.lock();
        let entry = inner.cache.get(account_id)?;
        if entry.generation != generation || &entry.credential_ref != reference {
            return None;
        }
        if entry.credential.expires_at_ms <= now.0.saturating_add(CREDENTIAL_EXPIRY_SKEW_MS) {
            return None;
        }
        Some(Arc::clone(&entry.credential))
    }

    fn remember(
        &self,
        account_id: &str,
        generation: i64,
        reference: &CredentialRef,
        credential: Arc<CloudDriveCredential>,
    ) {
        let mut inner = self.lock();
        inner.next_seq += 1;
        let seq = inner.next_seq;
        if !inner.cache.contains_key(account_id)
            && inner.cache.len() >= MAX_CREDENTIAL_CACHE_ENTRIES
        {
            let oldest = inner
                .cache
                .iter()
                .min_by_key(|(_, entry)| entry.seq)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                inner.cache.remove(&oldest);
            }
        }
        inner.cache.insert(
            account_id.to_owned(),
            CachedCredential {
                seq,
                generation,
                credential_ref: reference.clone(),
                credential,
            },
        );
    }

    /// 连接成功后写入缓存：账户行已是权威事实（新代际 + 新引用）。
    pub(crate) fn remember_connected(
        &self,
        account: &CloudAccount,
        staged: &StagedCredential,
        credential: Arc<CloudDriveCredential>,
    ) {
        self.remember(
            &account.id,
            account.generation,
            &staged.credential_ref,
            credential,
        );
    }

    fn enter_flight(
        &self,
        account_id: &str,
    ) -> Result<(Arc<tokio::sync::Mutex<()>>, u64), AppError> {
        let mut inner = self.lock();
        if let Some(existing) = inner.flights.get_mut(account_id) {
            if existing.waiters >= 64 {
                return Err(credential_busy());
            }
            existing.waiters += 1;
            return Ok((Arc::clone(&existing.gate), existing.seq));
        }
        if inner.flights.len() >= MAX_CREDENTIAL_FLIGHTS {
            return Err(credential_busy());
        }
        inner.next_seq += 1;
        let seq = inner.next_seq;
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        inner.flights.insert(
            account_id.to_owned(),
            FlightEntry {
                seq,
                waiters: 1,
                gate: Arc::clone(&gate),
            },
        );
        Ok((gate, seq))
    }

    fn leave_flight(&self, account_id: &str, seq: u64) {
        let mut inner = self.lock();
        let Some(entry) = inner.flights.get_mut(account_id) else {
            return;
        };
        if entry.seq != seq {
            return;
        }
        entry.waiters = entry.waiters.saturating_sub(1);
        if entry.waiters == 0 {
            inner.flights.remove(account_id);
        }
    }

    /// 断开 / 换代后立刻作废缓存条目；代际校验仍是对抗并发写入的最后一道闸。
    pub(crate) fn invalidate(&self, account_id: &str) {
        if !is_canonical_uuid(account_id) {
            return;
        }
        self.lock().cache.remove(account_id);
    }

    /// IO 后复核：账户仍 `connected`、代际与当前引用都与会话一致。
    pub(crate) async fn recheck_current(
        &self,
        session: &AccountCredential,
    ) -> Result<(), AppError> {
        let account = self
            .repo
            .get_account(&session.account_id)
            .await?
            .ok_or_else(account_stale)?;
        if !account.connected
            || account.generation != session.generation
            || account.credential_ref.as_ref() != Some(&session.credential_ref)
        {
            return Err(account_stale());
        }
        Ok(())
    }

    /// 先 stage 再写 keystore；`store.set` 失败也必须补偿（staged → ready），迟到写入因此可重试。
    pub(crate) async fn stage_and_write(
        &self,
        credential: &CloudDriveCredential,
    ) -> Result<StagedCredential, AppError> {
        validate_credential(credential)?;
        let encoded = SecretString::new(credential.refresh_token.expose());
        let reference = credential_ref(&uuid::Uuid::new_v4().to_string())?;
        self.repo.stage_credential(&reference).await?;
        if let Err(err) = self.store.set(&reference, &encoded).await {
            let staged = StagedCredential {
                credential_ref: reference,
            };
            return Err(self.compensate(&staged, err).await);
        }
        Ok(StagedCredential {
            credential_ref: reference,
        })
    }

    /// 补偿：尽力删除刚写入的秘密，并**必须**把 staged 转 ready（即使删除失败也要留下可重放
    /// 的清理记录）。原始错误保留为主错误，补偿失败作为 source 附上，绝不静默吞掉。
    pub(crate) async fn compensate(
        &self,
        staged: &StagedCredential,
        primary: AppError,
    ) -> AppError {
        let deleted = self.store.delete(&staged.credential_ref).await;
        let retired = self
            .repo
            .retire_staged_credential(&staged.credential_ref)
            .await;
        match retired {
            Ok(()) => match deleted {
                Ok(_) => primary,
                Err(err) => primary.with_source(err),
            },
            Err(err) => primary.with_source(err),
        }
    }

    /// 清理 outbox：先恢复超时 staged，再删除 ready 引用并收尾。
    /// 单条删除或收尾失败都不会中断本批其余条目；错误会返回（失败行仍是 ready，下次可重试）。
    pub(crate) async fn sweep_ready(&self, limit: u32) -> Result<u32, AppError> {
        if limit == 0 || limit > CLOUD_CREDENTIAL_CLEANUP_BATCH {
            return Err(validation("凭据清理批量非法"));
        }
        let _recovered = self
            .repo
            .recover_staged_credentials(UtcMillis::now())
            .await?;
        let pending = self.repo.pending_credentials(limit).await?;
        let mut cleaned = 0u32;
        let mut failure: Option<AppError> = None;
        for reference in pending {
            let outcome = match self.store.delete(&reference).await {
                Ok(_) => self.repo.finish_credential_cleanup(&reference).await,
                Err(err) => Err(err),
            };
            match outcome {
                // 只有 keystore 删除与 DB 收尾都成功才算清理完成。
                Ok(_) => cleaned = cleaned.saturating_add(1),
                Err(err) => {
                    if failure.is_none() {
                        failure = Some(err);
                    }
                }
            }
        }
        match failure {
            Some(err) => Err(err),
            None => Ok(cleaned),
        }
    }
}

pub(crate) fn account_id_invalid() -> AppError {
    AppError::new(
        "CLOUD_ACCOUNT_ID_INVALID",
        ErrorKind::Validation,
        "云盘账户 ID 不合法",
        false,
    )
}

pub(crate) fn account_missing() -> AppError {
    AppError::new(
        "CLOUD_ACCOUNT_NOT_FOUND",
        ErrorKind::NotFound,
        "云盘账户不存在",
        false,
    )
}

fn account_not_connected() -> AppError {
    AppError::new(
        "CLOUD_ACCOUNT_NOT_CONNECTED",
        ErrorKind::Conflict,
        "云盘账户尚未连接",
        false,
    )
}

pub(crate) fn account_stale() -> AppError {
    AppError::new(
        "CLOUD_ACCOUNT_STALE",
        ErrorKind::Conflict,
        "云盘账户状态已变化，请刷新后重试",
        true,
    )
}

fn credential_unavailable() -> AppError {
    AppError::new(
        "CLOUD_CREDENTIAL_UNAVAILABLE",
        ErrorKind::Unauthorized,
        "云盘凭据不可用，请重新连接",
        false,
    )
}

fn credential_unusable() -> AppError {
    AppError::new(
        "CLOUD_CREDENTIAL_INVALID",
        ErrorKind::Parse,
        "云盘凭据无法识别，请重新连接",
        false,
    )
}

fn credential_busy() -> AppError {
    AppError::new(
        "CLOUD_CREDENTIAL_BUSY",
        ErrorKind::Conflict,
        "云盘凭据读取繁忙，请稍后重试",
        true,
    )
}
struct FlightLease<'a> {
    sessions: &'a CredentialSessions,
    account_id: String,
    seq: u64,
}
impl Drop for FlightLease<'_> {
    fn drop(&mut self) {
        self.sessions.leave_flight(&self.account_id, self.seq);
    }
}
fn valid_refresh_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_STORED_CREDENTIAL_BYTES
        && value.bytes().all(|b| b.is_ascii_graphic())
}
fn validate_credential(credential: &CloudDriveCredential) -> Result<(), AppError> {
    if !valid_refresh_token(credential.refresh_token.expose())
        || !valid_refresh_token(credential.access_token.expose())
        || credential.expires_at_ms <= UtcMillis::now().0
        || credential
            .scope
            .as_deref()
            .is_some_and(|scope| scope != CLOUD_DRIVE_READONLY_SCOPE)
    {
        return Err(credential_unusable());
    }
    Ok(())
}
