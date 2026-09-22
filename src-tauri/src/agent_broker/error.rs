//! Broker 稳定错误码。
//!
//! 契约来源：`docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md` §5.6。
//!
//! 两条规则：
//! - 码是**闭合集合**里的 `SCREAMING_SNAKE_CASE`；客户端以 `code` 判定，不解析 `message`。
//! - `message` 是**可展示文案**：不得包含路径、SQL、堆栈或凭据材料。
//!   因此这里的构造函数全部接收**固定短语**，不接受拼接了用户数据的字符串。

use super::protocol::ErrorBody;

/// Broker 侧错误。带稳定码、可展示消息与可重试语义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokerError {
    code: String,
    message: String,
    retryable: bool,
}

impl BrokerError {
    pub fn new(code: &str, message: &str, retryable: bool) -> Self {
        Self {
            code: code.to_owned(),
            message: message.to_owned(),
            retryable,
        }
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn retryable(&self) -> bool {
        self.retryable
    }

    pub fn to_body(&self) -> ErrorBody {
        ErrorBody {
            code: self.code.clone(),
            message: self.message.clone(),
            retryable: self.retryable,
        }
    }

    // ---- 协议层 ----

    /// 帧类型不认识。不回显客户端给的类型字符串——它可能被用来注入文案。
    pub fn unknown_frame() -> Self {
        Self::new(
            "HAVEN_BROKER_UNKNOWN_FRAME",
            "请求帧类型不受支持；本协议只接受已声明的 Read / Propose 请求。",
            false,
        )
    }

    /// 与 [`Self::unknown_frame`] 相同；参数只为保持调用方 API 稳定，绝不回显客户端输入。
    pub fn unknown_frame_with(_kind: &str) -> Self {
        Self::unknown_frame()
    }

    pub fn protocol_mismatch() -> Self {
        Self::new(
            "HAVEN_BROKER_PROTOCOL_MISMATCH",
            "协议版本不受支持：请升级 Haven 或 MCP server，使两者协议版本一致。",
            false,
        )
    }

    pub fn request_too_large() -> Self {
        Self::new(
            "HAVEN_BROKER_REQUEST_TOO_LARGE",
            "请求超出大小上限。",
            false,
        )
    }

    pub fn response_too_large() -> Self {
        Self::new(
            "HAVEN_BROKER_RESPONSE_TOO_LARGE",
            "响应超出大小上限；请缩小请求范围后重试。",
            false,
        )
    }

    pub fn too_many_inflight() -> Self {
        Self::new(
            "HAVEN_BROKER_TOO_MANY_INFLIGHT",
            "本连接在途请求过多，请稍后重试。",
            true,
        )
    }

    pub fn quota_exceeded() -> Self {
        Self::new(
            "HAVEN_BROKER_QUOTA_EXCEEDED",
            "已超出外部 Agent 可创建的提案配额；请先在栖阅里处理已有提案。",
            false,
        )
    }

    pub fn not_enabled() -> Self {
        Self::new(
            "HAVEN_BROKER_NOT_ENABLED",
            "外部 Agent 接入未开启；请在栖阅设置 → 智能功能中显式开启。",
            false,
        )
    }

    /// 端点被另一个 Haven 实例占用。**fail closed**：不接管、不改名、不换端点。
    pub fn endpoint_busy() -> Self {
        Self::new(
            "HAVEN_BROKER_ENDPOINT_BUSY",
            "检测到另一个栖阅实例正在提供外部 Agent 接入；本实例的该功能不可用。",
            false,
        )
    }

    pub fn timeout() -> Self {
        Self::new("HAVEN_BROKER_TIMEOUT", "请求超时，可稍后重试。", true)
    }

    /// 请求被客户端 `cancel` 中止。
    ///
    /// **不保证零写入**：提案可能已经落库。取消是尽力而为的（设计 §5.5），
    /// 因此客户端拿到这个码时不能假设"什么都没发生"。
    pub fn cancelled() -> Self {
        Self::new(
            "HAVEN_BROKER_CANCELLED",
            "请求已被取消；若为创建提案，该提案可能已经生成，请在栖阅中确认。",
            false,
        )
    }

    pub fn busy() -> Self {
        Self::new("HAVEN_BROKER_BUSY", "栖阅正忙，暂时无法处理请求。", true)
    }

    pub fn invalid_argument(detail: &str) -> Self {
        Self::new("INVALID_ARGUMENT", detail, false)
    }

    pub fn capability_unavailable() -> Self {
        Self::new(
            "HAVEN_CAPABILITY_UNAVAILABLE",
            "当前栖阅版本未实现该能力。",
            false,
        )
    }

    pub fn internal() -> Self {
        Self::new(
            "INTERNAL_ERROR",
            "Broker 内部错误；该失败不携带任何可展示的细节。",
            false,
        )
    }
}

/// 把 `AppError` 归一成 Broker 错误。
///
/// `AppError::user_message()` 按契约就是可展示文案（既有 Wire 层同样直接投影它），
/// 因此这里原样传递 —— 但 `code` 仍然是 Haven 的稳定码，客户端据此判定。
impl From<&haven_common::AppError> for BrokerError {
    fn from(error: &haven_common::AppError) -> Self {
        let code = error.code().as_str();
        let code_is_safe = code_is_safe(code);
        let message_is_safe = error.kind() != haven_common::ErrorKind::Internal
            && broker_message_is_safe(error.user_message());
        if code_is_safe && message_is_safe {
            Self::new(code, error.user_message(), error.retryable())
        } else {
            Self::new(
                if code_is_safe { code } else { "INTERNAL_ERROR" },
                "请求未能完成；详细信息不会通过外部 Agent 返回。",
                error.retryable(),
            )
        }
    }
}

fn code_is_safe(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 64
        && code
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

/// `AppError::user_message` 对 UI 是展示文案，但 Broker 是外部 Agent 边界；这里再做
/// 一层保守过滤，避免未来某个底层错误把路径、SQL 或 Provider 原文带到外部。
fn broker_message_is_safe(message: &str) -> bool {
    if message.is_empty() || message.len() > 512 || message.chars().any(char::is_control) {
        return false;
    }
    let lowered = message.to_ascii_lowercase();
    let forbidden_fragments = [
        "select ",
        "insert ",
        "update ",
        "delete ",
        "pragma ",
        "sqlite",
        "http://",
        "https://",
        "file://",
        "bearer ",
        "api_key",
        "apikey",
        "credential",
        "password=",
        "token=",
        "raw response",
        "response body",
    ];
    !message.contains('\\')
        && !message.contains('/')
        && !message.contains('<')
        && !message.contains('>')
        && !forbidden_fragments
            .iter()
            .any(|fragment| lowered.contains(fragment))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_screaming_snake_case() {
        for error in [
            BrokerError::unknown_frame(),
            BrokerError::protocol_mismatch(),
            BrokerError::request_too_large(),
            BrokerError::response_too_large(),
            BrokerError::too_many_inflight(),
            BrokerError::quota_exceeded(),
            BrokerError::not_enabled(),
            BrokerError::endpoint_busy(),
            BrokerError::timeout(),
            BrokerError::busy(),
            BrokerError::capability_unavailable(),
            BrokerError::internal(),
        ] {
            let code = error.code();
            assert!(
                code.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'),
                "码必须是 SCREAMING_SNAKE_CASE：{code}"
            );
            assert!(!error.message().is_empty());
            // 文案里不得出现路径分隔符或尖括号。
            for forbidden in ['\\', '<', '>'] {
                assert!(
                    !error.message().contains(forbidden),
                    "{code} 的文案不得包含 {forbidden}"
                );
            }
        }
    }

    #[test]
    fn only_transient_failures_are_retryable() {
        assert!(BrokerError::timeout().retryable());
        assert!(BrokerError::busy().retryable());
        assert!(BrokerError::too_many_inflight().retryable());
        assert!(!BrokerError::quota_exceeded().retryable());
        assert!(!BrokerError::not_enabled().retryable());
        assert!(!BrokerError::endpoint_busy().retryable());
        assert!(!BrokerError::capability_unavailable().retryable());
    }

    #[test]
    fn app_error_mapping_redacts_internal_and_sensitive_details() {
        for (kind, message) in [
            (
                haven_common::ErrorKind::Internal,
                "provider raw response: {secret}",
            ),
            (
                haven_common::ErrorKind::Validation,
                "D:\\private\\token.txt",
            ),
            (haven_common::ErrorKind::Network, "SELECT * FROM secrets"),
            (
                haven_common::ErrorKind::Network,
                "https://provider.invalid/key",
            ),
        ] {
            let error = haven_common::AppError::new("PROVIDER_FAILED", kind, message, false);
            let mapped = BrokerError::from(&error);
            assert_eq!(mapped.code(), "PROVIDER_FAILED");
            assert_eq!(
                mapped.message(),
                "请求未能完成；详细信息不会通过外部 Agent 返回。"
            );
            assert!(!mapped.message().contains("secret"));
            assert!(!mapped.message().contains("provider.invalid"));
        }
    }
}
