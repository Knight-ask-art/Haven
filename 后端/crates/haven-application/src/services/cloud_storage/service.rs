//! CloudStorageService：云盘账户 / 目录 / 对象绑定的用例编排（Google Drive 只读切片）。
//!
//! 边界：
//! - 真实 Provider ID（账户 permissionId、folder ID、file ID）只在本层与仓储之间流动，
//!   绝不进入任何 `*View`、错误文案或日志；视图只含内部 UUID 与非敏感事实。
//! - `CredentialStore` 只持久化 refresh token；access token 只在内存会话中使用，
//!   DB 只保存 `CredentialRef`；令牌、授权码与回调 query 不进入页面视图。
//! - 网络 IO 一律在仓储事务之外；每个仓储方法自身是一个短事务，本层不持有事务跨 IO。
//! - OAuth 完成路径的取消语义见 `oauth_attempts`：领取只发生一次，取消在最终 connect
//!   提交前始终有效；提交门闸只覆盖最终 connect，不覆盖 about / keystore IO。
//! - 账户代际与当前 `credential_ref` 是所有网络 IO 的前后置条件；IO 后复核不一致就丢弃
//!   结果，绝不凭旧凭据重试。

use std::sync::Arc;

use haven_common::{AppError, ErrorKind};
use haven_domain::credential::CredentialStore;

use crate::services::ports::{RemoteByteRange, RemoteSessionBody};

use super::credential_session::{
    CLOUD_DRIVE_READONLY_SCOPE, CredentialSessions, account_id_invalid, account_missing,
    account_stale, is_canonical_uuid,
};
use super::oauth_attempts::{
    OAuthAttemptAnchor, OAuthAttemptRegistry, OAuthAttemptState, attempt_cancelled,
    attempt_expired, attempt_finished,
};
use super::pdf_read;
use super::ports::{
    CLOUD_DRIVE_PROVIDER_ID, CloudDriveAccount, CloudDriveAuthPort, CloudDrivePort, OAuthStatus,
};
use super::state::{
    CLOUD_CREDENTIAL_CLEANUP_BATCH, CloudAccount, CloudCasOutcome, CloudConnectRequest,
    CloudStorageRepository, cloud_binding_stale,
};

use super::views::*;

#[derive(Clone)]
pub struct CloudStorageService {
    pub(crate) repo: Arc<dyn CloudStorageRepository>,
    pub(crate) drive: Arc<dyn CloudDrivePort>,
    auth: Arc<dyn CloudDriveAuthPort>,
    pub(crate) credentials: Arc<CredentialSessions>,
    attempts: Arc<OAuthAttemptRegistry>,
}

impl CloudStorageService {
    pub fn new(
        repo: Arc<dyn CloudStorageRepository>,
        drive: Arc<dyn CloudDrivePort>,
        auth: Arc<dyn CloudDriveAuthPort>,
        store: Arc<dyn CredentialStore>,
    ) -> Self {
        Self {
            credentials: Arc::new(CredentialSessions::new(
                store,
                Arc::clone(&auth),
                Arc::clone(&repo),
            )),
            repo,
            drive,
            auth,
            attempts: Arc::new(OAuthAttemptRegistry::new()),
        }
    }

    /// Whether the desktop build can initiate OAuth; no secret/configuration leaves Rust.
    pub fn oauth_available(&self) -> bool {
        self.auth.is_configured()
    }

    /// 全部账户（含断开墓碑）；断开与否由 `connected` 事实表达。
    pub async fn list_accounts(&self) -> Result<Vec<CloudAccountView>, AppError> {
        let accounts = self.repo.list_accounts().await?;
        Ok(accounts.iter().map(account_view).collect())
    }

    /// 发起一次授权：先维护 outbox，再由 Provider 侧打开浏览器，最后登记本地租约。
    pub async fn begin_connect(
        &self,
        request: CloudConnectBeginRequest,
    ) -> Result<CloudConnectAttemptView, AppError> {
        let anchor = match request.account_id.as_deref() {
            None => OAuthAttemptAnchor::Absent,
            Some(raw) => {
                if !is_canonical_uuid(raw) {
                    return Err(account_id_invalid());
                }
                let account = self
                    .repo
                    .get_account(raw)
                    .await?
                    .ok_or_else(account_missing)?;
                // 断开墓碑也可重新授权：只要代际仍是捕获时的那一代（见 complete_connect）。
                OAuthAttemptAnchor::Existing {
                    account_id: account.id,
                    provider_account_id: account.provider_account_id,
                    generation: account.generation,
                }
            }
        };
        // 容量维护尽力而为：回收可清理的 ready 引用；清理失败只影响容量回收，
        // 绝不能把一次授权发起变成失败（失败行仍是 ready，后续可重试）。
        self.best_effort_sweep().await;
        let attempt = self
            .auth
            .start(&[CLOUD_DRIVE_READONLY_SCOPE.to_owned()])
            .await?;
        if let Err(err) = self.attempts.begin(&attempt.id, attempt.expires_at, anchor) {
            return Err(match self.auth.cancel(&attempt.id).await {
                Ok(()) => err,
                Err(cancel_err) => err.with_source(cancel_err),
            });
        }
        Ok(CloudConnectAttemptView {
            attempt_id: attempt.id,
            expires_at_ms: attempt.expires_at.0,
        })
    }

    /// 只读轮询；本地已取消 / 已过期的尝试不再回问 Provider。
    pub async fn poll_connect(&self, attempt_id: &str) -> Result<CloudConnectPollView, AppError> {
        let state = self.attempts.state(attempt_id)?;
        let expires_at_ms = self.attempts.expires_at(attempt_id)?.0;
        let status = match state {
            OAuthAttemptState::Pending => {
                let provider_attempt_id = self.attempts.provider_attempt_id(attempt_id)?;
                let observed = self.auth.poll(&provider_attempt_id).await?;
                let (current, authorized) = self.attempts.observe_poll(attempt_id, observed)?;
                if authorized {
                    CloudConnectStatus::Authorized
                } else {
                    status_of(current)
                }
            }
            OAuthAttemptState::Completing | OAuthAttemptState::Committing => {
                CloudConnectStatus::Completing
            }
            OAuthAttemptState::Cancelled => CloudConnectStatus::Cancelled,
            OAuthAttemptState::Failed => CloudConnectStatus::Failed,
            OAuthAttemptState::Expired => CloudConnectStatus::Expired,
            OAuthAttemptState::Completed => CloudConnectStatus::Completed,
        };
        Ok(CloudConnectPollView {
            status,
            expires_at_ms,
        })
    }

    /// 完成授权：领取（恰好一次）→ 取回结果 → `about` 解析身份 → 最终 connect。
    /// 取消在拿到凭据之后、最终 connect 提交之前仍然有效。
    pub async fn complete_connect(&self, attempt_id: &str) -> Result<CloudAccountView, AppError> {
        self.attempts.claim(attempt_id)?;
        let mut completion = self.attempts.completion_lease(attempt_id);
        let provider_attempt_id = self.attempts.provider_attempt_id(attempt_id)?;
        let anchor = self.attempts.anchor(attempt_id)?;

        match self.auth.poll(&provider_attempt_id).await? {
            OAuthStatus::Authorized => {}
            OAuthStatus::Pending => {
                completion.disarm();
                self.attempts.release_claim(attempt_id);
                return Err(oauth_not_authorized());
            }
            OAuthStatus::Cancelled => {
                self.attempts
                    .mark_terminal(attempt_id, OAuthAttemptState::Cancelled);
                return Err(attempt_cancelled());
            }
            OAuthStatus::Expired => {
                self.attempts
                    .mark_terminal(attempt_id, OAuthAttemptState::Expired);
                return Err(attempt_expired());
            }
            OAuthStatus::Failed => {
                self.attempts
                    .mark_terminal(attempt_id, OAuthAttemptState::Failed);
                return Err(oauth_failed());
            }
            OAuthStatus::Consumed => {
                self.attempts
                    .mark_terminal(attempt_id, OAuthAttemptState::Failed);
                return Err(attempt_finished());
            }
        }
        let Some(credential) = self.auth.take_result(&provider_attempt_id).await? else {
            completion.disarm();
            self.attempts.release_claim(attempt_id);
            return Err(oauth_not_authorized());
        };
        let credential = Arc::new(credential);
        self.attempts.ensure_live(attempt_id)?;

        let about = match self.drive.about(&credential).await {
            Ok(about) => about,
            Err(err) => {
                self.attempts
                    .mark_terminal(attempt_id, OAuthAttemptState::Failed);
                return Err(err);
            }
        };
        self.attempts.ensure_live(attempt_id)?;

        let (expected_prior_generation, request) =
            match self.resolve_connect_identity(&anchor, &about).await {
                Ok(resolved) => resolved,
                Err(err) => {
                    self.attempts
                        .mark_terminal(attempt_id, OAuthAttemptState::Failed);
                    return Err(err);
                }
            };

        // 先 stage 再 store.set：即使两层之间崩溃，outbox 也有可重放的清理所有者。
        let staged = match self.credentials.stage_and_write(&credential).await {
            Ok(staged) => staged,
            Err(err) => {
                self.attempts
                    .mark_terminal(attempt_id, OAuthAttemptState::Failed);
                return Err(err);
            }
        };
        // keystore IO 之后重新确认：取消 / 过期必须让本次写入在提交前作废。
        if let Err(err) = self.attempts.ensure_live(attempt_id) {
            return Err(self.credentials.compensate(&staged, err).await);
        }

        let gate = match self.attempts.lock_commit_gate(attempt_id).await {
            Ok(gate) => gate,
            Err(err) => return Err(self.credentials.compensate(&staged, err).await),
        };
        completion.hold_gate(gate);
        if let Err(err) = self.attempts.ensure_committable(attempt_id) {
            completion.release_gate();
            return Err(self.credentials.compensate(&staged, err).await);
        }
        let outcome = self
            .repo
            .connect(&request, expected_prior_generation, &staged.credential_ref)
            .await;
        match outcome {
            Ok(CloudCasOutcome::Applied(account)) => {
                self.attempts
                    .mark_terminal(attempt_id, OAuthAttemptState::Completed);
                completion.release_gate();
                self.credentials
                    .remember_connected(&account, &staged, Arc::clone(&credential));
                Ok(account_view(&account))
            }
            // Err / Stale / Missing 一律补偿：删除刚写入的秘密并把 staged 转 ready（可重试）。
            Ok(CloudCasOutcome::Stale) => {
                self.attempts
                    .mark_terminal(attempt_id, OAuthAttemptState::Failed);
                completion.release_gate();
                Err(self.credentials.compensate(&staged, account_stale()).await)
            }
            Ok(CloudCasOutcome::Missing) => {
                self.attempts
                    .mark_terminal(attempt_id, OAuthAttemptState::Failed);
                completion.release_gate();
                Err(self
                    .credentials
                    .compensate(&staged, account_missing())
                    .await)
            }
            Err(err) => {
                self.attempts
                    .mark_terminal(attempt_id, OAuthAttemptState::Failed);
                completion.release_gate();
                Err(self.credentials.compensate(&staged, err).await)
            }
        }
    }

    /// 取消：与最终提交共享门闸；已完成的授权不会被“取消”，返回值如实反映终态。
    pub async fn cancel_connect(&self, attempt_id: &str) -> Result<CloudConnectStatus, AppError> {
        let state = self.attempts.cancel(attempt_id).await?;
        if state == OAuthAttemptState::Cancelled {
            let provider_attempt_id = self.attempts.provider_attempt_id(attempt_id)?;
            self.auth.cancel(&provider_attempt_id).await?;
        }
        Ok(status_of(state))
    }

    pub async fn disconnect_account(
        &self,
        account_id: &str,
        expected_generation: i64,
    ) -> Result<CloudAccountView, AppError> {
        if !is_canonical_uuid(account_id) {
            return Err(account_id_invalid());
        }
        match self
            .repo
            .disconnect(account_id, expected_generation)
            .await?
        {
            CloudCasOutcome::Applied(account) => {
                // 墓碑已在本次 DB 事务中提交：断开成功只由这次变更事实决定。
                self.credentials.invalidate(&account.id);
                // 秘密删除尽力而为：失败行仍是 ready，可由显式 / 后台清理重试；
                // 不能因维护失败就让调用方以为断开未发生（那会跳过库变更事件）。
                self.best_effort_sweep().await;
                Ok(account_view(&account))
            }
            CloudCasOutcome::Stale => Err(account_stale()),
            CloudCasOutcome::Missing => Err(account_missing()),
        }
    }

    /// 显式清理入口：删除 ready 引用并收尾账户行；失败的行保持 ready，可重试。
    pub async fn sweep_credential_cleanup(&self) -> Result<u32, AppError> {
        self.credentials
            .sweep_ready(CLOUD_CREDENTIAL_CLEANUP_BATCH)
            .await
    }

    /// 前台维护尽力而为：outbox 容量回收失败不是用例失败，也不削弱代际 / 当前引用校验。
    /// 显式 [`Self::sweep_credential_cleanup`] 仍保持可失败、可重试。
    async fn best_effort_sweep(&self) {
        let _ = self
            .credentials
            .sweep_ready(CLOUD_CREDENTIAL_CLEANUP_BATCH)
            .await;
    }

    pub async fn object_snapshot(
        &self,
        object_id: &str,
    ) -> Result<super::state::CloudObjectSnapshot, AppError> {
        let snapshot = self
            .repo
            .object_snapshot(object_id)
            .await?
            .ok_or_else(cloud_binding_stale)?;
        self.repo
            .assert_current(snapshot.account_generation, &snapshot)
            .await?;
        Ok(snapshot)
    }

    /// 会话固定快照，不允许断开后旧会话自动追随新授权。
    pub async fn read_pdf(
        &self,
        object_id: &str,
        expected: &super::state::CloudObjectSnapshot,
        range: Option<RemoteByteRange>,
    ) -> Result<RemoteSessionBody, AppError> {
        pdf_read::read_pdf_object(
            &*self.repo,
            &*self.drive,
            &self.credentials,
            object_id,
            expected,
            range,
        )
        .await
    }

    /// 把授权身份落到账户行：`Absent` 要求身份仍未出现，`Existing` 要求行与代际未变。
    /// 断开墓碑（`connected = false`）只要代际没变就仍可重新授权。
    async fn resolve_connect_identity(
        &self,
        anchor: &OAuthAttemptAnchor,
        about: &CloudDriveAccount,
    ) -> Result<(Option<i64>, CloudConnectRequest), AppError> {
        let request = CloudConnectRequest {
            provider: CLOUD_DRIVE_PROVIDER_ID.to_owned(),
            provider_account_id: about.provider_account_id.clone(),
            display_name: about.display_name.clone(),
        };
        match anchor {
            OAuthAttemptAnchor::Existing {
                account_id,
                provider_account_id,
                generation,
            } => {
                if provider_account_id != &about.provider_account_id {
                    return Err(oauth_account_mismatch());
                }
                let current = self
                    .repo
                    .get_account(account_id)
                    .await?
                    .ok_or_else(account_missing)?;
                if current.generation != *generation
                    || current.provider_account_id != about.provider_account_id
                {
                    return Err(account_stale());
                }
                Ok((Some(*generation), request))
            }
            OAuthAttemptAnchor::Absent => {
                let existing = self
                    .repo
                    .get_account_by_provider(CLOUD_DRIVE_PROVIDER_ID, &about.provider_account_id)
                    .await?;
                if existing.is_some() {
                    return Err(account_appeared());
                }
                Ok((None, request))
            }
        }
    }
}

fn status_of(state: OAuthAttemptState) -> CloudConnectStatus {
    match state {
        OAuthAttemptState::Pending => CloudConnectStatus::Pending,
        OAuthAttemptState::Completing | OAuthAttemptState::Committing => {
            CloudConnectStatus::Completing
        }
        OAuthAttemptState::Cancelled => CloudConnectStatus::Cancelled,
        OAuthAttemptState::Failed => CloudConnectStatus::Failed,
        OAuthAttemptState::Expired => CloudConnectStatus::Expired,
        OAuthAttemptState::Completed => CloudConnectStatus::Completed,
    }
}

fn account_view(account: &CloudAccount) -> CloudAccountView {
    CloudAccountView {
        id: account.id.clone(),
        display_name: account.display_name.clone(),
        connected: account.connected,
        generation: account.generation,
    }
}

fn oauth_not_authorized() -> AppError {
    AppError::new(
        "CLOUD_OAUTH_NOT_AUTHORIZED",
        ErrorKind::Conflict,
        "云盘授权尚未完成，请继续等待回调",
        true,
    )
}

fn oauth_failed() -> AppError {
    AppError::new(
        "CLOUD_OAUTH_FAILED",
        ErrorKind::Unauthorized,
        "云盘授权未完成，请重新发起",
        false,
    )
}

fn oauth_account_mismatch() -> AppError {
    AppError::new(
        "CLOUD_OAUTH_ACCOUNT_MISMATCH",
        ErrorKind::Conflict,
        "本次授权返回的云盘账户与请求的账户不一致",
        false,
    )
}

fn account_appeared() -> AppError {
    AppError::new(
        "CLOUD_ACCOUNT_APPEARED",
        ErrorKind::Conflict,
        "该云盘账户在授权期间已被连接，请刷新后重试",
        true,
    )
}
