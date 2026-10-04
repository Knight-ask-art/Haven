//! `http.rs` 的离线测试：端点策略、Range 绑定、状态保留、上限、超时与脱敏。
//!
//! 全部用例都不需要真实网络：要么只调用纯函数，要么注入 fake 读流；唯一走真实传输的
//! 用例只喂**必然在校验阶段失败**的请求，因此不会触发任何 DNS 解析。

use std::collections::VecDeque;

use reqwest::header::{HeaderMap, HeaderValue};

use super::*;

fn url(raw: &str) -> Url {
    Url::parse(raw).expect("测试 URL")
}

fn drive_request(raw: &str) -> GoogleRequest {
    GoogleRequest {
        url: url(raw),
        method: GoogleMethod::Get,
        bearer: Some(SecretString::new("ya29.test-token")),
        form: Vec::new(),
        range: None,
        max_bytes: JSON_LIMIT,
    }
}

fn media_request(raw: &str) -> GoogleRequest {
    let mut request = drive_request(raw);
    request.max_bytes = MEDIA_LIMIT;
    request
}

fn token_request(raw: &str) -> GoogleRequest {
    GoogleRequest {
        url: url(raw),
        method: GoogleMethod::PostForm,
        bearer: None,
        form: vec![
            (
                "client_id".to_owned(),
                SecretString::new("1234567890-abc.apps.googleusercontent.com"),
            ),
            ("code".to_owned(), SecretString::new("4/0AeanS-test-code")),
            (
                "code_verifier".to_owned(),
                SecretString::new("verifier-test-value"),
            ),
            (
                "grant_type".to_owned(),
                SecretString::new("authorization_code"),
            ),
            (
                "redirect_uri".to_owned(),
                SecretString::new("http://127.0.0.1:51234/oauth2/callback"),
            ),
        ],
        range: None,
        max_bytes: JSON_LIMIT,
    }
}

fn head(status: u16, content_type: Option<&str>) -> ResponseHead {
    ResponseHead {
        status,
        content_type: content_type.map(str::to_owned),
        content_range: None,
        content_length: None,
        accept_ranges: false,
    }
}

struct FakeChunks {
    chunks: VecDeque<Vec<u8>>,
}

#[async_trait]
impl ChunkStream for FakeChunks {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, AppError> {
        Ok(self.chunks.pop_front())
    }
}

fn chunks(list: Vec<Vec<u8>>) -> FakeChunks {
    FakeChunks {
        chunks: list.into(),
    }
}

fn code(error: &AppError) -> &str {
    error.code().as_str()
}

#[test]
fn accepts_the_fixed_endpoint_surface() {
    for raw in [
        "https://www.googleapis.com/drive/v3/files",
        "https://www.googleapis.com/drive/v3/files?q=name%3D%27a%27&pageSize=100&fields=files(id)",
        "https://www.googleapis.com/drive/v3/files/1A2b3C-_x",
        "https://www.googleapis.com/drive/v3/files/root?alt=media",
        "https://www.googleapis.com/drive/v3/about?fields=user",
    ] {
        assert!(
            validate_request(&drive_request(raw)).is_ok(),
            "应接受: {raw}"
        );
    }
    // redirect_uri 必须在 token 表单白名单里，否则真实授权码交换会整条失败。
    let token = validate_request(&token_request("https://oauth2.googleapis.com/token"));
    assert!(token.is_ok());
    assert!(!token.expect("token 请求应通过").media);
}

#[test]
fn rejects_hosts_schemes_ports_and_paths_outside_the_fixed_surface() {
    for raw in [
        "http://www.googleapis.com/drive/v3/files",
        "https://www.googleapis.com:8443/drive/v3/files",
        "https://www.googleapis.com:80/drive/v3/files",
        "https://user:pw@www.googleapis.com/drive/v3/files",
        "https://www.googleapis.com/drive/v3/files#frag",
        "https://evil.example.invalid/drive/v3/files",
        "https://www.googleapis.com.evil.invalid/drive/v3/files",
        "https://drive.google.com/drive/v3/files",
        "https://www.googleapis.com/upload/drive/v3/files",
        "https://www.googleapis.com/drive/v3",
        "https://www.googleapis.com/drive/v3/",
        "https://www.googleapis.com/drive/v2/files",
        "https://www.googleapis.com/drive/v3/files/",
        "https://www.googleapis.com/drive/v3/files/abc/children",
        "https://www.googleapis.com/drive/v3/files/..%2Fabout",
        "https://www.googleapis.com/drive/v3/filesX",
        "https://www.googleapis.com/drive/v3/changes",
        "https://www.googleapis.com/drive/v3/about/extra",
        "https://oauth2.googleapis.com/token/",
        "https://www.googleapis.com/token",
        "https://www.googleapis.com/oauth2/v3/token",
    ] {
        assert!(
            validate_request(&drive_request(raw)).is_err()
                && validate_request(&token_request(raw)).is_err(),
            "应拒绝: {raw}"
        );
    }
}

#[test]
fn rejects_query_names_values_and_duplicates_outside_the_allowlist() {
    for raw in [
        "https://www.googleapis.com/drive/v3/files?unknown=1",
        "https://www.googleapis.com/drive/v3/files?alt=media&alt=json",
        "https://www.googleapis.com/drive/v3/files?alt=json&alt=json",
        "https://www.googleapis.com/drive/v3/files?",
        "https://www.googleapis.com/drive/v3/files?alt=raw",
        "https://www.googleapis.com/drive/v3/files?pageSize=0",
        "https://www.googleapis.com/drive/v3/files?pageSize=1001",
        "https://www.googleapis.com/drive/v3/files?pageSize=all",
        "https://www.googleapis.com/drive/v3/files?alt=",
    ] {
        assert!(
            validate_request(&drive_request(raw)).is_err(),
            "应拒绝: {raw}"
        );
    }
    let long_value = format!(
        "https://www.googleapis.com/drive/v3/files?pageToken={}",
        "a".repeat(2049)
    );
    assert!(validate_request(&drive_request(&long_value)).is_err());
    // 百分号解码发生在白名单比对之前：编码变体既不能绕过，也不会被误拒。
    assert!(
        validate_request(&drive_request(
            "https://www.googleapis.com/drive/v3/files/abc?alt=med%69a"
        ))
        .is_ok()
    );
    // `alt=media` 只对单个文件有意义，列表与 about 都不接受。
    for raw in [
        "https://www.googleapis.com/drive/v3/files?alt=media",
        "https://www.googleapis.com/drive/v3/about?alt=media",
    ] {
        assert!(
            validate_request(&drive_request(raw)).is_err(),
            "应拒绝: {raw}"
        );
    }
}

#[test]
fn binds_methods_and_forms_to_their_endpoint() {
    let mut get_on_token = token_request("https://oauth2.googleapis.com/token");
    get_on_token.method = GoogleMethod::Get;
    assert!(validate_request(&get_on_token).is_err());

    let mut post_on_drive = drive_request("https://www.googleapis.com/drive/v3/files");
    post_on_drive.method = GoogleMethod::PostForm;
    assert!(validate_request(&post_on_drive).is_err());

    let mut form_on_drive = drive_request("https://www.googleapis.com/drive/v3/files");
    form_on_drive
        .form
        .push(("code".to_owned(), SecretString::new("x")));
    assert!(validate_request(&form_on_drive).is_err());

    for raw in [
        "https://oauth2.googleapis.com/token?scope=x",
        "https://oauth2.googleapis.com/token/",
    ] {
        assert!(
            validate_request(&token_request(raw)).is_err(),
            "应拒绝: {raw}"
        );
    }

    let mut bearer_on_token = token_request("https://oauth2.googleapis.com/token");
    bearer_on_token.bearer = Some(SecretString::new("ya29.test-token"));
    assert!(validate_request(&bearer_on_token).is_err());

    let mut bare_token = token_request("https://oauth2.googleapis.com/token");
    bare_token.form.clear();
    assert!(validate_request(&bare_token).is_err());

    let mut unknown_field = token_request("https://oauth2.googleapis.com/token");
    unknown_field
        .form
        .push(("access_type".to_owned(), SecretString::new("offline")));
    assert!(validate_request(&unknown_field).is_err());

    let mut duplicated_field = token_request("https://oauth2.googleapis.com/token");
    duplicated_field
        .form
        .push(("code".to_owned(), SecretString::new("second")));
    assert!(validate_request(&duplicated_field).is_err());

    let mut ranged_token = token_request("https://oauth2.googleapis.com/token");
    ranged_token.range = Some(RemoteByteRange {
        start: 0,
        end: Some(16),
    });
    assert!(validate_request(&ranged_token).is_err());
}

#[test]
fn binds_range_to_media_downloads_and_bounds_the_window() {
    for raw in [
        "https://www.googleapis.com/drive/v3/files",
        "https://www.googleapis.com/drive/v3/files?q=name%3D%27a%27",
        "https://www.googleapis.com/drive/v3/files/1A2b3C-_x",
        "https://www.googleapis.com/drive/v3/about",
    ] {
        let mut request = drive_request(raw);
        request.range = Some(RemoteByteRange {
            start: 0,
            end: Some(1023),
        });
        assert!(
            validate_request(&request).is_err(),
            "应拒绝带 Range 的: {raw}"
        );
    }

    let mut media = media_request("https://www.googleapis.com/drive/v3/files/1A2b3C-_x?alt=media");
    media.range = Some(RemoteByteRange {
        start: 0,
        end: Some(1023),
    });
    let validated = validate_request(&media).expect("媒体下载应接受 Range");
    assert!(validated.media);
    assert_eq!(validated.range.as_deref(), Some("bytes=0-1023"));

    // 开口范围会被封闭成有界窗口。
    let mut open = media_request("https://www.googleapis.com/drive/v3/files/1A2b3C-_x?alt=media");
    open.range = Some(RemoteByteRange {
        start: 1024,
        end: None,
    });
    let validated = validate_request(&open).expect("开口范围应被封闭");
    assert_eq!(
        validated.range.as_deref(),
        Some(format!("bytes=1024-{}", 1024 + MEDIA_LIMIT as u64 - 1).as_str())
    );

    // 起止颠倒、窗口超限、起点溢出都拒绝。
    for range in [
        RemoteByteRange {
            start: 2048,
            end: Some(1024),
        },
        RemoteByteRange {
            start: 0,
            end: Some(MEDIA_LIMIT as u64),
        },
        RemoteByteRange {
            start: u64::MAX,
            end: None,
        },
        RemoteByteRange {
            start: 0,
            end: Some(u64::MAX),
        },
    ] {
        let mut request =
            media_request("https://www.googleapis.com/drive/v3/files/1A2b3C-_x?alt=media");
        request.range = Some(range);
        assert!(validate_request(&request).is_err(), "非法区间必须被拒绝");
    }
    // 恰好等于媒体上限的窗口是合法的。
    let mut exact = media_request("https://www.googleapis.com/drive/v3/files/1A2b3C-_x?alt=media");
    exact.range = Some(RemoteByteRange {
        start: 0,
        end: Some(MEDIA_LIMIT as u64 - 1),
    });
    assert!(validate_request(&exact).is_ok());
}

#[test]
fn rejects_a_zero_body_limit() {
    let mut request = drive_request("https://www.googleapis.com/drive/v3/files");
    request.max_bytes = 0;
    let error = validate_request(&request)
        .err()
        .expect("max_bytes=0 必须被拒绝");
    assert_eq!(code(&error), "VALIDATION");
}

#[tokio::test]
async fn rejects_malicious_credentials_before_any_io() {
    let transport = ReqwestGoogleTransport::new();
    for bad in [
        "",
        "ya29\r\nX-Evil: 1",
        "ya29\ntoken",
        "ya29 token",
        "ya29\u{7f}",
        "令牌",
    ] {
        let mut request = drive_request("https://www.googleapis.com/drive/v3/files");
        request.bearer = Some(SecretString::new(bad));
        let error = validate_request(&request)
            .err()
            .expect("非法凭据必须被拒绝");
        assert_eq!(code(&error), "SECURITY_POLICY_DENIED");
        assert!(!format!("{request:?}").contains("ya29"));
        // 真实传输在解析 DNS 之前就失败：这里没有网络，也不应挂起。
        let Err(error) = transport.send(request).await else {
            panic!("非法凭据必须被拒绝");
        };
        assert_eq!(code(&error), "SECURITY_POLICY_DENIED");
    }

    let mut form_crlf = token_request("https://oauth2.googleapis.com/token");
    form_crlf.form[1].1 = SecretString::new("code\r\nX-Evil: 1");
    assert!(validate_request(&form_crlf).is_err());

    let Err(error) = transport
        .send(drive_request("https://evil.example.invalid/drive/v3/files"))
        .await
    else {
        panic!("非白名单主机必须被拒绝");
    };
    assert_eq!(code(&error), "SECURITY_POLICY_DENIED");
    assert!(!format!("{error:?}").contains("example.invalid"));
}

#[test]
fn debug_output_hides_urls_queries_and_credentials() {
    let request = drive_request("https://www.googleapis.com/drive/v3/files?q=name%3D%27secret%27");
    let rendered = format!("{request:?}");
    for secret in ["ya29.test-token", "secret", "/drive/v3/"] {
        assert!(
            !rendered.contains(secret),
            "Debug 泄露了 {secret}: {rendered}"
        );
    }
    let rendered = format!("{:?}", token_request("https://oauth2.googleapis.com/token"));
    for secret in [
        "1234567890-abc.apps.googleusercontent.com",
        "verifier-test-value",
        "4/0AeanS-test-code",
    ] {
        assert!(
            !rendered.contains(secret),
            "Debug 泄露了 {secret}: {rendered}"
        );
    }
}

#[test]
fn caps_the_requested_limit_to_the_endpoint_ceiling() {
    assert_eq!(body_limit(usize::MAX, false), JSON_LIMIT);
    assert_eq!(body_limit(usize::MAX, true), MEDIA_LIMIT);
    assert_eq!(body_limit(1024, true), 1024);
    assert_eq!(body_limit(JSON_LIMIT, true), JSON_LIMIT);
}

#[tokio::test]
async fn rejects_redirects_html_successes_and_server_errors() {
    for (status, content_type) in [
        (301u16, Some("application/json")),
        (302, Some("text/html")),
        (200, Some("text/html; charset=utf-8")),
        (200, None),
        (200, Some("application/octet-stream")),
        (206, Some("application/json")),
        (500, Some("application/json")),
        (503, None),
    ] {
        let Err(error) = collect_bounded(
            JSON_LIMIT,
            false,
            head(status, content_type),
            &mut chunks(vec![]),
        )
        .await
        else {
            panic!("状态 {status} 的非预期响应必须被拒绝");
        };
        assert_eq!(code(&error), "SOURCE_UNAVAILABLE", "状态 {status}");
    }
}

#[tokio::test]
async fn preserves_http_error_status_for_the_adapter() {
    for status in [400u16, 401, 403, 404] {
        let body = br#"{"error":{"errors":[{"reason":"invalid_grant"}]}}"#.to_vec();
        let response = collect_bounded(
            JSON_LIMIT,
            false,
            head(status, Some("application/json; charset=UTF-8")),
            &mut chunks(vec![body.clone()]),
        )
        .await
        .expect("4xx 必须原样保留");
        assert_eq!(response.status, status);
        assert_eq!(response.bytes, body);
    }

    // 非 JSON 错误正文不读、留空；状态仍然保留。
    let mut stream = chunks(vec![b"<html>nope</html>".to_vec()]);
    let response = collect_bounded(JSON_LIMIT, false, head(401, Some("text/html")), &mut stream)
        .await
        .expect("HTML 错误正文不应让状态丢失");
    assert_eq!(response.status, 401);
    assert!(response.bytes.is_empty());
    assert_eq!(stream.chunks.len(), 1, "非 JSON 错误正文不应被读取");

    // 声明超限的 JSON 错误正文也只留空，状态照旧保留。
    let mut oversized = head(403, Some("application/json"));
    oversized.content_length = Some(JSON_LIMIT as u64 + 1);
    let response = collect_bounded(
        JSON_LIMIT,
        false,
        oversized,
        &mut chunks(vec![b"{}".to_vec()]),
    )
    .await
    .expect("超限的错误正文不应让状态丢失");
    assert_eq!(response.status, 403);
    assert!(response.bytes.is_empty());
}

#[tokio::test]
async fn stops_reading_a_body_that_exceeds_the_effective_limit() {
    // 12 MiB 的 chunked 正文：请求虽声明媒体上限，JSON 端点仍被夹到 4 MiB。
    let mut stream = chunks((0..12).map(|_| vec![b'x'; 1024 * 1024]).collect());
    let Err(error) = collect_bounded(
        MEDIA_LIMIT,
        false,
        head(200, Some("application/json")),
        &mut stream,
    )
    .await
    else {
        panic!("超过上限的正文必须失败");
    };
    assert_eq!(code(&error), "SOURCE_UNAVAILABLE");
    assert!(!stream.chunks.is_empty(), "超限后不应继续读取剩余正文");
}

#[tokio::test]
async fn rejects_a_declared_length_over_the_limit_without_reading() {
    let mut oversized = head(200, Some("application/json"));
    oversized.content_length = Some(JSON_LIMIT as u64 + 1);
    let mut stream = chunks(vec![b"{}".to_vec()]);
    let Err(error) = collect_bounded(JSON_LIMIT, false, oversized, &mut stream).await else {
        panic!("声明超限必须失败");
    };
    assert_eq!(code(&error), "SOURCE_UNAVAILABLE");
    assert_eq!(stream.chunks.len(), 1, "声明超限时不应读取正文");
}

#[tokio::test]
async fn returns_a_bounded_media_response_with_range_metadata() {
    let mut media_head = head(206, Some("application/pdf"));
    media_head.content_range = Some("bytes 0-6/7".to_owned());
    media_head.accept_ranges = true;
    let response = collect_bounded(
        MEDIA_LIMIT,
        true,
        media_head,
        &mut chunks(vec![b"PAYLOAD".to_vec()]),
    )
    .await
    .expect("合法媒体响应应成功");
    assert_eq!(response.status, 206);
    assert_eq!(response.bytes, b"PAYLOAD");
    assert_eq!(response.content_range.as_deref(), Some("bytes 0-6/7"));
    assert!(response.accept_ranges);
    assert_eq!(response.content_type.as_deref(), Some("application/pdf"));

    // 媒体端点不接受 JSON，也不会因为 200 就放行 HTML。
    for content_type in [Some("application/json"), Some("text/html"), None] {
        assert!(
            collect_bounded(
                MEDIA_LIMIT,
                true,
                head(200, content_type),
                &mut chunks(Vec::new())
            )
            .await
            .is_err(),
            "应拒绝: {content_type:?}"
        );
    }
}

#[test]
fn rejects_duplicate_or_oversized_response_headers() {
    let mut duplicated = HeaderMap::new();
    duplicated.append(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    duplicated.append(CONTENT_TYPE, HeaderValue::from_static("text/html"));
    assert!(bounded_header(&duplicated, CONTENT_TYPE).is_err());

    let mut duplicated_range = HeaderMap::new();
    duplicated_range.append(CONTENT_RANGE, HeaderValue::from_static("bytes 0-1/2"));
    duplicated_range.append(CONTENT_RANGE, HeaderValue::from_static("bytes 2-3/4"));
    assert!(bounded_header(&duplicated_range, CONTENT_RANGE).is_err());

    let mut oversized = HeaderMap::new();
    oversized.append(
        CONTENT_TYPE,
        HeaderValue::from_str(&format!("application/{}", "x".repeat(MAX_HEADER_BYTES)))
            .expect("合法头部值"),
    );
    assert!(bounded_header(&oversized, CONTENT_TYPE).is_err());

    let mut single = HeaderMap::new();
    single.append(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    assert_eq!(
        bounded_header(&single, CONTENT_TYPE)
            .expect("唯一头部应通过")
            .as_deref(),
        Some("application/json")
    );
    assert_eq!(
        bounded_header(&HeaderMap::new(), CONTENT_TYPE).expect("缺失头部"),
        None
    );
}

#[tokio::test]
async fn bounds_a_future_that_never_completes() {
    let error = with_deadline(
        std::future::pending::<Result<(), AppError>>(),
        Duration::from_millis(1),
    )
    .await
    .expect_err("永不完成的 future 必须被总超时截断");
    assert_eq!(code(&error), "SOURCE_TIMEOUT");
    assert!(error.retryable());
}

#[test]
fn maps_the_two_allowed_methods_to_http_verbs() {
    assert_eq!(method_of(GoogleMethod::Get), reqwest::Method::GET);
    assert_eq!(method_of(GoogleMethod::PostForm), reqwest::Method::POST);
}
