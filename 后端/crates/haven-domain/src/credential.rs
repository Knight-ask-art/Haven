//! 凭据存储契约（安全边界核心）。
//!
//! 规范：ADR-001（Windows Credential Manager Generic Credential，persistence=Local）。
//! 原则：
//! - secret 只以可清零的一次性类型在 Rust 边界存在，不进入 React / 日志 / settings.json / SQLite。
//! - target 名称受约束（`haven:<provider>:<profile-id>`），拒绝 CR/LF、反斜杠、空值、超长。
//! - 上层只接收 `CredentialRef`（DB 可存）与内存中的 `SecretString`（不可存）。

use async_trait::async_trait;
use zeroize::Zeroize;

use haven_common::AppError;

use crate::ids::CredentialRef;

/// target 前缀与总长上限（上限远低于 Windows CRED_MAX_GENERIC_TARGET_NAME_LENGTH=32767，
/// 留足余地并防滥用）。
pub const CREDENTIAL_TARGET_PREFIX: &str = "haven";
pub const CREDENTIAL_TARGET_MAX_LEN: usize = 128;
const CREDENTIAL_PART_MAX_LEN: usize = 60;

/// 一次性内存秘密。
///
/// - `Debug`/`Display` 一律脱敏为 `[REDACTED]`，禁止意外打印。
/// - `Drop` 时清零底层缓冲区。
/// - 不实现 `Clone`：防止秘密被无意识复制残留。
#[derive(Default)]
pub struct SecretString {
    inner: String,
}

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            inner: value.into(),
        }
    }

    /// 暴露底层文本。只允许在 CredentialStore 提供方调用栈内使用，
    /// 禁止塞进通用 IPC DTO。
    pub fn expose(&self) -> &str {
        &self.inner
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        self.inner.zeroize();
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl std::fmt::Display for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl CredentialRef {
    /// 构造受约束的 target：`haven:<provider>:<profile_id>`。
    ///
    /// 校验规则（ADR-001）：
    /// - provider / profile_id 非空。
    /// - 拒绝控制字符（CR/LF 等）、反斜杠、冒号（分隔符）与空白。
    /// - 单段长度 ≤ 60，总长 ≤ 128。
    pub fn new_scoped(provider: &str, profile_id: &str) -> Result<Self, AppError> {
        let target = format!("{CREDENTIAL_TARGET_PREFIX}:{provider}:{profile_id}");
        parse_scoped(&target)
    }
}

/// 完整格式校验：`haven:<provider>:<profile-id>`。
///
/// 同时是 `FromStr` / `Deserialize` 的入口（S-01 修复：脏 DB / 外部输入
/// 在进入 CredentialRef 前被拒绝），并被 `new_scoped` 与 `build_entry` 兜底复用。
pub fn parse_scoped(target: &str) -> Result<CredentialRef, AppError> {
    if target.len() > CREDENTIAL_TARGET_MAX_LEN {
        return Err(haven_common::validation(format!(
            "凭据 target 超长（{}/{}）",
            target.len(),
            CREDENTIAL_TARGET_MAX_LEN
        )));
    }
    let parts: Vec<&str> = target.split(':').collect();
    if parts.len() != 3 {
        return Err(haven_common::validation(
            "凭据 target 格式非法（应为 haven:<provider>:<profile-id>，恰好两段冒号）",
        ));
    }
    let prefix = parts[0];
    let provider = parts[1];
    let profile_id = parts[2];
    if prefix != CREDENTIAL_TARGET_PREFIX {
        return Err(haven_common::validation(format!(
            "凭据 target 命名空间非法（必须以 {CREDENTIAL_TARGET_PREFIX}: 开头）"
        )));
    }
    let provider = validate_part("provider", provider)?;
    let profile_id = validate_part("profile_id", profile_id)?;
    // 校验通过后重新规范化（拒绝的字符已排除，直接组装避免歧义）。
    let normalized = format!("{prefix}:{provider}:{profile_id}");
    Ok(CredentialRef::from_inner(normalized))
}

fn validate_part<'a>(name: &str, value: &'a str) -> Result<&'a str, AppError> {
    if value.is_empty() {
        return Err(haven_common::validation(format!("凭据 {name} 不能为空")));
    }
    if value.len() > CREDENTIAL_PART_MAX_LEN {
        return Err(haven_common::validation(format!(
            "凭据 {name} 超长（{}/{}）",
            value.len(),
            CREDENTIAL_PART_MAX_LEN
        )));
    }
    if value
        .chars()
        .any(|c| c.is_control() || c == ':' || c == '\\' || c.is_whitespace())
    {
        return Err(haven_common::validation(format!(
            "凭据 {name} 包含不允许的字符（控制字符/冒号/反斜杠/空白）"
        )));
    }
    Ok(value)
}

/// 凭据存储契约。实现方负责平台能力（Windows Credential Manager）与错误归一化。
#[async_trait]
pub trait CredentialStore: Send + Sync {
    /// 写入（覆盖）指定 target 的秘密。
    async fn set(&self, target: &CredentialRef, secret: &SecretString) -> Result<(), AppError>;

    /// 读取指定 target 的秘密；不存在返回 `None`。
    async fn get(&self, target: &CredentialRef) -> Result<Option<SecretString>, AppError>;

    /// 删除指定 target；返回是否实际删除（不存在返回 `false`）。
    async fn delete(&self, target: &CredentialRef) -> Result<bool, AppError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_ref_format() {
        let r = CredentialRef::new_scoped("webdav", "abc-123").unwrap();
        assert_eq!(r.as_str(), "haven:webdav:abc-123");
    }

    #[test]
    fn scoped_ref_rejects_invalid() {
        assert!(CredentialRef::new_scoped("", "x").is_err(), "空 provider");
        assert!(
            CredentialRef::new_scoped("webdav", "").is_err(),
            "空 profile"
        );
        assert!(CredentialRef::new_scoped("web dav", "x").is_err(), "空白");
        assert!(
            CredentialRef::new_scoped("webdav", "a\r\nb").is_err(),
            "CR/LF"
        );
        assert!(
            CredentialRef::new_scoped("webdav", "a\\b").is_err(),
            "反斜杠"
        );
        assert!(CredentialRef::new_scoped("webdav", "a:b").is_err(), "冒号");
        assert!(
            CredentialRef::new_scoped("w".repeat(61).as_str(), "x").is_err(),
            "超长段"
        );
        let long_profile = "p".repeat(200);
        assert!(
            CredentialRef::new_scoped("webdav", &long_profile).is_err(),
            "超长总长"
        );
    }

    #[test]
    fn secret_string_is_redacted_and_not_clone() {
        let secret = SecretString::new("super-secret-value");
        assert_eq!(format!("{:?}", secret), "[REDACTED]");
        assert_eq!(format!("{secret}"), "[REDACTED]");
        assert_eq!(secret.expose(), "super-secret-value");
        fn assert_not_clone<T: ?Sized>() {}
        assert_not_clone::<SecretString>();
    }

    #[test]
    fn from_str_rejects_foreign_namespace() {
        // S-01：外部 target 不得通过 FromStr / Deserialize 进入 CredentialRef。
        for foreign in [
            "foreign:webdav:abc-123",
            "haven",
            "haven:webdav",
            "haven:webdav:abc:extra",
            ":webdav:abc",
            "haven::abc",
            "haven:webdav:",
            "haven:web dav:x",
            "haven:webdav:a\r\nb",
            "haven:webdav:a\\b",
            "haven:webdav:a:b",
        ] {
            assert!(
                foreign.parse::<CredentialRef>().is_err(),
                "应拒绝: {foreign:?}"
            );
            let json = serde_json::to_string(foreign).unwrap();
            assert!(
                serde_json::from_str::<CredentialRef>(&json).is_err(),
                "Deserialize 应拒绝: {foreign:?}"
            );
        }
    }

    #[test]
    fn from_str_roundtrip_and_length_guard() {
        let parsed: CredentialRef = "haven:webdav:abc-123".parse().unwrap();
        assert_eq!(parsed.as_str(), "haven:webdav:abc-123");
        let json = serde_json::to_string(&parsed).unwrap();
        let back: CredentialRef = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, back);

        let too_long = format!("haven:{}:x", "p".repeat(61));
        assert!(
            too_long.parse::<CredentialRef>().is_err(),
            "单段超长 target 拒绝"
        );
        let bad = "w".repeat(200);
        assert!(bad.parse::<CredentialRef>().is_err(), "超长总长拒绝");
        // 单段上限内（60+60）总长 127 ≤ 128，合法；验证上限边界不误伤。
        let max_ok = format!("haven:{}:{}", "p".repeat(60), "q".repeat(60));
        assert!(
            max_ok.parse::<CredentialRef>().is_ok(),
            "合法边界（127 字符）应通过"
        );
    }
}
