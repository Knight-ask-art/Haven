use std::collections::VecDeque;
use std::sync::Mutex;

use super::*;

const FOLDER: &str = r#"{"nextPageToken":"tok-1","files":[{"id":"folder-1","name":"漫画","mimeType":"application/vnd.google-apps.folder","parents":["root-abc"],"trashed":false},{"id":"pdf_1","name":"a.pdf","mimeType":"application/pdf","size":"13","parents":["folder-1"],"trashed":false}]}"#;
const ABOUT: &str = r#"{"user":{"permissionId":"0123456789","displayName":"张三"}}"#;
const PDF: &[u8] = b"%PDF-1.7 body";

#[derive(Default)]
struct Fake {
    seen: Mutex<Vec<(String, Option<RemoteByteRange>, usize)>>,
    queue: Mutex<VecDeque<GoogleResponse>>,
}

#[async_trait]
impl GoogleTransport for Fake {
    async fn send(&self, request: GoogleRequest) -> Result<GoogleResponse, AppError> {
        assert!(request.bearer.is_some() && matches!(request.method, GoogleMethod::Get));
        self.seen
            .lock()
            .unwrap()
            .push((request.url.to_string(), request.range, request.max_bytes));
        Ok(self
            .queue
            .lock()
            .unwrap()
            .pop_front()
            .expect("fake exhausted"))
    }
}

fn build_fake(responses: Vec<GoogleResponse>) -> (Arc<Fake>, GoogleDriveClient) {
    let fake = Arc::new(Fake {
        queue: Mutex::new(responses.into()),
        ..Fake::default()
    });
    (Arc::clone(&fake), GoogleDriveClient::new(fake))
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

fn meta(mime: &str, size: Option<u64>) -> GoogleResponse {
    let size = size.map_or_else(String::new, |size| format!(r#","size":"{size}""#));
    json(&format!(
        r#"{{"id":"pdf_1","name":"a.pdf","mimeType":"{mime}","trashed":false{size}}}"#
    ))
}

fn media(status: u16, body: &[u8], content_range: Option<&str>) -> GoogleResponse {
    GoogleResponse {
        status,
        bytes: body.to_vec(),
        content_type: Some(PDF_MIME.into()),
        content_range: content_range.map(str::to_owned),
        accept_ranges: true,
    }
}

fn cred(token: &str, ttl_ms: i64) -> CloudDriveCredential {
    CloudDriveCredential {
        access_token: SecretString::new(token),
        refresh_token: SecretString::new("r"),
        expires_at_ms: UtcMillis::now().0 + ttl_ms,
        scope: None,
    }
}

fn param(url: &str, key: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()?
        .query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

fn code(error: &AppError) -> &str {
    error.code().0.as_str()
}

#[tokio::test]
async fn list_folder_sends_a_fixed_query_and_bounds_entries() {
    let (fake, client) = build_fake(vec![json(FOLDER)]);
    let page = client
        .list_folder(&cred("t", 60_000), "folder-1", None)
        .await
        .expect("列表应成功");
    assert_eq!(page.entries.len(), 2);
    assert!(page.entries[0].is_folder);
    assert_eq!(page.entries[1].size_bytes, Some(13));
    assert_eq!(page.entries[1].parents, vec!["folder-1"]);
    assert_eq!(page.next_page_token.as_deref(), Some("tok-1"));
    let seen = fake.seen.lock().unwrap().clone();
    assert_eq!(seen[0].2, JSON_LIMIT);
    assert!(
        seen[0]
            .0
            .starts_with("https://www.googleapis.com/drive/v3/files?")
    );
    assert_eq!(
        param(&seen[0].0, "q").as_deref(),
        Some("'folder-1' in parents and trashed=false")
    );
    assert_eq!(param(&seen[0].0, "pageSize").as_deref(), Some(PAGE_SIZE));
    assert_eq!(param(&seen[0].0, "fields").as_deref(), Some(LIST_FIELDS));
}

#[tokio::test]
async fn list_folder_rejects_bad_tokens_and_malformed_pages() {
    let (fake, client) = build_fake(vec![]);
    let error = client
        .list_folder(&cred("t", 60_000), "folder-1", Some("bad token"))
        .await
        .expect_err("非法 token 应被拒绝");
    assert_eq!(code(&error), "VALIDATION");
    assert!(fake.seen.lock().unwrap().is_empty());

    let entries = (0..101)
        .map(|i| format!(r#"{{"id":"f{i}","name":"n","mimeType":"application/pdf"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    let payloads = [
        "not json".to_owned(),
        r#"{"files":[{"id":"../escape","name":"n","mimeType":"application/pdf"}]}"#.to_owned(),
        r#"{"files":[{"name":"n","mimeType":"application/pdf"}]}"#.to_owned(),
        format!(r#"{{"files":[{entries}]}}"#),
    ];
    for payload in payloads {
        let (_, client) = build_fake(vec![json(&payload)]);
        let error = client
            .list_folder(&cred("t", 60_000), "folder-1", None)
            .await
            .expect_err("畸形分页应失败");
        assert_eq!(code(&error), "CLOUD_DRIVE_RESPONSE_INVALID");
    }
}

#[tokio::test]
async fn stat_matches_ids_and_resolves_only_the_root_alias() {
    let (_, client) = build_fake(vec![meta(PDF_MIME, Some(13))]);
    assert_eq!(
        client
            .stat_file(&cred("t", 60_000), "pdf_1")
            .await
            .expect("stat 应成功")
            .id,
        "pdf_1"
    );

    let (_, client) = build_fake(vec![meta(PDF_MIME, Some(13))]);
    let error = client
        .stat_file(&cred("t", 60_000), "other")
        .await
        .expect_err("ID 不匹配应失败");
    assert_eq!(code(&error), "CLOUD_DRIVE_RESPONSE_INVALID");

    let root = json(
        r#"{"id":"root-abc","name":"我的云盘","mimeType":"application/vnd.google-apps.folder","trashed":false}"#,
    );
    let (_, client) = build_fake(vec![root]);
    assert_eq!(
        client
            .stat_file(&cred("t", 60_000), CLOUD_DRIVE_ROOT_ID)
            .await
            .expect("root 应解析")
            .id,
        "root-abc"
    );

    let (_, client) = build_fake(vec![meta(PDF_MIME, Some(13))]);
    let error = client
        .stat_file(&cred("t", 60_000), CLOUD_DRIVE_ROOT_ID)
        .await
        .expect_err("root 必须解析成文件夹");
    assert_eq!(code(&error), "CLOUD_DRIVE_RESPONSE_INVALID");
}

#[tokio::test]
async fn about_reads_the_permission_id_without_inventing_an_account() {
    let (fake, client) = build_fake(vec![json(ABOUT)]);
    let account = client
        .about(&cred("t", 60_000))
        .await
        .expect("about 应成功");
    assert_eq!(account.provider_account_id, "0123456789");
    assert_eq!(account.display_name, "张三");
    assert!(!format!("{account:?}").contains('@'), "不得带出邮箱");
    assert_eq!(
        param(&fake.seen.lock().unwrap()[0].0, "fields").as_deref(),
        Some(ABOUT_FIELDS)
    );

    let (_, client) = build_fake(vec![json(r#"{"user":{"displayName":"x"}}"#)]);
    let error = client
        .about(&cred("t", 60_000))
        .await
        .expect_err("缺 permissionId 应失败");
    assert_eq!(code(&error), "CLOUD_DRIVE_RESPONSE_INVALID");
}

#[tokio::test]
async fn expired_credentials_and_upstream_errors_never_leak() {
    let (fake, client) = build_fake(vec![json(FOLDER)]);
    let error = client
        .list_folder(&cred("t", -1), "folder-1", None)
        .await
        .expect_err("过期凭据应被拒绝");
    assert_eq!(code(&error), "CLOUD_DRIVE_UNAUTHORIZED");
    let error = client
        .about(&cred("", 60_000))
        .await
        .expect_err("空令牌应被拒绝");
    assert_eq!(code(&error), "CLOUD_DRIVE_UNAUTHORIZED");
    assert!(
        fake.seen.lock().unwrap().is_empty(),
        "凭据检查必须早于任何网络 IO"
    );

    for (status, expected, retryable) in [
        (401, "CLOUD_DRIVE_UNAUTHORIZED", false),
        (403, "CLOUD_DRIVE_FORBIDDEN", false),
        (404, "CLOUD_DRIVE_NOT_FOUND", false),
        (500, "CLOUD_DRIVE_UNAVAILABLE", true),
    ] {
        let mut response = json(&format!(r#"{{"error":"canary-{status}"}}"#));
        response.status = status;
        let (_, client) = build_fake(vec![response]);
        let error = client
            .stat_file(&cred("t", 60_000), "pdf_1")
            .await
            .expect_err("异常状态应失败");
        assert_eq!((code(&error), error.retryable()), (expected, retryable));
        assert!(
            !format!("{error:?} {error}").contains(&format!("canary-{status}")),
            "错误泄露了上游正文"
        );
    }
}

#[tokio::test]
async fn read_pdf_refuses_non_pdf_and_oversized_files_before_media_io() {
    let (fake, client) = build_fake(vec![meta("text/html", Some(13))]);
    let error = client
        .read_pdf(&cred("t", 60_000), "pdf_1", None)
        .await
        .expect_err("非 PDF 元数据应被拒绝");
    assert_eq!(code(&error), "CLOUD_DRIVE_UNSUPPORTED_CONTENT");
    assert_eq!(fake.seen.lock().unwrap().len(), 1, "不应发起 media 请求");

    let (fake, client) = build_fake(vec![meta(PDF_MIME, Some(MAX_PDF_TOTAL_BYTES + 1))]);
    let error = client
        .read_pdf(&cred("t", 60_000), "pdf_1", None)
        .await
        .expect_err("超大 PDF 应被拒绝");
    assert_eq!(code(&error), "CLOUD_DRIVE_PDF_TOO_LARGE");
    assert_eq!(fake.seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn read_pdf_validates_whole_and_ranged_bodies() {
    let (fake, client) = build_fake(vec![
        meta(PDF_MIME, Some(PDF.len() as u64)),
        media(200, PDF, None),
    ]);
    let body = client
        .read_pdf(&cred("t", 60_000), "pdf_1", None)
        .await
        .expect("整份 PDF 应成功");
    assert_eq!(
        (body.total_size, body.content_range),
        (PDF.len() as u64, None)
    );
    assert_eq!(body.bytes.as_slice(), PDF);
    let seen = fake.seen.lock().unwrap().clone();
    assert_eq!(seen[1].2, MEDIA_LIMIT);
    assert_eq!(param(&seen[1].0, "alt").as_deref(), Some("media"));

    // 请求 0-999，对象只有 13 字节：服务端夹紧成 0-12，必须接受并原样交回。
    let requested = RemoteByteRange {
        start: 0,
        end: Some(999),
    };
    let (fake, client) = build_fake(vec![
        meta(PDF_MIME, Some(PDF.len() as u64)),
        media(206, PDF, Some("bytes 0-12/13")),
    ]);
    let body = client
        .read_pdf(&cred("t", 60_000), "pdf_1", Some(requested))
        .await
        .expect("夹紧范围应成功");
    assert_eq!(
        body.content_range,
        Some(RemoteContentRange {
            start: 0,
            end: 12,
            total: 13
        })
    );
    assert_eq!(fake.seen.lock().unwrap()[1].1, Some(requested));
}

#[tokio::test]
async fn read_pdf_rejects_unsafe_range_and_media_shapes() {
    let size = Some(PDF.len() as u64);
    let range = RemoteByteRange {
        start: 1,
        end: Some(4),
    };
    let cases = [
        (range, media(200, PDF, None), "SOURCE_RANGE_UNSUPPORTED"),
        (
            range,
            media(206, b"PDF-", None),
            "CLOUD_DRIVE_RESPONSE_INVALID",
        ),
        (
            range,
            media(206, b"PDF-", Some("bytes 0-3/13")),
            "CLOUD_DRIVE_RESPONSE_INVALID",
        ),
        (
            range,
            media(206, b"PDF", Some("bytes 1-4/13")),
            "CLOUD_DRIVE_RESPONSE_INVALID",
        ),
        (
            range,
            media(206, b"PDF-", Some("bytes 1-4/99")),
            "CLOUD_DRIVE_RESPONSE_INVALID",
        ),
        (
            range,
            media(200, b"<html>", None),
            "SOURCE_RANGE_UNSUPPORTED",
        ),
    ];
    for (range, response, expected) in cases {
        let (_, client) = build_fake(vec![meta(PDF_MIME, size), response]);
        let error = client
            .read_pdf(&cred("t", 60_000), "pdf_1", Some(range))
            .await
            .expect_err("非法分片应失败");
        assert_eq!(code(&error), expected, "case {expected}");
    }
    // 起点越过 EOF：在 media 之前就失败。
    let (fake, client) = build_fake(vec![meta(PDF_MIME, size)]);
    let error = client
        .read_pdf(
            &cred("t", 60_000),
            "pdf_1",
            Some(RemoteByteRange {
                start: 13,
                end: None,
            }),
        )
        .await
        .expect_err("越界起点应失败");
    assert_eq!(code(&error), "RANGE_INVALID");
    assert_eq!(fake.seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn read_pdf_rejects_a_whole_body_that_is_not_pdf() {
    let (_, client) = build_fake(vec![
        meta(PDF_MIME, Some(13)),
        media(200, b"<html>nope</html>", None),
    ]);
    let error = client
        .read_pdf(&cred("t", 60_000), "pdf_1", None)
        .await
        .expect_err("伪装成 PDF 的 HTML 应被拒绝");
    assert_eq!(code(&error), "CLOUD_DRIVE_UNSUPPORTED_CONTENT");
    let (_, client) = build_fake(vec![
        meta(PDF_MIME, Some(13)),
        media(200, PDF, Some("bytes 0-12/13")),
    ]);
    let error = client
        .read_pdf(&cred("t", 60_000), "pdf_1", None)
        .await
        .expect_err("整份请求不应接受 206");
    assert_eq!(code(&error), "SOURCE_RANGE_UNSUPPORTED");
}
