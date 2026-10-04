//! 桌面 Google OAuth 运行时（Infrastructure）：系统浏览器 + loopback 回调的完整生命周期。
//!
//! 覆盖 `start` / `poll` / `cancel` / `take_result` / `refresh`：单飞发起、随机 UUID 尝试、
//! 总时限 5 分钟（含初始化、回调与换取令牌）、到点即清除秘密结果并关闭监听、终态有界保留。
//! 监听只绑 `127.0.0.1` 随机端口，只接受 `GET /oauth2/callback`；Host、重复头、
//! Content-Length / Transfer-Encoding 与请求读取（16 KiB / 3 秒）严格有界。回调 query 是
//! 不可信输入，交给 `OAuthChallenge` 校验；state 不匹配、重复参数或非法编码不会消费真实
//! 尝试，也不会延长时限。授权码、state、verifier 与回调 URL 不进入日志、Debug、错误文案或
//! 任何 DTO。系统浏览器由注入的 `CloudOAuthBrowserPort` 打开，本模块不派生子进程、不调用
//! shell。监听任务句柄归本结构所有，`Drop` 会中止全部任务，不泄漏端口或后台任务。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use uuid::Uuid;

use haven_application::services::cloud_storage::ports::{
    CloudDriveAuthAttempt, CloudDriveAuthPort, CloudDriveCredential, CloudOAuthBrowserPort,
    OAuthStatus,
};
use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::credential::SecretString;

use crate::cloud_drive::handshake::{
    CallbackOutcome, GOOGLE_DRIVE_READONLY_SCOPE, OAuthChallenge, loopback_redirect_uri,
};
use crate::cloud_drive::tokens::GoogleOAuthTokens;

/// 授权总时限（毫秒）：初始化、回调与换取令牌都算在内。
const AUTH_TTL_MS: i64 = 300_000;
/// 单次回调请求的读取预算：整段请求头必须在 3 秒内读完。
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(3);
/// 单次回调请求的字节上限（请求行 + 全部请求头）。
const MAX_REQUEST_BYTES: usize = 16 * 1024;
const MAX_HEADER_COUNT: usize = 64;
/// 有界保留的终态尝试数量；更早的终态在下次 `start` 时回收。
const MAX_RETAINED_ATTEMPTS: usize = 16;
/// 与 handshake 的冻结错误契约一致：state 匹配但 provider 拒绝是真实终态。
const PROVIDER_REJECTED_CODE: &str = "CLOUD_OAUTH_PROVIDER_REJECTED";
const UNRELATED_BODY: &str =
    "<!doctype html><meta charset=\"utf-8\"><title>Haven</title>该地址与本次云盘授权无关。";
const GRANTED_BODY: &str = "<!doctype html><meta charset=\"utf-8\"><title>Haven</title>已收到授权回调，请回到 Haven 确认连接结果。";
const DENIED_BODY: &str =
    "<!doctype html><meta charset=\"utf-8\"><title>Haven</title>已取消本次云盘授权。";
const FAILED_BODY: &str =
    "<!doctype html><meta charset=\"utf-8\"><title>Haven</title>授权未完成，请回到应用重试。";

/// 稳定错误码；对外只暴露 code / kind / 固定文案，不回显任何回调内容。
fn oauth_error(code: &'static str, kind: ErrorKind, message: &'static str) -> AppError {
    AppError::new(code, kind, message, false)
}
fn already_active() -> AppError {
    oauth_error(
        "CLOUD_OAUTH_ATTEMPT_ACTIVE",
        ErrorKind::Conflict,
        "已有进行中的云盘授权",
    )
}
fn attempt_not_found() -> AppError {
    oauth_error(
        "CLOUD_OAUTH_ATTEMPT_NOT_FOUND",
        ErrorKind::NotFound,
        "云盘授权尝试不存在",
    )
}
fn attempt_id_invalid() -> AppError {
    oauth_error(
        "CLOUD_OAUTH_ATTEMPT_ID_INVALID",
        ErrorKind::Validation,
        "云盘授权尝试 ID 不合法",
    )
}
fn scope_rejected() -> AppError {
    oauth_error(
        "CLOUD_OAUTH_SCOPE_REJECTED",
        ErrorKind::Validation,
        "云盘授权只接受固定只读 scope",
    )
}
fn listener_failed(message: &'static str) -> AppError {
    AppError::new("CLOUD_OAUTH_LISTENER_FAILED", ErrorKind::Io, message, true)
}

/// 令牌端口的内部抽象：生产实现是 [`GoogleOAuthTokens`]，测试注入进程内 fake。
/// 这四个方法就是冻结的 token API；本模块不自定义 client id、scope 或端点。
#[async_trait]
trait OAuthTokensPort: Send + Sync {
    fn ensure_configured(&self) -> Result<(), AppError>;
    fn authorization_url(
        &self,
        redirect_uri: &str,
        challenge: &OAuthChallenge,
    ) -> Result<String, AppError>;
    async fn exchange(
        &self,
        code: &SecretString,
        verifier: &SecretString,
        redirect_uri: &str,
    ) -> Result<CloudDriveCredential, AppError>;
    async fn refresh(&self, refresh_token: &SecretString)
    -> Result<CloudDriveCredential, AppError>;
}

#[async_trait]
impl OAuthTokensPort for GoogleOAuthTokens {
    fn ensure_configured(&self) -> Result<(), AppError> {
        GoogleOAuthTokens::ensure_configured(self)
    }
    fn authorization_url(
        &self,
        redirect_uri: &str,
        challenge: &OAuthChallenge,
    ) -> Result<String, AppError> {
        GoogleOAuthTokens::authorization_url(self, redirect_uri, challenge)
    }
    async fn exchange(
        &self,
        code: &SecretString,
        verifier: &SecretString,
        redirect_uri: &str,
    ) -> Result<CloudDriveCredential, AppError> {
        GoogleOAuthTokens::exchange(self, code, verifier, redirect_uri).await
    }
    async fn refresh(
        &self,
        refresh_token: &SecretString,
    ) -> Result<CloudDriveCredential, AppError> {
        GoogleOAuthTokens::refresh(self, refresh_token).await
    }
}

struct AttemptState {
    seq: u64,
    record: CloudDriveAuthAttempt,
    status: OAuthStatus,
    credential: Option<CloudDriveCredential>,
    task: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct AuthInner {
    attempts: HashMap<String, AttemptState>,
    next_seq: u64,
}

/// Google Drive 只读授权的桌面运行时；实现 [`CloudDriveAuthPort`]。
pub struct GoogleDriveAuth {
    tokens: Arc<dyn OAuthTokensPort>,
    browser: Arc<dyn CloudOAuthBrowserPort>,
    ttl_ms: i64,
    starting: AtomicBool,
    inner: Arc<Mutex<AuthInner>>,
}

impl GoogleDriveAuth {
    /// 生产入口：由 Tauri 组合根注入令牌端点与系统浏览器端口。
    pub(crate) fn new(
        tokens: Arc<GoogleOAuthTokens>,
        browser: Arc<dyn CloudOAuthBrowserPort>,
    ) -> Self {
        Self::with_runtime(tokens, browser, AUTH_TTL_MS)
    }

    fn with_runtime(
        tokens: Arc<dyn OAuthTokensPort>,
        browser: Arc<dyn CloudOAuthBrowserPort>,
        ttl_ms: i64,
    ) -> Self {
        Self {
            tokens,
            browser,
            ttl_ms,
            starting: AtomicBool::new(false),
            inner: Arc::new(Mutex::new(AuthInner::default())),
        }
    }

    fn lock(&self) -> MutexGuard<'_, AuthInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 单飞预留：同一时刻最多一个未终结尝试；失败不得改动既有状态。
    fn reserve(&self) -> Result<StartReservation<'_>, AppError> {
        let mut inner = self.lock();
        expire_due(&mut inner, UtcMillis::now());
        if inner.attempts.values().any(|attempt| {
            matches!(
                attempt.status,
                OAuthStatus::Pending | OAuthStatus::Authorized
            )
        }) {
            return Err(already_active());
        }
        if self.starting.swap(true, Ordering::SeqCst) {
            return Err(already_active());
        }
        Ok(StartReservation(&self.starting))
    }
}

impl Drop for GoogleDriveAuth {
    /// 中止全部监听任务：任务被丢弃即关闭对应 loopback 端口。
    fn drop(&mut self) {
        let mut inner = self.lock();
        for attempt in inner.attempts.values_mut() {
            attempt.credential = None;
            if let Some(task) = attempt.task.take() {
                task.abort();
            }
        }
    }
}

/// 预留守卫：无论 `start` 从哪条路径退出，都释放单飞标志。
struct StartReservation<'a>(&'a AtomicBool);

impl Drop for StartReservation<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// 总时限截止时刻；`>=` 即过期，与 [`CloudDriveAuthAttempt::is_expired_at`] 一致。
fn deadline_for(now: UtcMillis, ttl_ms: i64) -> UtcMillis {
    UtcMillis(now.0.saturating_add(ttl_ms))
}

fn validate_attempt_id(raw: &str) -> Result<String, AppError> {
    if raw.len() != 36 || Uuid::parse_str(raw).is_err() {
        return Err(attempt_id_invalid());
    }
    Ok(raw.to_owned())
}

/// 到点即过期：清除秘密结果并中止监听任务；只影响 Pending / Authorized。
fn expire_due(inner: &mut AuthInner, now: UtcMillis) {
    let mut retired = Vec::new();
    for attempt in inner.attempts.values_mut() {
        let live = matches!(
            attempt.status,
            OAuthStatus::Pending | OAuthStatus::Authorized
        );
        if live && attempt.record.is_expired_at(now) {
            attempt.status = OAuthStatus::Expired;
            attempt.credential = None;
            if let Some(task) = attempt.task.take() {
                retired.push(task);
            }
        }
    }
    for task in retired {
        task.abort();
    }
}

/// 终态有界保留：超出上限时回收最早的终态尝试。
fn prune_terminal(inner: &mut AuthInner) {
    let mut terminal: Vec<(u64, String)> = inner
        .attempts
        .iter()
        .filter(|(_, attempt)| attempt.status != OAuthStatus::Pending)
        .map(|(id, attempt)| (attempt.seq, id.clone()))
        .collect();
    if terminal.len() <= MAX_RETAINED_ATTEMPTS {
        return;
    }
    terminal.sort_unstable_by_key(|(seq, _)| *seq);
    let excess = terminal.len() - MAX_RETAINED_ATTEMPTS;
    for (_, id) in terminal.into_iter().take(excess) {
        if let Some(mut attempt) = inner.attempts.remove(&id) {
            if let Some(task) = attempt.task.take() {
                task.abort();
            }
        }
    }
}

#[async_trait]
impl CloudDriveAuthPort for GoogleDriveAuth {
    fn is_configured(&self) -> bool {
        self.tokens.ensure_configured().is_ok()
    }
    async fn start(&self, scopes: &[String]) -> Result<CloudDriveAuthAttempt, AppError> {
        if scopes.len() > 1
            || scopes
                .iter()
                .any(|scope| scope != GOOGLE_DRIVE_READONLY_SCOPE)
        {
            return Err(scope_rejected());
        }
        self.tokens.ensure_configured()?;
        let reservation = self.reserve()?;
        let expires_at = deadline_for(UtcMillis::now(), self.ttl_ms);

        let challenge = OAuthChallenge::generate();
        let listener =
            tokio::time::timeout(REQUEST_READ_TIMEOUT, TcpListener::bind(("127.0.0.1", 0)))
                .await
                .map_err(|_| listener_failed("云盘授权初始化超时"))?
                .map_err(|_| listener_failed("无法启动云盘授权回调监听"))?;
        let port = listener
            .local_addr()
            .map_err(|_| listener_failed("无法读取云盘授权回调端口"))?
            .port();
        let redirect_uri = loopback_redirect_uri(port)?;
        let url = self.tokens.authorization_url(&redirect_uri, &challenge)?;
        // 浏览器失败发生在注册尝试之前：listener 仍是局部变量，错误返回即关闭端口。
        tokio::time::timeout(
            REQUEST_READ_TIMEOUT,
            self.browser.open_google_authorization(&url),
        )
        .await
        .map_err(|_| listener_failed("打开授权浏览器超时"))??;

        let id = Uuid::new_v4().to_string();
        let record = CloudDriveAuthAttempt {
            id: id.clone(),
            expires_at,
        };
        {
            let mut inner = self.lock();
            inner.next_seq += 1;
            let seq = inner.next_seq;
            inner.attempts.insert(
                id.clone(),
                AttemptState {
                    seq,
                    record: record.clone(),
                    status: OAuthStatus::Pending,
                    credential: None,
                    task: None,
                },
            );
            prune_terminal(&mut inner);
        }
        let server = CallbackServer {
            inner: Arc::clone(&self.inner),
            tokens: Arc::clone(&self.tokens),
            attempt_id: id.clone(),
            challenge,
            redirect_uri,
            port,
        };
        let owned_inner = Arc::clone(&self.inner);
        let owned_id = id.clone();
        let budget =
            Duration::from_millis(expires_at.0.saturating_sub(UtcMillis::now().0).max(0) as u64);
        let deadline = tokio::time::Instant::now() + budget;
        let task = tokio::spawn(async move {
            let _ = tokio::time::timeout_at(deadline, server.run(listener)).await;
            let live = {
                let inner = owned_inner.lock().unwrap_or_else(|e| e.into_inner());
                inner.attempts.get(&owned_id).is_some_and(|a| {
                    matches!(a.status, OAuthStatus::Pending | OAuthStatus::Authorized)
                })
            };
            if live {
                tokio::time::sleep_until(deadline).await;
                let mut inner = owned_inner.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(a) = inner.attempts.get_mut(&owned_id) {
                    if matches!(a.status, OAuthStatus::Pending | OAuthStatus::Authorized) {
                        a.status = OAuthStatus::Expired;
                        a.credential = None;
                    }
                }
            }
        });
        {
            let mut inner = self.lock();
            match inner.attempts.get_mut(&id) {
                Some(attempt) => attempt.task = Some(task),
                None => task.abort(),
            }
        }
        drop(reservation);
        Ok(record)
    }

    async fn poll(&self, attempt_id: &str) -> Result<OAuthStatus, AppError> {
        let id = validate_attempt_id(attempt_id)?;
        let mut inner = self.lock();
        expire_due(&mut inner, UtcMillis::now());
        inner
            .attempts
            .get(&id)
            .map(|attempt| attempt.status)
            .ok_or_else(attempt_not_found)
    }

    async fn cancel(&self, attempt_id: &str) -> Result<(), AppError> {
        let id = validate_attempt_id(attempt_id)?;
        let task = {
            let mut inner = self.lock();
            let attempt = inner.attempts.get_mut(&id).ok_or_else(attempt_not_found)?;
            if matches!(
                attempt.status,
                OAuthStatus::Pending | OAuthStatus::Authorized
            ) {
                attempt.status = OAuthStatus::Cancelled;
            }
            attempt.credential = None;
            attempt.task.take()
        };
        if let Some(task) = task {
            task.abort();
        }
        Ok(())
    }

    async fn take_result(
        &self,
        attempt_id: &str,
    ) -> Result<Option<CloudDriveCredential>, AppError> {
        let id = validate_attempt_id(attempt_id)?;
        let mut inner = self.lock();
        expire_due(&mut inner, UtcMillis::now());
        let attempt = inner.attempts.get_mut(&id).ok_or_else(attempt_not_found)?;
        if attempt.status != OAuthStatus::Authorized {
            return Ok(None);
        }
        attempt.status = OAuthStatus::Consumed;
        Ok(attempt.credential.take())
    }

    async fn refresh(
        &self,
        refresh_token: &SecretString,
    ) -> Result<CloudDriveCredential, AppError> {
        self.tokens.refresh(refresh_token).await
    }
}

/// 单次尝试的回调监听器；只服务到本次尝试终结或到点为止。
struct CallbackServer {
    inner: Arc<Mutex<AuthInner>>,
    tokens: Arc<dyn OAuthTokensPort>,
    attempt_id: String,
    challenge: OAuthChallenge,
    redirect_uri: String,
    port: u16,
}

impl CallbackServer {
    fn lock(&self) -> MutexGuard<'_, AuthInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn deadline(&self) -> Option<UtcMillis> {
        self.lock()
            .attempts
            .get(&self.attempt_id)
            .map(|attempt| attempt.record.expires_at)
    }

    fn is_pending(&self) -> bool {
        self.lock()
            .attempts
            .get(&self.attempt_id)
            .is_some_and(|attempt| attempt.status == OAuthStatus::Pending)
    }

    /// 只在尝试仍是 Pending 且未到点时落结果；取消 / 过期后的迟到结果被丢弃（Drop 清零）。
    fn settle(&self, status: OAuthStatus, credential: Option<CloudDriveCredential>) {
        let now = UtcMillis::now();
        let mut inner = self.lock();
        let Some(attempt) = inner.attempts.get_mut(&self.attempt_id) else {
            return;
        };
        if attempt.status != OAuthStatus::Pending {
            return;
        }
        if attempt.record.is_expired_at(now) {
            attempt.status = OAuthStatus::Expired;
            attempt.credential = None;
            return;
        }
        attempt.status = status;
        attempt.credential = credential;
    }

    /// 只服务到终结或到点；任何分支都不记录回调 URL / query。
    async fn run(self, listener: TcpListener) {
        loop {
            let Some(deadline) = self.deadline() else {
                return;
            };
            if !self.is_pending() {
                return;
            }
            let remaining_ms = deadline.0.saturating_sub(UtcMillis::now().0);
            if remaining_ms <= 0 {
                self.settle(OAuthStatus::Expired, None);
                return;
            }
            match tokio::time::timeout(
                Duration::from_millis(remaining_ms as u64),
                listener.accept(),
            )
            .await
            {
                // 到点：关闭监听并清除秘密结果。
                Err(_) => {
                    self.settle(OAuthStatus::Expired, None);
                    return;
                }
                Ok(Ok((stream, _peer))) => {
                    if self.handle(stream).await {
                        return;
                    }
                }
                // 监听错误不改变时限；短暂让出，避免空转。
                Ok(Err(_)) => tokio::time::sleep(Duration::from_millis(5)).await,
            }
        }
    }

    /// 返回 `true` 表示本次尝试已终结，监听循环应当结束。
    async fn handle(&self, mut stream: TcpStream) -> bool {
        let Some(head) = read_head(&mut stream).await else {
            // 读取失败 / 超限 / 超时：静默丢弃，既不消费尝试也不延长时限。
            return false;
        };
        let head = zeroize::Zeroizing::new(head);
        match evaluate_head(&head, self.port) {
            HeadDecision::Callback(query) => self.dispatch(&mut stream, &query).await,
            HeadDecision::Ignore(status) => {
                let _ = write_response(&mut stream, status, UNRELATED_BODY).await;
                false
            }
            HeadDecision::Drop => false,
        }
    }

    async fn dispatch(&self, stream: &mut TcpStream, query: &str) -> bool {
        match self.challenge.parse_callback(query) {
            Ok(CallbackOutcome::Denied) => {
                let _ = write_response(stream, 200, DENIED_BODY).await;
                self.settle(OAuthStatus::Cancelled, None);
                true
            }
            Ok(CallbackOutcome::Code(code)) => {
                let _ = write_response(stream, 200, GRANTED_BODY).await;
                self.exchange(code).await;
                true
            }
            Err(error) if error.code().as_str() == PROVIDER_REJECTED_CODE => {
                let _ = write_response(stream, 200, FAILED_BODY).await;
                self.settle(OAuthStatus::Failed, None);
                true
            }
            // state 不匹配、重复参数或非法编码：与本次尝试无关，不能消费或延长它。
            Err(_) => {
                let _ = write_response(stream, 400, UNRELATED_BODY).await;
                false
            }
        }
    }

    /// 在剩余时限内换取令牌；超时与失败都是终态，迟到的成功不会复活已取消 / 过期的尝试。
    async fn exchange(&self, code: SecretString) {
        let remaining_ms = self
            .deadline()
            .map_or(0, |deadline| deadline.0.saturating_sub(UtcMillis::now().0));
        if remaining_ms <= 0 {
            self.settle(OAuthStatus::Expired, None);
            return;
        }
        let exchange =
            self.tokens
                .exchange(&code, &self.challenge.code_verifier, &self.redirect_uri);
        match tokio::time::timeout(Duration::from_millis(remaining_ms as u64), exchange).await {
            Ok(Ok(credential)) => self.settle(OAuthStatus::Authorized, Some(credential)),
            Ok(Err(_)) => self.settle(OAuthStatus::Failed, None),
            Err(_) => self.settle(OAuthStatus::Expired, None),
        }
    }
}

#[path = "callback.rs"]
mod callback;
use callback::{HeadDecision, evaluate_head, read_head, write_response};

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
