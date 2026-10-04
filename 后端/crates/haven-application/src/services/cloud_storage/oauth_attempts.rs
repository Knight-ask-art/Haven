//! 云盘 OAuth 尝试的 Application 侧登记表与提交门闸。
//!
//! 边界：
//! - 只保存**非敏感**编排事实：Provider 尝试 ID、到期时刻、授权开始时捕获的账户锚点与
//!   状态机。授权码、state、verifier、回调 query 与令牌一律不进入本模块。
//! - 「领取完成」（claim）只允许一次，但领取后条目仍留在表里，取消始终可达：取消与最终
//!   connect 共享同一把异步门闸，因此取消要么在提交前把尝试作废，要么看到已完成的终态，
//!   不会出现「取消成功、凭据却已写入」。
//! - 同步 `Mutex` 只覆盖极短读改写，绝不跨 `.await` 持有；跨等待的点一律用
//!   `tokio::sync::Mutex`。门闸只覆盖**最终 connect 提交**，不覆盖 about / keystore IO。
//! - 表有界：进行中的尝试有硬上限，终态条目按登记序回收；所有键先做规范化 UUID 校验，
//!   外部随意字符串既进不了表，也撑不大表。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use haven_common::{AppError, ErrorKind, UtcMillis};

use super::credential_session::is_canonical_uuid;
use super::ports::OAuthStatus;

/// 同时处于「进行中」（Pending / Completing）的尝试上限。
pub(crate) const MAX_TRACKED_OAUTH_ATTEMPTS: usize = 8;
/// 终态尝试的有界保留数量；更早的终态在下次 `begin` 时回收。
const MAX_RETAINED_OAUTH_ATTEMPTS: usize = 16;

/// 授权开始时捕获的账户锚点。
///
/// 授权完成后只有「行与代际都与捕获时一致」才允许提交：代际变过、或身份在授权期间
/// 才出现的账户都是并发冲突，绝不静默接管。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OAuthAttemptAnchor {
    /// 调用方指名了既有账户行。含断开墓碑：只要代际未变就仍可重新授权。
    Existing {
        account_id: String,
        provider_account_id: String,
        generation: i64,
    },
    /// 新连接：授权解析出的身份在提交时仍必须没有任何账户行。
    Absent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OAuthAttemptState {
    Pending,
    Completing,
    Committing,
    Cancelled,
    Failed,
    Expired,
    Completed,
}

impl OAuthAttemptState {
    fn is_live(self) -> bool {
        matches!(self, Self::Pending | Self::Completing | Self::Committing)
    }
}

struct AttemptEntry {
    seq: u64,
    provider_attempt_id: String,
    expires_at: UtcMillis,
    anchor: OAuthAttemptAnchor,
    state: OAuthAttemptState,
    gate: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Default)]
struct RegistryInner {
    attempts: HashMap<String, AttemptEntry>,
    next_seq: u64,
}

/// 有界 OAuth 尝试登记表；条目只由 [`OAuthAttemptRegistry::begin`] 创建。
pub(crate) struct OAuthAttemptRegistry {
    inner: Mutex<RegistryInner>,
}

impl OAuthAttemptRegistry {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(RegistryInner::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, RegistryInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 登记一次已由 Provider 侧发起的尝试。失败时调用方必须取消 Provider 侧尝试。
    pub(crate) fn begin(
        &self,
        attempt_id: &str,
        expires_at: UtcMillis,
        anchor: OAuthAttemptAnchor,
    ) -> Result<(), AppError> {
        let attempt_id = validate_attempt_id(attempt_id)?;
        let mut inner = self.lock();
        expire_due(&mut inner, UtcMillis::now());
        if inner.attempts.contains_key(&attempt_id) {
            return Err(attempt_conflict());
        }
        let live = inner
            .attempts
            .values()
            .filter(|entry| entry.state.is_live())
            .count();
        if live >= MAX_TRACKED_OAUTH_ATTEMPTS {
            return Err(attempt_limit());
        }
        inner.next_seq += 1;
        let seq = inner.next_seq;
        inner.attempts.insert(
            attempt_id.clone(),
            AttemptEntry {
                seq,
                provider_attempt_id: attempt_id,
                expires_at,
                anchor,
                state: OAuthAttemptState::Pending,
                gate: Arc::new(tokio::sync::Mutex::new(())),
            },
        );
        prune_terminal(&mut inner);
        Ok(())
    }

    pub(crate) fn state(&self, attempt_id: &str) -> Result<OAuthAttemptState, AppError> {
        self.read(attempt_id, |entry| entry.state)
    }

    pub(crate) fn expires_at(&self, attempt_id: &str) -> Result<UtcMillis, AppError> {
        self.read(attempt_id, |entry| entry.expires_at)
    }

    pub(crate) fn provider_attempt_id(&self, attempt_id: &str) -> Result<String, AppError> {
        self.read(attempt_id, |entry| entry.provider_attempt_id.clone())
    }

    pub(crate) fn anchor(&self, attempt_id: &str) -> Result<OAuthAttemptAnchor, AppError> {
        self.read(attempt_id, |entry| entry.anchor.clone())
    }

    /// 恰好一次地从 Pending 领取为 Completing；重复领取显式失败。
    pub(crate) fn claim(&self, attempt_id: &str) -> Result<(), AppError> {
        let attempt_id = validate_attempt_id(attempt_id)?;
        let mut inner = self.lock();
        expire_due(&mut inner, UtcMillis::now());
        let entry = inner
            .attempts
            .get_mut(&attempt_id)
            .ok_or_else(attempt_not_found)?;
        match entry.state {
            OAuthAttemptState::Pending => {
                entry.state = OAuthAttemptState::Completing;
                Ok(())
            }
            OAuthAttemptState::Completing | OAuthAttemptState::Committing => {
                Err(attempt_in_progress())
            }
            OAuthAttemptState::Cancelled => Err(attempt_cancelled()),
            OAuthAttemptState::Expired => Err(attempt_expired()),
            OAuthAttemptState::Failed | OAuthAttemptState::Completed => Err(attempt_finished()),
        }
    }

    /// 领取后、真正动 IO 之前失败：把租约退回 Pending，让调用方可以重试（幂等）。
    pub(crate) fn release_claim(&self, attempt_id: &str) {
        self.mutate(attempt_id, |entry| {
            if entry.state == OAuthAttemptState::Completing {
                entry.state = OAuthAttemptState::Pending;
            }
        });
    }

    /// A late Provider poll cannot overwrite a concurrently claimed/committed attempt.
    pub(crate) fn observe_poll(
        &self,
        attempt_id: &str,
        observed: OAuthStatus,
    ) -> Result<(OAuthAttemptState, bool), AppError> {
        let attempt_id = validate_attempt_id(attempt_id)?;
        let mut inner = self.lock();
        expire_due(&mut inner, UtcMillis::now());
        let entry = inner
            .attempts
            .get_mut(&attempt_id)
            .ok_or_else(attempt_not_found)?;
        if entry.state != OAuthAttemptState::Pending {
            return Ok((entry.state, false));
        }
        match observed {
            OAuthStatus::Pending => {}
            OAuthStatus::Authorized => return Ok((entry.state, true)),
            OAuthStatus::Cancelled => entry.state = OAuthAttemptState::Cancelled,
            OAuthStatus::Expired => entry.state = OAuthAttemptState::Expired,
            OAuthStatus::Failed | OAuthStatus::Consumed => entry.state = OAuthAttemptState::Failed,
        }
        Ok((entry.state, false))
    }

    pub(crate) fn completion_lease<'a>(&'a self, attempt_id: &'a str) -> OAuthCompletionLease<'a> {
        OAuthCompletionLease {
            registry: self,
            attempt_id,
            armed: true,
            gate: None,
        }
    }

    /// 每次 IO 前后复核：只有仍然 Completing 且未到点才允许继续。
    pub(crate) fn ensure_live(&self, attempt_id: &str) -> Result<(), AppError> {
        match self.state(attempt_id)? {
            OAuthAttemptState::Completing => Ok(()),
            OAuthAttemptState::Cancelled => Err(attempt_cancelled()),
            OAuthAttemptState::Expired => Err(attempt_expired()),
            _ => Err(attempt_finished()),
        }
    }

    /// 最终 connect 的提交前判定；调用方必须已持有 [`Self::lock_commit_gate`] 的门闸。
    pub(crate) fn ensure_committable(&self, attempt_id: &str) -> Result<(), AppError> {
        let attempt_id = validate_attempt_id(attempt_id)?;
        let mut inner = self.lock();
        expire_due(&mut inner, UtcMillis::now());
        let entry = inner
            .attempts
            .get_mut(&attempt_id)
            .ok_or_else(attempt_not_found)?;
        match entry.state {
            OAuthAttemptState::Completing => {
                entry.state = OAuthAttemptState::Committing;
                Ok(())
            }
            OAuthAttemptState::Cancelled => Err(attempt_cancelled()),
            OAuthAttemptState::Expired => Err(attempt_expired()),
            _ => Err(attempt_finished()),
        }
    }

    /// 终态只写一次：已经被取消 / 过期的尝试不会被后来的失败覆盖。
    pub(crate) fn mark_terminal(&self, attempt_id: &str, state: OAuthAttemptState) {
        self.mutate(attempt_id, |entry| {
            if entry.state.is_live() {
                entry.state = state;
            }
        });
    }

    /// 取消与最终提交共享同一把门闸：取消要么先失效尝试，要么看到已完成 / 已终止的终态。
    pub(crate) async fn cancel(&self, attempt_id: &str) -> Result<OAuthAttemptState, AppError> {
        let gate = self.gate(attempt_id)?;
        let guard = gate.lock_owned().await;
        let attempt_id = validate_attempt_id(attempt_id)?;
        let mut inner = self.lock();
        let now = UtcMillis::now();
        let Some(entry) = inner.attempts.get_mut(&attempt_id) else {
            drop(guard);
            return Err(attempt_not_found());
        };
        if matches!(
            entry.state,
            OAuthAttemptState::Pending | OAuthAttemptState::Completing
        ) && now.0 >= entry.expires_at.0
        {
            entry.state = OAuthAttemptState::Expired;
            drop(guard);
            return Ok(OAuthAttemptState::Expired);
        }
        let state = match entry.state {
            OAuthAttemptState::Pending | OAuthAttemptState::Completing => {
                entry.state = OAuthAttemptState::Cancelled;
                OAuthAttemptState::Cancelled
            }
            terminal => terminal,
        };
        drop(guard);
        Ok(state)
    }

    /// 取得最终提交门闸。持闸期间只允许执行最终的那一次仓储 `connect`，不得夹带 IO。
    pub(crate) async fn lock_commit_gate(
        &self,
        attempt_id: &str,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, AppError> {
        let gate = self.gate(attempt_id)?;
        Ok(gate.lock_owned().await)
    }

    fn gate(&self, attempt_id: &str) -> Result<Arc<tokio::sync::Mutex<()>>, AppError> {
        self.read(attempt_id, |entry| Arc::clone(&entry.gate))
    }

    fn read<T>(&self, attempt_id: &str, f: impl FnOnce(&AttemptEntry) -> T) -> Result<T, AppError> {
        let attempt_id = validate_attempt_id(attempt_id)?;
        let mut inner = self.lock();
        expire_due(&mut inner, UtcMillis::now());
        inner
            .attempts
            .get(&attempt_id)
            .map(f)
            .ok_or_else(attempt_not_found)
    }

    fn mutate(&self, attempt_id: &str, f: impl FnOnce(&mut AttemptEntry)) {
        if !is_canonical_uuid(attempt_id) {
            return;
        }
        let mut inner = self.lock();
        expire_due(&mut inner, UtcMillis::now());
        if let Some(entry) = inner.attempts.get_mut(attempt_id) {
            f(entry);
        }
    }
}

/// Dropped/erroring completion terminates explicitly. Owned gate is released only after that fact.
pub(crate) struct OAuthCompletionLease<'a> {
    registry: &'a OAuthAttemptRegistry,
    attempt_id: &'a str,
    armed: bool,
    gate: Option<tokio::sync::OwnedMutexGuard<()>>,
}
impl OAuthCompletionLease<'_> {
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
    pub(crate) fn hold_gate(&mut self, gate: tokio::sync::OwnedMutexGuard<()>) {
        self.gate = Some(gate);
    }
    pub(crate) fn release_gate(&mut self) {
        self.gate = None;
    }
}
impl Drop for OAuthCompletionLease<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.registry
                .mark_terminal(self.attempt_id, OAuthAttemptState::Failed);
        }
    }
}

/// 尝试 ID 与 Provider 侧同一串（Infrastructure 生成的随机 UUID），但仍先做规范化校验。
fn validate_attempt_id(raw: &str) -> Result<String, AppError> {
    if !is_canonical_uuid(raw) {
        return Err(attempt_id_invalid());
    }
    Ok(raw.to_owned())
}

/// 提交前到点即过期；Committing 的短事务结果由持闸方收口，不能被轮询改写。
fn expire_due(inner: &mut RegistryInner, now: UtcMillis) {
    for entry in inner.attempts.values_mut() {
        if matches!(
            entry.state,
            OAuthAttemptState::Pending | OAuthAttemptState::Completing
        ) && now.0 >= entry.expires_at.0
        {
            entry.state = OAuthAttemptState::Expired;
        }
    }
}

/// 终态有界保留：超出上限时按登记序回收最早的终态条目。
fn prune_terminal(inner: &mut RegistryInner) {
    let mut terminal: Vec<(u64, String)> = inner
        .attempts
        .iter()
        .filter(|(_, entry)| !entry.state.is_live())
        .map(|(id, entry)| (entry.seq, id.clone()))
        .collect();
    if terminal.len() <= MAX_RETAINED_OAUTH_ATTEMPTS {
        return;
    }
    terminal.sort_unstable_by_key(|(seq, _)| *seq);
    let excess = terminal.len() - MAX_RETAINED_OAUTH_ATTEMPTS;
    for (_, id) in terminal.into_iter().take(excess) {
        inner.attempts.remove(&id);
    }
}

fn attempt_not_found() -> AppError {
    AppError::new(
        "CLOUD_OAUTH_ATTEMPT_NOT_FOUND",
        ErrorKind::NotFound,
        "云盘授权尝试不存在",
        false,
    )
}

fn attempt_id_invalid() -> AppError {
    AppError::new(
        "CLOUD_OAUTH_ATTEMPT_ID_INVALID",
        ErrorKind::Validation,
        "云盘授权尝试 ID 不合法",
        false,
    )
}

fn attempt_limit() -> AppError {
    AppError::new(
        "CLOUD_OAUTH_ATTEMPT_LIMIT",
        ErrorKind::Conflict,
        "进行中的云盘授权过多，请先取消或等待过期",
        true,
    )
}

fn attempt_in_progress() -> AppError {
    AppError::new(
        "CLOUD_OAUTH_ATTEMPT_IN_PROGRESS",
        ErrorKind::Conflict,
        "云盘授权正在完成中",
        true,
    )
}

fn attempt_conflict() -> AppError {
    AppError::new(
        "CLOUD_OAUTH_ATTEMPT_CONFLICT",
        ErrorKind::Conflict,
        "云盘授权尝试已存在",
        false,
    )
}

pub(crate) fn attempt_cancelled() -> AppError {
    AppError::new(
        "CLOUD_OAUTH_CANCELLED",
        ErrorKind::Cancelled,
        "云盘授权已取消",
        false,
    )
}

pub(crate) fn attempt_expired() -> AppError {
    AppError::new(
        "CLOUD_OAUTH_ATTEMPT_EXPIRED",
        ErrorKind::Timeout,
        "云盘授权已过期，请重新发起",
        false,
    )
}

pub(crate) fn attempt_finished() -> AppError {
    AppError::new(
        "CLOUD_OAUTH_ATTEMPT_FINISHED",
        ErrorKind::Conflict,
        "云盘授权尝试已结束",
        false,
    )
}

#[cfg(test)]
mod tests;
