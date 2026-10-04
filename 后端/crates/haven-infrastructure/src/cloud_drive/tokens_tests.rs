use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;

use super::*;

/// 记录 / 断言用的哨兵：任何错误或 Debug 输出里都不允许出现它。
const SENTINEL: &str = "SENTINEL-token-value";
const CLIENT_ID: &str = "1234567890-abc.apps.googleusercontent.com";
const REDIRECT: &str = "http://127.0.0.1:51234/oauth2/callback";

/// 进程内 fake：记录完整请求（含 SecretString 表单），按队列回放响应。
#[derive(Default)]
struct Fake {
    seen: Mutex<Vec<GoogleRequest>>,
    queue: Mutex<VecDeque<GoogleResponse>>,
}

#[async_trait]
impl GoogleTransport for Fake {
    async fn send(&self, request: GoogleRequest) -> Result<GoogleResponse, AppError> {
        self.seen.lock().unwrap().push(request);
        Ok(self
            .queue
            .lock()
            .unwrap()
            .pop_front()
            .expect("fake 响应已耗尽"))
    }
}

fn client(
    client_id: Option<&str>,
    client_secret: Option<&str>,
    responses: Vec<GoogleResponse>,
) -> (Arc<Fake>, GoogleOAuthTokens) {
    let fake = Arc::new(Fake {
        queue: Mutex::new(responses.into()),
        ..Fake::default()
    });
    let secret = client_secret.map(SecretString::new);
    (
        Arc::clone(&fake),
        GoogleOAuthTokens::new(client_id.map(str::to_owned), secret, fake),
    )
}

fn json(body: &str) -> GoogleResponse {
    GoogleResponse {
        status: 200,
        bytes: body.into(),
        content_type: Some("application/json".into()),
        content_range: None,
        accept_ranges: false,
    }
}

fn status_response(status: u16, body: &str) -> GoogleResponse {
    GoogleResponse {
        status,
        ..json(body)
    }
}

fn ok(access: &str, refresh: Option<&str>, extra: &str) -> GoogleResponse {
    let refresh = refresh.map_or_else(String::new, |value| {
        format!(r#","refresh_token":"{value}""#)
    });
    json(&format!(
        r#"{{"access_token":"{access}","token_type":"Bearer","expires_in":3599{refresh}{extra}}}"#
    ))
}

fn names(request: &GoogleRequest) -> Vec<&str> {
    request.form.iter().map(|(name, _)| name.as_str()).collect()
}

fn value<'a>(request: &'a GoogleRequest, name: &str) -> Option<&'a str> {
    request
        .form
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.expose())
}

fn code(error: &AppError) -> &str {
    error.code().0.as_str()
}

/// 固定授权码 / verifier 的 exchange，便于只关注被测行为。
async fn exchange(
    oauth: &GoogleOAuthTokens,
    redirect: &str,
) -> Result<CloudDriveCredential, AppError> {
    let (code, verifier) = (
        SecretString::new("auth-code"),
        SecretString::new("v".repeat(64)),
    );
    oauth.exchange(&code, &verifier, redirect).await
}

#[tokio::test]
async fn exchange_posts_the_exact_form_and_derives_expiry() {
    let (fake, oauth) = client(
        Some(CLIENT_ID),
        Some("client-secret"),
        vec![ok(SENTINEL, Some("refresh-1"), "")],
    );
    let before = UtcMillis::now().0;
    let granted = exchange(&oauth, REDIRECT).await.expect("交换应成功");
    let after = UtcMillis::now().0;

    let seen = fake.seen.lock().unwrap();
    let request = &seen[0];
    assert_eq!(
        names(request),
        [
            "client_id",
            "client_secret",
            "code",
            "code_verifier",
            "grant_type",
            "redirect_uri"
        ]
    );
    assert!(matches!(request.method, GoogleMethod::PostForm));
    assert!(request.bearer.is_none() && request.range.is_none());
    assert_eq!(request.max_bytes, JSON_LIMIT);
    assert_eq!(request.url.as_str(), TOKEN_ENDPOINT);
    assert_eq!(value(request, "client_id"), Some(CLIENT_ID));
    assert_eq!(value(request, "grant_type"), Some("authorization_code"));
    assert_eq!(value(request, "redirect_uri"), Some(REDIRECT));
    assert!(value(request, "code").is_some_and(|sent| sent == "auth-code"));
    assert!(value(request, "code_verifier").is_some_and(|sent| sent == "v".repeat(64)));
    assert!(value(request, "client_secret").is_some_and(|sent| sent == "client-secret"));
    assert!(
        granted.access_token.expose() == SENTINEL,
        "access token 应原样带回"
    );
    assert!(
        granted.refresh_token.expose() == "refresh-1",
        "refresh token 应原样带回"
    );
    assert_eq!(granted.scope, None);
    assert!((before + 3_599_000..=after + 3_599_000).contains(&granted.expires_at_ms));
}

#[tokio::test]
async fn client_secret_is_optional_but_validated_before_any_io() {
    let (fake, oauth) = client(
        Some(CLIENT_ID),
        None,
        vec![ok("access", Some("refresh"), "")],
    );
    exchange(&oauth, REDIRECT)
        .await
        .expect("缺少 client secret 也应成功");
    {
        let seen = fake.seen.lock().unwrap();
        assert_eq!(
            names(&seen[0]),
            [
                "client_id",
                "code",
                "code_verifier",
                "grant_type",
                "redirect_uri"
            ]
        );
        assert!(value(&seen[0], "client_secret").is_none());
    }

    for client_id in [None, Some(""), Some("has space")] {
        let (fake, oauth) = client(client_id, None, vec![]);
        let error = oauth
            .ensure_configured()
            .expect_err("非法 client id 应被拒绝");
        assert!(
            matches!(
                code(&error),
                "CLOUD_OAUTH_NOT_CONFIGURED" | "CLOUD_OAUTH_CONFIG_INVALID"
            ),
            "{client_id:?}"
        );
        assert!(fake.seen.lock().unwrap().is_empty());
    }
    for secret in ["", "has space", "非 ASCII"] {
        let (fake, oauth) = client(Some(CLIENT_ID), Some(secret), vec![]);
        let error = oauth
            .ensure_configured()
            .expect_err("非法 client secret 应被拒绝");
        assert_eq!(code(&error), "CLOUD_OAUTH_CONFIG_INVALID", "{secret:?}");
        assert!(fake.seen.lock().unwrap().is_empty());
    }
    let (_, oauth) = client(Some(CLIENT_ID), Some("GOCSPX-valid_secret"), vec![]);
    oauth.ensure_configured().expect("合法配置应通过");
}

#[tokio::test]
async fn refresh_reuses_the_stored_token_when_google_omits_it() {
    let (fake, oauth) = client(
        Some(CLIENT_ID),
        None,
        vec![
            ok("new-access", None, ""),
            ok("new-access-2", Some("rotated"), ""),
        ],
    );
    let stored = SecretString::new("stored-refresh");
    let kept = oauth.refresh(&stored).await.expect("刷新应成功");
    assert!(
        kept.access_token.expose() == "new-access",
        "刷新必须给出新 access token"
    );
    assert!(
        kept.refresh_token.expose() == "stored-refresh",
        "缺新 token 时必须回退到旧值"
    );
    let rotated = oauth.refresh(&stored).await.expect("轮换应成功");
    assert!(
        rotated.refresh_token.expose() == "rotated",
        "轮换时必须采用新 refresh token"
    );

    let seen = fake.seen.lock().unwrap();
    assert_eq!(
        names(&seen[0]),
        ["client_id", "grant_type", "refresh_token"]
    );
    assert_eq!(value(&seen[0], "client_id"), Some(CLIENT_ID));
    assert_eq!(value(&seen[0], "grant_type"), Some("refresh_token"));
    assert!(value(&seen[0], "refresh_token").is_some_and(|sent| sent == "stored-refresh"));
    assert!(value(&seen[1], "refresh_token").is_some_and(|sent| sent == "stored-refresh"));
}

#[tokio::test]
async fn exchange_requires_a_refresh_token() {
    let (_, oauth) = client(Some(CLIENT_ID), None, vec![ok("access", None, "")]);
    let error = exchange(&oauth, REDIRECT)
        .await
        .expect_err("缺少 refresh token 必须失败");
    assert_eq!(code(&error), "CLOUD_DRIVE_RESPONSE_INVALID");
    assert!(!error.retryable());
}

#[tokio::test]
async fn token_type_expiry_and_scope_are_strictly_bounded() {
    let scoped = format!(r#","scope":"{GOOGLE_DRIVE_READONLY_SCOPE}""#);
    let (_, oauth) = client(
        Some(CLIENT_ID),
        None,
        vec![ok("access", Some("refresh"), &scoped)],
    );
    let granted = exchange(&oauth, REDIRECT)
        .await
        .expect("固定 scope 必须通过");
    assert_eq!(granted.scope.as_deref(), Some(GOOGLE_DRIVE_READONLY_SCOPE));

    let payloads = vec![
            r#"{"access_token":"a","token_type":"MAC","expires_in":3599,"refresh_token":"r"}"#
                .to_owned(),
            r#"{"access_token":"a","expires_in":3599,"refresh_token":"r"}"#.to_owned(),
            r#"{"access_token":"a","token_type":"Bearer","refresh_token":"r"}"#.to_owned(),
            r#"{"access_token":"a","token_type":"Bearer","expires_in":0,"refresh_token":"r"}"#
                .to_owned(),
            r#"{"access_token":"a","token_type":"Bearer","expires_in":-1,"refresh_token":"r"}"#
                .to_owned(),
            r#"{"access_token":"a","token_type":"Bearer","expires_in":99999999,"refresh_token":"r"}"#
                .to_owned(),
            r#"{"access_token":"","token_type":"Bearer","expires_in":3599,"refresh_token":"r"}"#
                .to_owned(),
            r#"{"access_token":"a b","token_type":"Bearer","expires_in":3599,"refresh_token":"r"}"#
                .to_owned(),
            r#"{"access_token":"a\u0000b","token_type":"Bearer","expires_in":3599,"refresh_token":"r"}"#
                .to_owned(),
            r#"{"access_token":42,"token_type":"Bearer","expires_in":3599,"refresh_token":"r"}"#
                .to_owned(),
            format!(
                r#"{{"access_token":"a","token_type":"Bearer","expires_in":3599,"refresh_token":"{}"}}"#,
                "r".repeat(MAX_TOKEN_LEN + 1)
            ),
            format!(
                r#"{{"access_token":"a","token_type":"Bearer","expires_in":3599,"refresh_token":"r","scope":"{GOOGLE_DRIVE_READONLY_SCOPE} extra"}}"#
            ),
        ];
    for payload in payloads {
        let (_, oauth) = client(Some(CLIENT_ID), None, vec![json(&payload)]);
        let error = exchange(&oauth, REDIRECT)
            .await
            .expect_err("非法令牌响应必须失败");
        assert_eq!(code(&error), "CLOUD_DRIVE_RESPONSE_INVALID");
        assert!(!error.retryable());
    }
}

#[tokio::test]
async fn upstream_statuses_map_without_leaking_bodies() {
    let cases = [
        (
            400,
            r#"{"error":"invalid_grant","error_description":"SENTINEL-token-value"}"#.to_owned(),
            "CLOUD_DRIVE_UNAUTHORIZED",
            false,
        ),
        (
            401,
            r#"{"error":"invalid_grant"}"#.to_owned(),
            "CLOUD_DRIVE_UNAUTHORIZED",
            false,
        ),
        (
            400,
            r#"{"error":"invalid_client","error_description":"SENTINEL-token-value"}"#.to_owned(),
            "CLOUD_OAUTH_PROVIDER_REJECTED",
            false,
        ),
        (
            400,
            "not json SENTINEL-token-value".to_owned(),
            "CLOUD_OAUTH_PROVIDER_REJECTED",
            false,
        ),
        (
            403,
            r#"<html>SENTINEL-token-value</html>"#.to_owned(),
            "CLOUD_OAUTH_PROVIDER_REJECTED",
            false,
        ),
        (
            500,
            r#"{"error":"SENTINEL-token-value"}"#.to_owned(),
            "CLOUD_DRIVE_UNAVAILABLE",
            true,
        ),
        (
            429,
            "SENTINEL-token-value".to_owned(),
            "CLOUD_DRIVE_UNAVAILABLE",
            true,
        ),
    ];
    for (status, body, expected, retryable) in cases {
        let (_, oauth) = client(Some(CLIENT_ID), None, vec![status_response(status, &body)]);
        let error = oauth
            .refresh(&SecretString::new(SENTINEL))
            .await
            .expect_err("异常状态必须失败");
        assert_eq!(
            (code(&error), error.retryable()),
            (expected, retryable),
            "{status}"
        );
        let rendered = format!("{error:?} {error}");
        assert!(
            !rendered.contains(SENTINEL),
            "错误泄露了上游正文或令牌: {rendered}"
        );
    }
}

#[tokio::test]
async fn malformed_and_oversized_bodies_fail_closed() {
    let mut oversized = json("{}");
    oversized.bytes = format!(
        r#"{{"padding":"{}"}}"#,
        "x".repeat(MAX_TOKEN_RESPONSE_BYTES)
    )
    .into_bytes();
    let responses = [
        json("not json"),
        json("[]"),
        json(r#"{"access_token":{}}"#),
        oversized,
    ];
    for response in responses {
        let (_, oauth) = client(Some(CLIENT_ID), None, vec![response]);
        let error = exchange(&oauth, REDIRECT)
            .await
            .expect_err("畸形响应必须失败");
        assert_eq!(code(&error), "CLOUD_DRIVE_RESPONSE_INVALID");
        assert!(!error.retryable());
    }
}

#[tokio::test]
async fn redirect_uri_is_validated_before_any_io() {
    let (fake, oauth) = client(Some(CLIENT_ID), None, vec![]);
    for bad in [
        "https://127.0.0.1:51234/oauth2/callback",
        "http://localhost:51234/oauth2/callback",
        "http://127.0.0.1:51234/other",
        "http://127.0.0.1:0/oauth2/callback",
        "http://127.0.0.1:51234/oauth2/callback?code=x",
    ] {
        let error = exchange(&oauth, bad)
            .await
            .expect_err("非 loopback 回调必须失败");
        assert_eq!(code(&error), "CLOUD_OAUTH_CONFIG_INVALID", "{bad}");
    }
    assert!(
        fake.seen.lock().unwrap().is_empty(),
        "校验必须早于任何网络 IO"
    );
}

#[tokio::test]
async fn secrets_never_render_in_debug_output() {
    let (_, oauth) = client(
        Some(CLIENT_ID),
        Some(SENTINEL),
        vec![ok(SENTINEL, Some(SENTINEL), "")],
    );
    let granted = exchange(&oauth, REDIRECT).await.expect("交换应成功");
    let rendered = format!("{granted:?}");
    assert!(
        !rendered.contains(SENTINEL),
        "凭据 Debug 泄露了令牌: {rendered}"
    );
    assert!(rendered.contains("[REDACTED]"), "{rendered}");
}
