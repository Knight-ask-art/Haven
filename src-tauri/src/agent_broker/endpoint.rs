//! 每用户稳定端点：计算、平台形态与校验。
//!
//! 契约来源：`docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md` §4。
//!
//! 三条性质：
//! - **稳定**：同一用户重启 Haven 后端点字符串不变，因此写进 MCP 配置的值长期有效。
//! - **每用户唯一**：Windows 用当前用户 SID 的哈希前 8 位做后缀；Unix 用本身就是
//!   每用户隔离的 runtime 目录。
//! - **不是秘密**：端点名会被复制进客户端配置，必然可被同机进程猜到。安全性来自
//!   OS ACL 与"这条通道只暴露两件事"，不来自名字的隐蔽性。
//!
//! 两条 Unix 形态都接受，且**只接受这两条**：
//! - `$XDG_RUNTIME_DIR/haven/agent-v1.sock`
//! - `$HOME/.haven/run/agent-v1.sock`（`XDG_RUNTIME_DIR` 未设置或非绝对路径时的回退）
//!
//! 路径构造与校验写成**平台无关的纯函数**，因此即使在没有 Unix 原语的机器上也能被
//! 完整测试——校验规则是安全边界的一部分，不能只在某一种平台上可验证。

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::error::BrokerError;

/// Windows Named Pipe 前缀。Node 侧 `endpoint.ts` 使用同一份前缀常量语义。
pub const PIPE_PREFIX: &str = r"\\.\pipe\haven-agent-v1-";
/// Unix 端点在 runtime 根之下的目录名。
pub const SOCKET_DIR_NAME: &str = "haven";
/// Unix 回退端点在 `$HOME` 之下的目录段。
pub const FALLBACK_DIR_SEGMENTS: [&str; 2] = [".haven", "run"];
/// Unix 端点固定文件名。
pub const SOCKET_FILE_NAME: &str = "agent-v1.sock";
/// Unix **活实例锁**文件名：与端点 socket 同父目录、固定名。
///
/// 它**不是**端点，也不被 [`validate_unix_socket`] 接受——它只是同一目录里的一个内部
/// 文件，用来把"这个用户下此刻有没有活的 Haven 实例在提供 Broker"变成一次
/// `flock(LOCK_EX | LOCK_NB)` 判定（契约来源：`MCP_EXTERNAL_AGENT_TRANSPORT.md` §7.3）。
pub const LOCK_FILE_NAME: &str = "agent-v1.lock";
/// Windows pipe 名长度上限。完整 pipe 路径上限远高于此，这里取保守值。
pub const PIPE_NAME_MAX_LEN: usize = 256;
/// Unix `sun_path` 安全上限：低于 Linux 的 107 与 macOS 的 103。
pub const SOCKET_PATH_MAX_LEN: usize = 100;

/// 解析出的端点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrokerEndpoint {
    /// Windows Named Pipe，例如 `\\.\pipe\haven-agent-v1-3f2a91c4`。
    NamedPipe(String),
    /// Unix domain socket 的绝对路径。
    UnixSocket(PathBuf),
}

impl BrokerEndpoint {
    /// 投给 UI 与客户端配置的字符串形态。
    pub fn display(&self) -> String {
        match self {
            Self::NamedPipe(name) => name.clone(),
            Self::UnixSocket(path) => path.to_string_lossy().into_owned(),
        }
    }
}

/// 由用户 SID 派生 8 位十六进制后缀（纯函数）。
pub fn sid_suffix(sid: &str) -> String {
    let digest = Sha256::digest(sid.as_bytes());
    let hex = format!("{digest:x}");
    hex[..8].to_owned()
}

/// 由 SID 构造完整 pipe 名（纯函数）。
pub fn named_pipe_for_sid(sid: &str) -> String {
    format!("{PIPE_PREFIX}{}", sid_suffix(sid))
}

/// `$XDG_RUNTIME_DIR/haven/agent-v1.sock`。
pub fn unix_socket_at(runtime_root: &Path) -> PathBuf {
    runtime_root.join(SOCKET_DIR_NAME).join(SOCKET_FILE_NAME)
}

/// 端点 socket 对应的活实例锁文件路径（**同一父目录**）。
///
/// 纯路径拼接，因此任何平台都能测。返回 `None` 表示端点没有父目录，调用方按"参数不合法"
/// 处理。这里**不**做任何候选目录搜索：锁的位置由端点的位置唯一决定，与端点本身一样是
/// 每用户固定的。
pub fn unix_lock_file_at(socket_path: &Path) -> Option<PathBuf> {
    socket_path
        .parent()
        .map(|parent| parent.join(LOCK_FILE_NAME))
}

/// 按 Unix 语义判断绝对路径。
///
/// 这些纯函数也在 Windows 测试目标上验证 Unix 形态；`Path::is_absolute` 在 Windows
/// 上会把 `/run/user/...` 解释成没有盘符的路径而返回 false，因此不能用于这里的
/// 协议形状判断。真正的 Unix listener 只会在 `cfg(unix)` 下调用这些结果。
fn is_unix_absolute(path: &Path) -> bool {
    path.to_string_lossy().starts_with('/')
}

/// `$HOME/.haven/run/agent-v1.sock`。
pub fn unix_fallback_socket(home: &Path) -> PathBuf {
    let mut path = home.to_path_buf();
    for segment in FALLBACK_DIR_SEGMENTS {
        path.push(segment);
    }
    path.push(SOCKET_FILE_NAME);
    path
}

/// 依据可注入的 runtime 根与 home 解析 Unix 端点。
///
/// 纯函数：把环境变量作为参数传入，因此两条分支都能在任何平台上被测试。
/// `XDG_RUNTIME_DIR` 只有是**绝对路径**时才被采用；否则回退到 `$HOME`。
pub fn unix_socket_path(
    runtime_root: Option<&Path>,
    home: Option<&Path>,
) -> Result<PathBuf, BrokerError> {
    if let Some(root) = runtime_root.filter(|path| is_unix_absolute(path)) {
        return Ok(unix_socket_at(root));
    }
    if let Some(home) = home.filter(|path| is_unix_absolute(path)) {
        return Ok(unix_fallback_socket(home));
    }
    // 两者都不可用（或都不是绝对路径）：fail closed，**不**回退到 /tmp 之类的共享目录。
    Err(BrokerError::new(
        "HAVEN_BROKER_ENDPOINT_UNSUPPORTED",
        "无法确定每用户运行目录（XDG_RUNTIME_DIR 与 HOME 均不可用）。",
        false,
    ))
}

/// 校验一个 Unix 端点字符串是否为**当前用户两条合法形态之一**。
///
/// 不放宽到"任意路径"：必须与由 `runtime_root` / `home` 推导出的两条路径之一**完全相等**。
/// 单独的形态检查（绝对、文件名、父目录名）不足以构成边界——`/tmp/haven/agent-v1.sock`
/// 也能过形态检查，但它不是本进程的端点。
pub fn validate_unix_socket(
    raw: &str,
    runtime_root: Option<&Path>,
    home: Option<&Path>,
) -> Result<(), BrokerError> {
    if raw.is_empty() || raw.len() > SOCKET_PATH_MAX_LEN {
        return Err(BrokerError::invalid_argument("端点路径长度不合法。"));
    }
    if raw.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(BrokerError::invalid_argument(
            "端点路径不得包含空白或控制字符。",
        ));
    }
    if !raw.starts_with('/') {
        return Err(BrokerError::invalid_argument("端点必须是绝对路径。"));
    }
    if raw.contains("..") {
        return Err(BrokerError::invalid_argument(
            "端点路径不得包含上级目录跳转。",
        ));
    }

    let mut allowed = Vec::new();
    if let Some(root) = runtime_root.filter(|path| is_unix_absolute(path)) {
        allowed.push(unix_socket_at(root));
    }
    if let Some(home) = home.filter(|path| is_unix_absolute(path)) {
        allowed.push(unix_fallback_socket(home));
    }
    if allowed.is_empty() {
        return Err(BrokerError::new(
            "HAVEN_BROKER_ENDPOINT_UNSUPPORTED",
            "无法确定每用户运行目录（XDG_RUNTIME_DIR 与 HOME 均不可用）。",
            false,
        ));
    }
    // 按字节形状比较而不是 `Path` 比较：Windows 会把末尾分隔符规范化，
    // 但这个函数验证的是 Unix endpoint 字符串的精确形态。
    if allowed
        .iter()
        .any(|path| path.to_string_lossy().as_ref() == raw)
    {
        return Ok(());
    }
    Err(BrokerError::invalid_argument(
        "端点路径不是当前用户的 Haven 运行端点。",
    ))
}

/// 校验一个端点字符串是否符合**当前平台**的闭合形态。
pub fn validate_for_current_platform(raw: &str) -> Result<(), BrokerError> {
    if raw.is_empty() || raw.len() > PIPE_NAME_MAX_LEN {
        return Err(BrokerError::invalid_argument("端点字符串长度不合法。"));
    }
    if raw.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(BrokerError::invalid_argument(
            "端点字符串不得包含空白或控制字符。",
        ));
    }
    // 任何 scheme、相对路径、UNC 共享路径都拒绝。
    for forbidden in ["://", "..", "@"] {
        if raw.contains(forbidden) {
            return Err(BrokerError::invalid_argument("端点字符串形态不合法。"));
        }
    }

    #[cfg(windows)]
    {
        let suffix = raw
            .strip_prefix(PIPE_PREFIX)
            .ok_or_else(|| BrokerError::invalid_argument("端点必须是本机的 Haven 命名管道。"))?;
        if suffix.len() != 8
            || !suffix
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(BrokerError::invalid_argument(
                "命名管道后缀必须是 8 位小写十六进制。",
            ));
        }
        Ok(())
    }

    #[cfg(unix)]
    {
        validate_unix_socket(
            raw,
            std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .as_deref(),
            std::env::var_os("HOME").map(PathBuf::from).as_deref(),
        )
    }
}

/// 计算当前用户的端点；并自校验一次，确保"算出来的值"本身合法。
pub fn resolve_endpoint() -> Result<BrokerEndpoint, BrokerError> {
    let endpoint = platform::resolve()?;
    validate_for_current_platform(&endpoint.display())?;
    Ok(endpoint)
}

#[cfg(windows)]
mod platform {
    use super::*;

    pub(super) fn resolve() -> Result<BrokerEndpoint, BrokerError> {
        let sid = current_user_sid_string()?;
        Ok(BrokerEndpoint::NamedPipe(named_pipe_for_sid(&sid)))
    }

    /// 读取当前进程令牌的用户 SID（字符串形态）。
    ///
    /// 只用**当前进程**的令牌，不接受任何外部输入——端点名因此不能被调用方影响。
    /// listener 侧要用同一条 SID 去构造 DACL，所以这里暴露给 crate 内部。
    pub(crate) fn current_user_sid_string() -> Result<String, BrokerError> {
        use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE};
        use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
        use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY};
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

        // SAFETY：全部为 Win32 文档化的调用序列；每个句柄都在离开作用域前释放，
        // 失败路径同样释放，`LocalFree` 只作用于 `ConvertStringSidToSidW` 分配的缓冲区。
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(BrokerError::internal());
            }

            let mut size: u32 = 0;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size);
            if size == 0 {
                CloseHandle(token);
                return Err(BrokerError::internal());
            }
            let mut buffer = vec![0u8; size as usize];
            let ok = GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                size,
                &mut size,
            );
            if ok == 0 {
                CloseHandle(token);
                return Err(BrokerError::internal());
            }
            let token_user = &*buffer
                .as_ptr()
                .cast::<windows_sys::Win32::Security::TOKEN_USER>();
            let sid_ptr = token_user.User.Sid;

            let mut string_ptr: *mut u16 = std::ptr::null_mut();
            let converted = ConvertSidToStringSidW(sid_ptr, &mut string_ptr);
            let sid = if converted == 0 || string_ptr.is_null() {
                None
            } else {
                let mut len = 0usize;
                while *string_ptr.add(len) != 0 {
                    len += 1;
                }
                let text = String::from_utf16_lossy(std::slice::from_raw_parts(string_ptr, len));
                LocalFree(string_ptr.cast());
                Some(text)
            };

            CloseHandle(token);
            sid.ok_or_else(BrokerError::internal)
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::*;

    pub(super) fn resolve() -> Result<BrokerEndpoint, BrokerError> {
        let runtime_root = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
        let home = std::env::var_os("HOME").map(PathBuf::from);
        Ok(BrokerEndpoint::UnixSocket(unix_socket_path(
            runtime_root.as_deref(),
            home.as_deref(),
        )?))
    }
}

#[cfg(not(any(windows, unix)))]
mod platform {
    use super::*;

    pub(super) fn resolve() -> Result<BrokerEndpoint, BrokerError> {
        // 本平台既非 Windows 也非 Unix：**不伪造**任何端点，明确 fail closed。
        Err(BrokerError::new(
            "HAVEN_BROKER_ENDPOINT_UNSUPPORTED",
            "当前平台不支持外部 Agent 接入的本地端点。",
            false,
        ))
    }
}

/// listener 侧构造 DACL 时要复用同一条 SID：单一来源，避免"端点名用一个用户、
/// ACL 用另一个用户"这种自相矛盾。
#[cfg(windows)]
pub(crate) use platform::current_user_sid_string;

#[cfg(test)]
mod tests {
    use super::*;

    const XDG: &str = "/run/user/1000";
    const HOME: &str = "/home/alice";

    fn xdg_root() -> PathBuf {
        PathBuf::from(XDG)
    }
    fn home_dir() -> PathBuf {
        PathBuf::from(HOME)
    }

    #[test]
    fn sid_suffix_is_stable_and_bounded() {
        let sid = "S-1-5-21-1111111111-2222222222-3333333333-1001";
        let first = sid_suffix(sid);
        assert_eq!(first, sid_suffix(sid), "同一 SID 必须派生同一后缀");
        assert_eq!(first.len(), 8);
        assert!(
            first
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "必须是小写十六进制"
        );
        assert_ne!(first, sid_suffix("S-1-5-21-1-2-3-1002"));
    }

    #[test]
    fn named_pipe_shape_is_exact() {
        let name = named_pipe_for_sid("S-1-5-21-1-2-3-1001");
        assert!(name.starts_with(PIPE_PREFIX));
        assert_eq!(name.len(), PIPE_PREFIX.len() + 8);
        assert!(name.len() <= PIPE_NAME_MAX_LEN);
    }

    #[test]
    fn unix_paths_have_the_two_frozen_shapes() {
        assert_eq!(
            unix_socket_at(&xdg_root()),
            PathBuf::from("/run/user/1000/haven/agent-v1.sock")
        );
        assert_eq!(
            unix_fallback_socket(&home_dir()),
            PathBuf::from("/home/alice/.haven/run/agent-v1.sock"),
            "回退端点必须是 $HOME/.haven/run/agent-v1.sock"
        );
        assert!(unix_socket_at(&xdg_root()).to_string_lossy().len() <= SOCKET_PATH_MAX_LEN);
        assert!(unix_fallback_socket(&home_dir()).to_string_lossy().len() <= SOCKET_PATH_MAX_LEN);
    }

    #[test]
    fn unix_lock_file_sits_next_to_the_socket_and_is_not_an_endpoint() {
        let xdg = unix_socket_at(&xdg_root());
        let fallback = unix_fallback_socket(&home_dir());
        assert_eq!(
            unix_lock_file_at(&xdg),
            Some(PathBuf::from("/run/user/1000/haven/agent-v1.lock"))
        );
        assert_eq!(
            unix_lock_file_at(&fallback),
            Some(PathBuf::from("/home/alice/.haven/run/agent-v1.lock"))
        );

        for socket in [&xdg, &fallback] {
            let lock = unix_lock_file_at(socket).expect("端点必须有父目录");
            assert_eq!(lock.parent(), socket.parent(), "锁必须与端点同父目录");
            assert_ne!(lock.as_path(), socket.as_path(), "锁与端点不能是同一个文件");
            // 锁路径**不是**合法端点：它只能作为同一目录下的内部文件出现，不能被
            // 复制进任何客户端配置。
            assert!(
                validate_unix_socket(
                    &lock.to_string_lossy(),
                    Some(&xdg_root()),
                    Some(&home_dir())
                )
                .is_err(),
                "锁文件路径不得被当作端点接受：{lock:?}"
            );
        }
        // 没有父目录时没有锁文件位置可言（不猜、不回退）。
        assert_eq!(unix_lock_file_at(Path::new("/")), None);
    }

    #[test]
    fn unix_resolution_prefers_xdg_then_falls_back_to_home() {
        assert_eq!(
            unix_socket_path(Some(&xdg_root()), Some(&home_dir())).unwrap(),
            PathBuf::from("/run/user/1000/haven/agent-v1.sock")
        );
        assert_eq!(
            unix_socket_path(None, Some(&home_dir())).unwrap(),
            PathBuf::from("/home/alice/.haven/run/agent-v1.sock")
        );
        // 非绝对路径的 XDG 视同未设置。
        assert_eq!(
            unix_socket_path(Some(Path::new("relative/run")), Some(&home_dir())).unwrap(),
            PathBuf::from("/home/alice/.haven/run/agent-v1.sock")
        );
        // 两者都不可用 → fail closed，**不**回退到 /tmp。
        assert!(unix_socket_path(None, None).is_err());
        assert!(unix_socket_path(Some(Path::new("rel")), Some(Path::new("also-rel"))).is_err());
    }

    #[test]
    fn unix_validator_accepts_both_shapes_and_only_those() {
        let xdg = unix_socket_at(&xdg_root()).to_string_lossy().into_owned();
        let fallback = unix_fallback_socket(&home_dir())
            .to_string_lossy()
            .into_owned();

        assert!(
            validate_unix_socket(&xdg, Some(&xdg_root()), Some(&home_dir())).is_ok(),
            "XDG 形态必须通过"
        );
        assert!(
            validate_unix_socket(&fallback, Some(&xdg_root()), Some(&home_dir())).is_ok(),
            "回退形态必须通过"
        );
        // 只有 HOME 时，XDG 形态不再被接受（它不是本进程的端点）。
        assert!(validate_unix_socket(&xdg, None, Some(&home_dir())).is_err());
        assert!(
            validate_unix_socket(&fallback, Some(&xdg_root()), None).is_err(),
            "没有 HOME 时回退形态不得被接受"
        );
    }

    #[test]
    fn unix_validator_rejects_everything_else() {
        for bad in [
            "",
            "   ",
            "tcp://127.0.0.1:1234",
            "http://localhost/x",
            "ws://x",
            "file:///tmp/x",
            "relative/path.sock",
            "/tmp/haven/agent-v1.sock",
            "/tmp/agent-v1.sock",
            "/run/user/1000/agent-v1.sock",
            "/run/user/1000/haven/other.sock",
            "/run/user/1000/haven/../../etc/passwd",
            "/home/alice/.haven/run/agent-v1.sock/",
            "user@host",
            "has space",
            "has\nnewline",
            &format!("/run/user/1000/haven/{}", "a".repeat(200)),
        ] {
            assert!(
                validate_unix_socket(bad, Some(&xdg_root()), Some(&home_dir())).is_err(),
                "必须拒绝：{bad:?}"
            );
        }
    }

    #[test]
    fn platform_validator_rejects_schemes_traversal_and_whitespace() {
        for bad in [
            "",
            "   ",
            "tcp://127.0.0.1:1234",
            "http://localhost/x",
            "ws://x",
            "file:///tmp/x",
            "relative/path.sock",
            "/tmp/../etc/passwd",
            r"\\server\share\pipe",
            "has space",
            "has\nnewline",
            "user@host",
        ] {
            assert!(
                validate_for_current_platform(bad).is_err(),
                "必须拒绝：{bad:?}"
            );
        }
    }

    #[test]
    fn platform_validator_accepts_only_the_platform_shape() {
        #[cfg(windows)]
        {
            assert!(
                validate_for_current_platform(&named_pipe_for_sid("S-1-5-21-1-2-3-1001")).is_ok()
            );
            for bad in [
                r"\\.\pipe\haven-agent-v1-",
                r"\\.\pipe\haven-agent-v1-ABCDEF12",
                r"\\.\pipe\haven-agent-v1-1234567",
                r"\\.\pipe\haven-agent-v1-123456789",
                r"\\.\pipe\other-pipe-1234abcd",
            ] {
                assert!(
                    validate_for_current_platform(bad).is_err(),
                    "必须拒绝：{bad}"
                );
            }
        }

        #[cfg(unix)]
        {
            let endpoint = resolve_endpoint().expect("测试环境应有 HOME 或 XDG_RUNTIME_DIR");
            assert!(validate_for_current_platform(&endpoint.display()).is_ok());
            assert_eq!(endpoint, resolve_endpoint().unwrap(), "必须稳定");
        }
    }
}
