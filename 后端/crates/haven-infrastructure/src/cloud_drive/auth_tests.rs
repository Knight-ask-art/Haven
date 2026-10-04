use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Notify;

use super::*;

/// 进程内令牌端点：记录授权地址与 state；exchange 可被 gate 卡住。
struct FakeTokens {
    configured: bool,
    recorded: Mutex<Option<(String, String)>>,
    gate: Option<Arc<Notify>>,
    exchanges: Mutex<usize>,
}

impl FakeTokens {
    fn configured() -> Self {
        Self {
            configured: true,
            recorded: Mutex::new(None),
            gate: None,
            exchanges: Mutex::new(0),
        }
    }
    fn unconfigured() -> Self {
        Self {
            configured: false,
            ..Self::configured()
        }
    }
}

#[async_trait]
impl OAuthTokensPort for FakeTokens {
    fn ensure_configured(&self) -> Result<(), AppError> {
        if self.configured {
            Ok(())
        } else {
            Err(AppError::new(
                "CLOUD_OAUTH_NOT_CONFIGURED",
                ErrorKind::Unsupported,
                "未配置",
                false,
            ))
        }
    }
    fn authorization_url(
        &self,
        redirect_uri: &str,
        challenge: &OAuthChallenge,
    ) -> Result<String, AppError> {
        *self.recorded.lock().unwrap() =
            Some((redirect_uri.to_owned(), challenge.state.expose().to_owned()));
        Ok(format!(
            "https://accounts.google.com/o/oauth2/v2/auth?state={}",
            challenge.state.expose()
        ))
    }
    async fn exchange(
        &self,
        _code: &SecretString,
        _verifier: &SecretString,
        _redirect_uri: &str,
    ) -> Result<CloudDriveCredential, AppError> {
        if let Some(gate) = &self.gate {
            gate.notified().await;
        }
        *self.exchanges.lock().unwrap() += 1;
        Ok(credential())
    }
    async fn refresh(
        &self,
        _refresh_token: &SecretString,
    ) -> Result<CloudDriveCredential, AppError> {
        Ok(credential())
    }
}

struct FakeBrowser {
    fail: bool,
    opened: Mutex<usize>,
}

impl FakeBrowser {
    fn ok() -> Self {
        Self {
            fail: false,
            opened: Mutex::new(0),
        }
    }
    fn failing() -> Self {
        Self {
            fail: true,
            opened: Mutex::new(0),
        }
    }
}

#[async_trait]
impl CloudOAuthBrowserPort for FakeBrowser {
    async fn open_google_authorization(&self, _url: &str) -> Result<(), AppError> {
        if self.fail {
            return Err(AppError::new(
                "TEST_BROWSER_FAILED",
                ErrorKind::Io,
                "浏览器启动失败",
                true,
            ));
        }
        *self.opened.lock().unwrap() += 1;
        Ok(())
    }
}

fn credential() -> CloudDriveCredential {
    CloudDriveCredential {
        access_token: SecretString::new("access-token"),
        refresh_token: SecretString::new("refresh-token"),
        expires_at_ms: UtcMillis::now().0 + 3_600_000,
        scope: Some(GOOGLE_DRIVE_READONLY_SCOPE.to_owned()),
    }
}

async fn start_basic(ttl_ms: i64) -> (GoogleDriveAuth, Arc<FakeTokens>, CloudDriveAuthAttempt) {
    let tokens = Arc::new(FakeTokens::configured());
    let auth = GoogleDriveAuth::with_runtime(tokens.clone(), Arc::new(FakeBrowser::ok()), ttl_ms);
    let attempt = auth.start(&[]).await.expect("start");
    (auth, tokens, attempt)
}

/// 从记录的授权地址取出 loopback 端口与本次 state。
fn endpoint(tokens: &FakeTokens) -> (u16, String) {
    let (redirect, state) = tokens
        .recorded
        .lock()
        .unwrap()
        .clone()
        .expect("authorization_url");
    let rest = redirect
        .strip_prefix("http://127.0.0.1:")
        .expect("loopback redirect");
    (
        rest.split('/').next().unwrap().parse().expect("port"),
        state,
    )
}

fn callback_request(port: u16, query: &str) -> String {
    format!("GET /oauth2/callback?{query} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n")
}

/// 只在本机回环发起一次原始 HTTP 请求；写失败按空响应处理。
async fn send(port: u16, request: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    let _ = stream.write_all(request.as_bytes()).await;
    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response).await;
    String::from_utf8_lossy(&response).into_owned()
}

async fn status(auth: &GoogleDriveAuth, id: &str) -> OAuthStatus {
    auth.poll(id).await.expect("poll")
}

async fn wait_for(auth: &GoogleDriveAuth, id: &str, want: OAuthStatus) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let current = status(auth, id).await;
        if current == want {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "等待 {want:?} 超时，当前 {current:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn missing_configuration_fails_before_listener_or_browser() {
    let browser = Arc::new(FakeBrowser::ok());
    let auth = GoogleDriveAuth::with_runtime(
        Arc::new(FakeTokens::unconfigured()),
        browser.clone(),
        AUTH_TTL_MS,
    );
    let error = auth.start(&[]).await.expect_err("未配置应失败");
    assert_eq!(error.code().as_str(), "CLOUD_OAUTH_NOT_CONFIGURED");
    assert_eq!(*browser.opened.lock().unwrap(), 0, "未配置不应打开浏览器");
}

#[tokio::test]
async fn only_the_fixed_scope_is_accepted_and_starts_are_single_flight() {
    let auth = GoogleDriveAuth::with_runtime(
        Arc::new(FakeTokens::configured()),
        Arc::new(FakeBrowser::ok()),
        AUTH_TTL_MS,
    );
    let rejected = auth
        .start(&["https://www.googleapis.com/auth/drive".to_owned()])
        .await
        .expect_err("任意 scope 应被拒绝");
    assert_eq!(rejected.code().as_str(), "CLOUD_OAUTH_SCOPE_REJECTED");
    auth.start(&[GOOGLE_DRIVE_READONLY_SCOPE.to_owned()])
        .await
        .expect("固定只读 scope 应被接受");
    let overlapping = auth.start(&[]).await.expect_err("并发 start 应被拒绝");
    assert_eq!(overlapping.code().as_str(), "CLOUD_OAUTH_ATTEMPT_ACTIVE");
}

#[tokio::test]
async fn browser_failure_releases_the_single_flight_reservation() {
    let auth = GoogleDriveAuth::with_runtime(
        Arc::new(FakeTokens::configured()),
        Arc::new(FakeBrowser::failing()),
        AUTH_TTL_MS,
    );
    assert_eq!(
        auth.start(&[]).await.unwrap_err().code().as_str(),
        "TEST_BROWSER_FAILED"
    );
    // 预留必须已释放：第二次失败仍是浏览器错误，而不是「已有进行中的授权」。
    assert_eq!(
        auth.start(&[]).await.unwrap_err().code().as_str(),
        "TEST_BROWSER_FAILED"
    );
}

#[tokio::test]
async fn invalid_callback_does_not_consume_the_authentic_attempt() {
    let (auth, tokens, attempt) = start_basic(AUTH_TTL_MS).await;
    let (port, state) = endpoint(&tokens);
    let unrelated = send(port, &callback_request(port, "code=x&state=WRONG")).await;
    assert!(unrelated.starts_with("HTTP/1.1 400"), "{unrelated}");
    assert_eq!(
        status(&auth, &attempt.id).await,
        OAuthStatus::Pending,
        "无关回调不得消费尝试"
    );

    let granted = send(
        port,
        &callback_request(port, &format!("code=good&state={state}&scope=extra")),
    )
    .await;
    assert!(granted.starts_with("HTTP/1.1 200"), "{granted}");
    wait_for(&auth, &attempt.id, OAuthStatus::Authorized).await;
    let credential = auth
        .take_result(&attempt.id)
        .await
        .expect("take")
        .expect("授权结果");
    assert_eq!(credential.access_token.expose(), "access-token");
    // 一次性：第二次拿不到东西，状态落到 Consumed。
    assert!(
        auth.take_result(&attempt.id)
            .await
            .expect("second take")
            .is_none()
    );
    assert_eq!(status(&auth, &attempt.id).await, OAuthStatus::Consumed);
}

#[tokio::test]
async fn denied_callback_cancels_and_leaves_no_result() {
    let (auth, tokens, attempt) = start_basic(AUTH_TTL_MS).await;
    let (port, state) = endpoint(&tokens);
    let response = send(
        port,
        &callback_request(port, &format!("error=access_denied&state={state}")),
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    wait_for(&auth, &attempt.id, OAuthStatus::Cancelled).await;
    assert!(auth.take_result(&attempt.id).await.expect("take").is_none());
}

#[tokio::test]
async fn cancel_clears_the_secret_and_ids_are_validated() {
    let (auth, _tokens, attempt) = start_basic(AUTH_TTL_MS).await;
    assert_eq!(
        auth.poll("not-a-uuid").await.unwrap_err().code().as_str(),
        "CLOUD_OAUTH_ATTEMPT_ID_INVALID"
    );
    assert_eq!(
        auth.cancel("00000000-0000-4000-8000-000000000000")
            .await
            .unwrap_err()
            .code()
            .as_str(),
        "CLOUD_OAUTH_ATTEMPT_NOT_FOUND"
    );
    let refreshed = auth
        .refresh(&SecretString::new("refresh-token"))
        .await
        .expect("refresh");
    assert_eq!(refreshed.refresh_token.expose(), "refresh-token");
    auth.cancel(&attempt.id).await.expect("cancel");
    assert_eq!(status(&auth, &attempt.id).await, OAuthStatus::Cancelled);
    assert!(auth.take_result(&attempt.id).await.expect("take").is_none());
}

#[tokio::test]
async fn deadline_is_exact_and_expiry_clears_the_result() {
    let exact = CloudDriveAuthAttempt {
        id: "x".into(),
        expires_at: deadline_for(UtcMillis(1_000), AUTH_TTL_MS),
    };
    assert_eq!(exact.expires_at, UtcMillis(1_000 + 300_000));
    assert!(!exact.is_expired_at(UtcMillis(1_000 + 299_999)));
    assert!(
        exact.is_expired_at(UtcMillis(1_000 + 300_000)),
        "到点即过期"
    );

    let (auth, _tokens, attempt) = start_basic(120).await;
    tokio::time::sleep(Duration::from_millis(220)).await;
    assert_eq!(status(&auth, &attempt.id).await, OAuthStatus::Expired);
    assert!(auth.take_result(&attempt.id).await.expect("take").is_none());
}

#[tokio::test]
async fn cancellation_during_exchange_cannot_resurrect_a_credential() {
    let gate = Arc::new(Notify::new());
    let tokens = Arc::new(FakeTokens {
        gate: Some(Arc::clone(&gate)),
        ..FakeTokens::configured()
    });
    let auth =
        GoogleDriveAuth::with_runtime(tokens.clone(), Arc::new(FakeBrowser::ok()), AUTH_TTL_MS);
    let attempt = auth.start(&[]).await.expect("start");
    let (port, state) = endpoint(&tokens);
    let response = send(
        port,
        &callback_request(port, &format!("code=good&state={state}")),
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    // 交换仍被 gate 挡住时取消；即使交换随后完成，也不得写入凭据。
    auth.cancel(&attempt.id).await.expect("cancel");
    gate.notify_waiters();
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(status(&auth, &attempt.id).await, OAuthStatus::Cancelled);
    assert!(auth.take_result(&attempt.id).await.expect("take").is_none());
}

#[tokio::test]
async fn hostile_and_unrelated_requests_never_touch_the_attempt() {
    let (auth, tokens, attempt) = start_basic(AUTH_TTL_MS).await;
    let (port, state) = endpoint(&tokens);
    let query = format!("code=x&state={state}");
    let not_found = format!("GET /other HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
    let wrong_method =
        format!("POST /oauth2/callback?{query} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
    assert!(send(port, &not_found).await.starts_with("HTTP/1.1 404"));
    assert!(send(port, &wrong_method).await.starts_with("HTTP/1.1 405"));
    // 形状违例静默关闭：重复 Host、framing 头、错误 Host、超长请求。
    let dropped = [
        format!(
            "GET /oauth2/callback?{query} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nHost: 127.0.0.1:{port}\r\n\r\n"
        ),
        format!(
            "GET /oauth2/callback?{query} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\n\r\n"
        ),
        format!(
            "GET /oauth2/callback?{query} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nTransfer-Encoding: chunked\r\n\r\n"
        ),
        format!("GET /oauth2/callback?{query} HTTP/1.1\r\nHost: 127.0.0.1:1\r\n\r\n"),
        format!(
            "GET /oauth2/callback?{query} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Pad: {}\r\n\r\n",
            "A".repeat(MAX_REQUEST_BYTES)
        ),
    ];
    for request in dropped {
        assert!(
            send(port, &request).await.is_empty(),
            "畸形请求不应收到响应"
        );
    }
    assert_eq!(status(&auth, &attempt.id).await, OAuthStatus::Pending);
}
