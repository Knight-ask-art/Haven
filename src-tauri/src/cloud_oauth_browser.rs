//! 用系统默认浏览器打开 Google 授权页（桌面组合根适配器）。
//!
//! 这是 [`CloudOAuthBrowserPort`] 的生产实现：只有在用户主动发起「连接云盘」后，由
//! `CloudDriveAuthPort::start` 调用一次；构造和应用启动不会触发任何浏览器动作，
//! 更不会在后台自行打开页面。
//!
//! 派发之前先做 URL 策略校验：地址必须恰好是本应用生成的形状——`https` +
//! `accounts.google.com` + 443 + `/o/oauth2/v2/auth`，无 userinfo、无 fragment、无反斜杠、
//! 无控制字符，query 参数集合完整且固定（`scope` 必须是固定只读 scope、
//! `code_challenge_method` 必须是 S256、`redirect_uri` 必须是
//! `http://127.0.0.1:<port>/oauth2/callback`）。任何不符合的地址都在进程 / 系统调用**之前**
//! 以固定错误码拒绝；错误文案与 `Debug` 都不回显 URL、query 或参数值。
//!
//! 打开方式全部是参数化调用，没有 shell 字符串拼接：Windows 用系统 API `ShellExecuteW`
//! （不经过 cmd / PowerShell），macOS 用 `/usr/bin/open`，Linux 用 `xdg-open`。

use std::collections::HashMap;

use async_trait::async_trait;

use haven_application::services::cloud_storage::ports::CloudOAuthBrowserPort;
use haven_common::{AppError, ErrorKind};

/// 固定授权端点；host 与路径由本模块校验，调用方不能替换。
const AUTHORIZATION_HOST: &str = "accounts.google.com";
const AUTHORIZATION_PATH: &str = "/o/oauth2/v2/auth";
const AUTHORIZATION_SCOPE: &str = "https://www.googleapis.com/auth/drive.readonly";
/// 回调地址只接受本机回环的固定形状。
const LOOPBACK_REDIRECT_PREFIX: &str = "http://127.0.0.1:";
const LOOPBACK_REDIRECT_PATH: &str = "/oauth2/callback";
/// 固定端点只接受默认 HTTPS 端口。
const TLS_PORT: u16 = 443;
const MAX_URL_BYTES: usize = 4096;
const MAX_CLIENT_ID_LEN: usize = 128;
const MAX_STATE_LEN: usize = 128;
const MAX_REDIRECT_URI_LEN: usize = 128;
/// S256 challenge 是 SHA-256 摘要的 base64url 无填充编码，长度固定 43。
const S256_CHALLENGE_LEN: usize = 43;
/// 授权 URL 的完整参数集合：多一个、少一个或重复都不接受。
const AUTHORIZATION_PARAMS: &[&str] = &[
    "access_type",
    "client_id",
    "code_challenge",
    "code_challenge_method",
    "prompt",
    "redirect_uri",
    "response_type",
    "scope",
    "state",
];

fn url_rejected() -> AppError {
    AppError::new(
        "CLOUD_OAUTH_BROWSER_URL_REJECTED",
        ErrorKind::Security,
        "云盘授权地址不符合固定策略",
        false,
    )
}

fn open_failed() -> AppError {
    AppError::new(
        "CLOUD_OAUTH_BROWSER_FAILED",
        ErrorKind::Io,
        "无法打开系统浏览器",
        true,
    )
}

/// 系统浏览器适配器；无状态，构造不产生任何副作用。
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemGoogleOAuthBrowser;

impl SystemGoogleOAuthBrowser {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl CloudOAuthBrowserPort for SystemGoogleOAuthBrowser {
    async fn open_google_authorization(&self, url: &str) -> Result<(), AppError> {
        // 策略先于派发：非法地址永远不会到达下面的系统调用 / 子进程。
        validate_authorization_url(url)?;
        launch_system_browser(url).map_err(|_| open_failed())
    }
}

/// 只接受 `handshake` 生成的授权地址形状；纯策略判断、无副作用，因此可在测试里直接调用。
fn validate_authorization_url(raw: &str) -> Result<(), AppError> {
    if raw.is_empty()
        || raw.len() > MAX_URL_BYTES
        || raw.contains('\\')
        || raw.chars().any(char::is_control)
    {
        return Err(url_rejected());
    }
    let url = reqwest::Url::parse(raw).map_err(|_| url_rejected())?;
    if url.scheme() != "https"
        || url.host_str() != Some(AUTHORIZATION_HOST)
        || url.port_or_known_default() != Some(TLS_PORT)
        || url.path() != AUTHORIZATION_PATH
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(url_rejected());
    }
    let mut params: HashMap<String, String> = HashMap::new();
    for (name, value) in url.query_pairs() {
        if !AUTHORIZATION_PARAMS.contains(&name.as_ref())
            || params
                .insert(name.into_owned(), value.into_owned())
                .is_some()
        {
            return Err(url_rejected());
        }
    }
    if params.len() != AUTHORIZATION_PARAMS.len()
        || params.get("response_type").map(String::as_str) != Some("code")
        || params.get("access_type").map(String::as_str) != Some("offline")
        || params.get("prompt").map(String::as_str) != Some("consent")
        || params.get("scope").map(String::as_str) != Some(AUTHORIZATION_SCOPE)
        || params.get("code_challenge_method").map(String::as_str) != Some("S256")
    {
        return Err(url_rejected());
    }
    let client_id = params
        .get("client_id")
        .map(String::as_str)
        .unwrap_or_default();
    let state = params.get("state").map(String::as_str).unwrap_or_default();
    let challenge = params
        .get("code_challenge")
        .map(String::as_str)
        .unwrap_or_default();
    let redirect = params
        .get("redirect_uri")
        .map(String::as_str)
        .unwrap_or_default();
    if !bounded_client_id(client_id)
        || !is_base64url(state, 16, MAX_STATE_LEN)
        || !is_base64url(challenge, S256_CHALLENGE_LEN, S256_CHALLENGE_LEN)
        || !is_loopback_redirect(redirect)
    {
        return Err(url_rejected());
    }
    Ok(())
}

/// client id 与 `handshake` 的字符集一致：非空、有界、只含 `[A-Za-z0-9-_.]`。
fn bounded_client_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CLIENT_ID_LEN
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `state` 与 `code_challenge` 都是 base64url 无填充字符串。
fn is_base64url(value: &str, min_len: usize, max_len: usize) -> bool {
    (min_len..=max_len).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// 只接受 `http://127.0.0.1:<port>/oauth2/callback`，端口必须是有值的十进制数字。
fn is_loopback_redirect(value: &str) -> bool {
    if value.is_empty() || value.len() > MAX_REDIRECT_URI_LEN {
        return false;
    }
    let Some(rest) = value.strip_prefix(LOOPBACK_REDIRECT_PREFIX) else {
        return false;
    };
    let Some((port, path)) = rest.split_once('/') else {
        return false;
    };
    !port.is_empty()
        && port.bytes().all(|b| b.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|port| port != 0)
        && format!("/{path}") == LOOPBACK_REDIRECT_PATH
}

/// 以 NUL 结尾的 UTF-16 缓冲；Windows 系统调用只在本调用期间读它。
#[cfg(target_os = "windows")]
fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Windows：系统 API `ShellExecuteW` 打开默认浏览器，不经过 cmd / PowerShell。
#[cfg(target_os = "windows")]
fn launch_system_browser(url: &str) -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let operation = to_wide("open");
    let file = to_wide(url);
    // 句柄在 windows-sys 里是裸指针；显式转换让代码不依赖别名的具体定义。
    let hwnd = std::ptr::null_mut::<core::ffi::c_void>() as HWND;
    // SAFETY：两个宽字符串缓冲在本调用期间存活，其余参数是空指针与固定常量；
    // ShellExecuteW 接收独立参数，没有可注入的 shell 字符串。
    let result = unsafe {
        ShellExecuteW(
            hwnd,
            operation.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW 的文档约定：返回值 <= 32 表示调用失败（未找到关联程序、被拒绝等）。
    if (result as isize) <= 32 {
        return Err(std::io::Error::other("ShellExecuteW 未能打开默认浏览器"));
    }
    Ok(())
}

/// macOS：`/usr/bin/open`，地址作为独立 argv 项传入。
#[cfg(target_os = "macos")]
fn launch_system_browser(url: &str) -> std::io::Result<()> {
    spawn_detached("/usr/bin/open", url)
}

/// Linux 及其它 Unix：`xdg-open`，地址作为独立 argv 项传入。
#[cfg(all(unix, not(target_os = "macos")))]
fn launch_system_browser(url: &str) -> std::io::Result<()> {
    spawn_detached("xdg-open", url)
}

/// 启动浏览器子进程；退出状态交给后台线程回收，避免留下僵尸进程。
#[cfg(unix)]
fn spawn_detached(program: &str, url: &str) -> std::io::Result<()> {
    let mut child = std::process::Command::new(program)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// 其它平台没有系统浏览器入口：返回固定错误，不 panic。
#[cfg(not(any(target_os = "windows", unix)))]
fn launch_system_browser(_url: &str) -> std::io::Result<()> {
    Err(std::io::Error::other("当前平台没有系统浏览器入口"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 与 `cloud_drive::handshake::authorization_url` 输出同形的固定样例。
    const CLIENT_ID: &str = "1234567890-abc.apps.googleusercontent.com";
    const STATE: &str = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-_";
    /// RFC 7636 附录 B 的公开 S256 challenge 样例（43 字符）。
    const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

    fn valid_url() -> String {
        format!(
            "https://accounts.google.com/o/oauth2/v2/auth?client_id={CLIENT_ID}&redirect_uri=http%3A%2F%2F127.0.0.1%3A51234%2Foauth2%2Fcallback&response_type=code&scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fdrive.readonly&access_type=offline&prompt=consent&code_challenge={CHALLENGE}&code_challenge_method=S256&state={STATE}"
        )
    }

    #[test]
    fn accepts_the_fixed_authorization_shape() {
        assert!(validate_authorization_url(&valid_url()).is_ok());
    }

    #[test]
    fn rejects_foreign_origin_path_and_transport() {
        let valid = valid_url();
        let cases = [
            valid.replacen("https://", "http://", 1),
            valid.replacen(AUTHORIZATION_HOST, "accounts.google.com.evil.example", 1),
            valid.replacen(AUTHORIZATION_HOST, "evil.example", 1),
            valid.replacen(AUTHORIZATION_PATH, "/o/oauth2/v2/auth/extra", 1),
            valid.replacen(AUTHORIZATION_PATH, "/o/oauth2/v2/authorize", 1),
            valid.replacen(AUTHORIZATION_HOST, "accounts.google.com:8443", 1),
            valid.replacen("https://", "https://user@", 1),
            format!("{valid}#access_token=x"),
            valid.replace(AUTHORIZATION_HOST, "accounts\\google.com"),
        ];
        for case in cases {
            assert!(
                validate_authorization_url(&case).is_err(),
                "该地址必须被拒绝"
            );
        }
    }

    #[test]
    fn rejects_query_outside_the_fixed_shape() {
        let valid = valid_url();
        let encoded_scope = "https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fdrive.readonly";
        let cases = [
            valid.replace(
                encoded_scope,
                "https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fdrive",
            ),
            valid.replace("code_challenge_method=S256", "code_challenge_method=plain"),
            valid.replace("response_type=code", "response_type=token"),
            format!("{valid}&extra=1"),
            format!("{valid}&state={STATE}"),
            valid.replace(&format!("&state={STATE}"), ""),
            valid.replace("127.0.0.1%3A51234", "localhost%3A51234"),
            valid.replace("127.0.0.1%3A51234", "127.0.0.1%3A0"),
            valid.replace("oauth2%2Fcallback", "other"),
            valid.replace(CHALLENGE, "short"),
        ];
        for case in cases {
            assert!(
                validate_authorization_url(&case).is_err(),
                "该 query 必须被拒绝"
            );
        }
    }

    #[test]
    fn rejects_control_characters_and_oversized_input() {
        let with_newline = format!("{}\n", valid_url());
        let oversized = "a".repeat(MAX_URL_BYTES + 1);
        for bad in ["", "  ", with_newline.as_str(), oversized.as_str()] {
            assert!(validate_authorization_url(bad).is_err(), "该输入必须被拒绝");
        }
    }

    /// 非法地址在派发之前就被拒绝：端口方法返回固定错误码，不会触发任何系统浏览器动作。
    #[tokio::test]
    async fn port_rejects_policy_violation_before_dispatch() {
        let browser = SystemGoogleOAuthBrowser::new();
        let error = browser
            .open_google_authorization(
                "https://accounts.google.com.evil.example/o/oauth2/v2/auth?scope=x",
            )
            .await
            .expect_err("非法地址必须被拒绝");
        assert_eq!(error.code().as_str(), "CLOUD_OAUTH_BROWSER_URL_REJECTED");
        assert_eq!(error.kind(), ErrorKind::Security);
        assert!(!format!("{error:?}").contains("evil.example"));
    }
}
