//! A5 Broker 协议：framing、闭合帧集合、上限与稳定错误码。
//!
//! 契约来源：`docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md` §5。
//!
//! 本文件是**纯逻辑**：不碰 socket、不碰 tokio、不碰 Application service。
//! 这样协议本身可以在任何平台上被完整测试，而平台原语（Named Pipe / Unix socket）
//! 只在 listener 层出现。
//!
//! 三条硬规则：
//! - **闭合**：帧类型只有 `hello` / `context` / `create_proposal` / `cancel`（客户端→服务端）
//!   与 `welcome` / `ok` / `error`（服务端→客户端）。没有 `get_proposal` / `approve` /
//!   `reject` / `apply` / `receipt` / `fs` / `sql` / `secret` / `exec`。
//! - **严格**：帧对象带 `deny_unknown_fields`；多一个字段就是错误，不是忽略。
//! - **有界**：帧/请求/响应/在途/连接五条上限写死，不协商。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use haven_domain::agent::AgentCapabilityManifest;

use super::error::BrokerError;

/// 协议版本。与客户端 `hello.protocol_version` **严格相等**才继续，不做降级。
pub const PROTOCOL_VERSION: u32 = 1;

/// 帧载荷上限。超过即断连（不回复），因为此时连接状态已不可信。
pub const MAX_FRAME_BYTES: usize = 256 * 1024;
/// 单请求上限。
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
/// 单响应上限。刻意高于 MCP 侧 24 000 字符的响应上限，避免两层各自截断。
pub const MAX_RESPONSE_BYTES: usize = 256 * 1024;
/// 每连接在途请求上限。
pub const MAX_INFLIGHT_PER_CONNECTION: usize = 8;
/// 并发连接上限。
pub const MAX_CONNECTIONS: usize = 8;
/// 每连接 pending 提案上限（配额）。
pub const MAX_PENDING_PROPOSALS_PER_CONNECTION: u32 = 8;
/// 每个 Haven 进程窗口的外部提案创建上限（配额）。进程窗口计数，重启清零。
pub const MAX_EXTERNAL_PROPOSALS_PER_PROCESS: u32 = 32;

/// 帧类型（客户端 → 服务端）。闭合集合，恰好 4 项。
pub const CLIENT_FRAME_TYPES: &[&str] = &["hello", "context", "create_proposal", "cancel"];
/// 帧类型（服务端 → 客户端）。闭合集合，恰好 3 项。
pub const SERVER_FRAME_TYPES: &[&str] = &["welcome", "ok", "error"];
/// 服务端授予的请求类型（`welcome.granted_requests`）。闭合集合，**恰好 2 项**。
pub const GRANTED_REQUESTS: &[&str] = &["context", "create_proposal"];

/// 明确禁止的帧类型。它们**不存在**于本协议——列在这里是为了让"不存在"可被断言，
/// 而不是因为解析器要处理它们。
pub const FORBIDDEN_FRAME_TYPES: &[&str] = &[
    "get_proposal",
    "approve",
    "reject",
    "apply",
    "write",
    "delete",
    "rename",
    "receipt",
    "filesystem",
    "fs",
    "sql",
    "secret",
    "secret_read",
    "exec",
    "invoke",
];

// ---------- 字段上限 ----------
//
// 全部是**服务端**上限：外部 Agent 是不可信输入源，任何进入领域身份的字符串都必须
// 先被限长、限字符集。这些数值与 Node 侧 schema 的上限一一对应。

/// `hello.client.name` 上限。
pub const MAX_CLIENT_NAME_LEN: usize = 64;
/// `hello.client.version` 上限。
pub const MAX_CLIENT_VERSION_LEN: usize = 32;
/// `hello.client.instance_id` 上限。
pub const MAX_CLIENT_INSTANCE_ID_LEN: usize = 64;
/// `create_proposal.context_id` 上限（领域侧是 UUID 字符串）。
pub const MAX_CONTEXT_ID_LEN: usize = 64;
/// `create_proposal.base_revision` 上限。
pub const MAX_BASE_REVISION_LEN: usize = 128;
/// canonical digest 的十六进制长度（与 `haven_domain::setting_proposal::is_canonical_digest` 同规则）。
pub const CANONICAL_DIGEST_HEX_LEN: usize = 64;

/// 短文本校验：非空、限长、无控制字符。
///
/// 控制字符必须拒绝而不是清洗：它们能伪造日志行、终端转义序列与 JSON 之外的歧义。
fn validate_short_text(
    label: &'static str,
    value: &str,
    max_len: usize,
) -> Result<(), BrokerError> {
    if value.is_empty() {
        return Err(BrokerError::invalid_argument(match label {
            "client.name" => "hello.client.name 不能为空。",
            "context_id" => "context_id 不能为空。",
            _ => "字段不能为空。",
        }));
    }
    if value.len() > max_len {
        return Err(BrokerError::invalid_argument(match label {
            "client.name" => "hello.client.name 超长。",
            "client.version" => "hello.client.version 超长。",
            "client.instance_id" => "hello.client.instance_id 超长。",
            "context_id" => "context_id 超长。",
            "base_revision" => "base_revision 超长。",
            _ => "字段超长。",
        }));
    }
    if value.chars().any(|c| c.is_control()) {
        return Err(BrokerError::invalid_argument(match label {
            "client.name" => "hello.client.name 不得包含控制字符。",
            "client.version" => "hello.client.version 不得包含控制字符。",
            "client.instance_id" => "hello.client.instance_id 不得包含控制字符。",
            "context_id" => "context_id 不得包含控制字符。",
            "base_revision" => "base_revision 不得包含控制字符。",
            _ => "字段不得包含控制字符。",
        }));
    }
    Ok(())
}

/// 请求 id 必须是**正数**。
///
/// `0` 被保留给"握手阶段没有请求 id 的错误帧"，负数没有意义；两者都拒绝，
/// 避免客户端用一个哨兵值去撞服务端的内部约定。
fn validate_request_id(id: i64) -> Result<(), BrokerError> {
    if id <= 0 {
        return Err(BrokerError::invalid_argument("请求 id 必须是正整数。"));
    }
    Ok(())
}

/// `context` 只接受**空对象**载荷。
///
/// 该用例没有任何参数。允许携带任意字段会让"客户端以为传了参数、服务端静默忽略"
/// 成为可能——那正是本协议在别处极力避免的失败形态。
fn validate_empty_payload(payload: Option<&Value>) -> Result<(), BrokerError> {
    match payload {
        Some(Value::Object(map)) if map.is_empty() => Ok(()),
        Some(_) => Err(BrokerError::invalid_argument(
            "context 不接受任何参数，payload 必须是空对象。",
        )),
        None => Err(BrokerError::invalid_argument(
            "context 必须携带空对象 payload。",
        )),
    }
}

/// canonical digest 的形状：恰好 64 个 ASCII **小写**十六进制字符。
fn is_canonical_digest(value: &str) -> bool {
    value.len() == CANONICAL_DIGEST_HEX_LEN
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

// ---------- 客户端 → 服务端 ----------

/// `hello.client`：**仅用于诊断**。绝不参与授权，也不成为领域身份。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientInfo {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub instance_id: Option<String>,
}

/// 握手帧。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelloFrame {
    #[serde(rename = "type")]
    pub kind: String,
    pub protocol_version: u32,
    pub client: ClientInfo,
}

/// 无载荷请求（`context`）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlainRequest {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: i64,
    #[serde(default)]
    pub payload: Option<Value>,
}

/// `create_proposal` 的载荷。
///
/// **刻意不含 `session_id` / `request_id`**：领域身份由 Broker 生成（见 `session.rs`），
/// 客户端无法提供。多传这两个字段会因为 `deny_unknown_fields` 而被拒绝。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateProposalPayload {
    pub section: String,
    pub context_id: String,
    pub context_hash: String,
    pub base_revision: Option<String>,
    pub patch: Value,
}

/// `create_proposal` 帧。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateProposalFrame {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: i64,
    pub payload: CreateProposalPayload,
}

/// `cancel` 帧。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelFrame {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: i64,
}

/// 解析后的客户端帧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientFrame {
    Hello(HelloFrame),
    Context(PlainRequest),
    CreateProposal(CreateProposalFrame),
    Cancel(CancelFrame),
}

impl ClientFrame {
    /// 该帧是否携带请求 id（`hello` 没有）。
    pub fn request_id(&self) -> Option<i64> {
        match self {
            Self::Hello(_) => None,
            Self::Context(frame) => Some(frame.id),
            Self::CreateProposal(frame) => Some(frame.id),
            Self::Cancel(frame) => Some(frame.id),
        }
    }

    /// 帧类型字符串。
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Hello(_) => "hello",
            Self::Context(_) => "context",
            Self::CreateProposal(_) => "create_proposal",
            Self::Cancel(_) => "cancel",
        }
    }
}

/// 解析一帧客户端 JSON。
///
/// 先取 `type` 再按类型反序列化到具体结构：这样未知类型给出
/// `HAVEN_BROKER_UNKNOWN_FRAME`，而"类型对但字段错"给出 `INVALID_ARGUMENT`，
/// 两者对调用方含义不同。
pub fn parse_client_frame(text: &str) -> Result<ClientFrame, BrokerError> {
    if text.len() > MAX_REQUEST_BYTES {
        return Err(BrokerError::request_too_large());
    }
    let value: Value = serde_json::from_str(text).map_err(|_| BrokerError::unknown_frame())?;
    let object = value.as_object().ok_or_else(BrokerError::unknown_frame)?;
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(BrokerError::unknown_frame)?;

    let invalid = |detail: &'static str| BrokerError::invalid_argument(detail);

    match kind {
        "hello" => {
            let frame: HelloFrame =
                serde_json::from_value(value).map_err(|_| invalid("hello 帧字段不合法"))?;
            validate_short_text("client.name", &frame.client.name, MAX_CLIENT_NAME_LEN)?;
            if let Some(version) = frame.client.version.as_deref() {
                validate_short_text("client.version", version, MAX_CLIENT_VERSION_LEN)?;
            }
            if let Some(instance_id) = frame.client.instance_id.as_deref() {
                validate_short_text(
                    "client.instance_id",
                    instance_id,
                    MAX_CLIENT_INSTANCE_ID_LEN,
                )?;
            }
            Ok(ClientFrame::Hello(frame))
        }
        "context" => {
            let frame: PlainRequest =
                serde_json::from_value(value).map_err(|_| invalid("context 帧字段不合法"))?;
            validate_request_id(frame.id)?;
            validate_empty_payload(frame.payload.as_ref())?;
            Ok(ClientFrame::Context(frame))
        }
        "create_proposal" => {
            let frame: CreateProposalFrame = serde_json::from_value(value)
                .map_err(|_| invalid("create_proposal 帧字段不合法"))?;
            validate_request_id(frame.id)?;
            validate_short_text("context_id", &frame.payload.context_id, MAX_CONTEXT_ID_LEN)?;
            if !is_canonical_digest(&frame.payload.context_hash) {
                return Err(invalid("context_hash 必须是 64 位小写十六进制。"));
            }
            if let Some(revision) = frame.payload.base_revision.as_deref() {
                validate_short_text("base_revision", revision, MAX_BASE_REVISION_LEN)?;
            }
            Ok(ClientFrame::CreateProposal(frame))
        }
        "cancel" => {
            let frame: CancelFrame =
                serde_json::from_value(value).map_err(|_| invalid("cancel 帧字段不合法"))?;
            validate_request_id(frame.id)?;
            Ok(ClientFrame::Cancel(frame))
        }
        _other => Err(BrokerError::unknown_frame_with("unknown")),
    }
}

// ---------- 服务端 → 客户端 ----------

/// 能力清单投影（`welcome.haven.capabilities`）。**服务端是唯一权威**。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CapabilityReport {
    pub settings_read: bool,
    pub settings_proposal: bool,
    pub library_summary_read: bool,
    pub metadata_proposal: bool,
    pub rename_proposal: bool,
    pub secret_read: bool,
    pub filesystem_write: bool,
}

impl CapabilityReport {
    /// 从**领域**能力清单投影。
    ///
    /// 这里**刻意不维护第二份布尔值**：任何一处硬编码都会在领域清单变更时静默漂移，
    /// 于是 Broker 的 `welcome` 会宣称与 Haven 实际提供的能力不一致——而那正是客户端
    /// 唯一的权威来源。`src-tauri/tests` 里有一条测试专门证明"改清单就改报告"。
    pub fn from_manifest(manifest: AgentCapabilityManifest) -> Self {
        let capabilities = manifest.capabilities;
        Self {
            settings_read: capabilities.settings_read,
            settings_proposal: capabilities.settings_proposal,
            library_summary_read: capabilities.library_summary_read,
            metadata_proposal: capabilities.metadata_proposal,
            rename_proposal: capabilities.rename_proposal,
            secret_read: capabilities.secret_read,
            filesystem_write: capabilities.filesystem_write,
        }
    }

    /// 本切片的能力清单（= 领域清单的当前值）。
    pub fn for_current_slice() -> Self {
        Self::from_manifest(AgentCapabilityManifest::for_current_slice())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HavenInfo {
    pub app_version: String,
    pub agent_api_version: u32,
    pub capabilities: CapabilityReport,
}

/// 握手响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WelcomeFrame {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub protocol_version: u32,
    /// **服务端生成**的会话身份。见 `session.rs`。
    pub session_id: String,
    pub haven: HavenInfo,
    pub granted_requests: Vec<&'static str>,
}

impl WelcomeFrame {
    /// 由**领域能力清单**构造握手响应：`agent_api_version` 与 `capabilities` 都来自它，
    /// 因此不存在"版本号一处、能力位另一处"的漂移空间。
    pub fn new(session_id: String, app_version: String, manifest: AgentCapabilityManifest) -> Self {
        Self {
            kind: "welcome",
            protocol_version: PROTOCOL_VERSION,
            session_id,
            haven: HavenInfo {
                app_version,
                agent_api_version: manifest.agent_api_version,
                capabilities: CapabilityReport::from_manifest(manifest),
            },
            granted_requests: GRANTED_REQUESTS.to_vec(),
        }
    }
}

/// 成功响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OkFrame {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub id: i64,
    pub payload: Value,
}

impl OkFrame {
    pub fn new(id: i64, payload: Value) -> Self {
        Self {
            kind: "ok",
            id,
            payload,
        }
    }
}

/// 错误载荷。**不含**堆栈、路径、SQL 或原始 Provider 报文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

/// 失败响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorFrame {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub id: i64,
    pub error: ErrorBody,
}

impl ErrorFrame {
    pub fn new(id: i64, body: ErrorBody) -> Self {
        Self {
            kind: "error",
            id,
            error: body,
        }
    }
}

/// 把响应序列化并做上限校验。
///
/// 超限**不**截断：返回 `HAVEN_BROKER_RESPONSE_TOO_LARGE`，由调用方换成错误帧。
/// 截断会让客户端拿到一份看起来完整、实际缺字段的载荷——那比报错更危险。
pub fn encode_server_frame<T: Serialize>(frame: &T) -> Result<String, BrokerError> {
    let text = serde_json::to_string(frame).map_err(|_| BrokerError::internal())?;
    if text.len() > MAX_RESPONSE_BYTES {
        return Err(BrokerError::response_too_large());
    }
    Ok(text)
}

// ---------- framing ----------

/// 把载荷编码成 `4 字节大端长度 + UTF-8 载荷`。
pub fn encode_frame(payload: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload.as_bytes());
    out
}

/// 从 4 字节长度前缀解析载荷长度。
///
/// 超过 [`MAX_FRAME_BYTES`] 时返回 `None`：调用方必须**断连**而不是继续读——
/// 一个声称要发 4 GiB 的帧，其后的字节流已经无法当协议解释。
pub fn decode_frame_length(prefix: [u8; 4]) -> Option<usize> {
    let len = u32::from_be_bytes(prefix) as usize;
    if len > MAX_FRAME_BYTES {
        return None;
    }
    Some(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello_json() -> String {
        r#"{"type":"hello","protocol_version":1,"client":{"name":"claude-code","version":"1.0"}}"#
            .to_owned()
    }

    #[test]
    fn client_frame_set_is_closed_and_forbidden_types_are_absent() {
        assert_eq!(CLIENT_FRAME_TYPES.len(), 4);
        assert_eq!(SERVER_FRAME_TYPES.len(), 3);
        // `granted_requests` **恰好 2 项**——这是 A5 v1 的核心收敛。
        assert_eq!(GRANTED_REQUESTS.len(), 2);
        for forbidden in FORBIDDEN_FRAME_TYPES {
            assert!(
                !CLIENT_FRAME_TYPES.contains(forbidden),
                "{forbidden} 不得是客户端帧类型"
            );
            assert!(
                !GRANTED_REQUESTS.contains(forbidden),
                "{forbidden} 不得被授予"
            );
        }
        // get_proposal 是本次修订从 v1 删除的，单独点名。
        assert!(!CLIENT_FRAME_TYPES.contains(&"get_proposal"));
        assert!(!GRANTED_REQUESTS.contains(&"get_proposal"));
    }

    #[test]
    fn parses_hello_and_reports_no_request_id() {
        let frame = parse_client_frame(&hello_json()).unwrap();
        assert_eq!(frame.kind(), "hello");
        assert!(frame.request_id().is_none());
        match frame {
            ClientFrame::Hello(hello) => {
                assert_eq!(hello.protocol_version, 1);
                assert_eq!(hello.client.name, "claude-code");
            }
            other => panic!("期望 hello，得到 {other:?}"),
        }
    }

    #[test]
    fn parses_context_create_proposal_and_cancel() {
        let context = parse_client_frame(r#"{"type":"context","id":1,"payload":{}}"#).unwrap();
        assert_eq!(context.request_id(), Some(1));

        let create = parse_client_frame(
            r#"{"type":"create_proposal","id":2,"payload":{"section":"reading","context_id":"c","context_hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","base_revision":null,"patch":{}}}"#,
        )
        .unwrap();
        assert_eq!(create.request_id(), Some(2));

        let cancel = parse_client_frame(r#"{"type":"cancel","id":3}"#).unwrap();
        assert_eq!(cancel.request_id(), Some(3));
    }

    #[test]
    fn context_requires_an_empty_object_payload() {
        for payload in [
            r#"{"type":"context","id":1}"#,
            r#"{"type":"context","id":1,"payload":null}"#,
            r#"{"type":"context","id":1,"payload":{"unexpected":true}}"#,
            r#"{"type":"context","id":1,"payload":[]}"#,
        ] {
            assert_eq!(
                parse_client_frame(payload).unwrap_err().code(),
                "INVALID_ARGUMENT",
                "必须拒绝：{payload}"
            );
        }
    }

    #[test]
    fn unknown_frame_type_is_rejected_with_its_own_code() {
        for forbidden in FORBIDDEN_FRAME_TYPES {
            let json = format!(r#"{{"type":"{forbidden}","id":1}}"#);
            let error = parse_client_frame(&json).unwrap_err();
            assert_eq!(
                error.code(),
                "HAVEN_BROKER_UNKNOWN_FRAME",
                "{forbidden} 必须是未知帧，而不是被解析"
            );
        }
        let error = parse_client_frame(r#"{"type":"nonsense","id":1}"#).unwrap_err();
        assert_eq!(error.code(), "HAVEN_BROKER_UNKNOWN_FRAME");
    }

    #[test]
    fn unknown_frame_errors_do_not_echo_untrusted_type_names() {
        let error =
            parse_client_frame(r#"{"type":"\u001b[31msecret\nlog-injection","id":1}"#).unwrap_err();
        assert_eq!(error.code(), "HAVEN_BROKER_UNKNOWN_FRAME");
        assert!(!error.message().contains("secret"));
        assert!(!error.message().contains("log-injection"));
    }

    #[test]
    fn unknown_fields_are_rejected_not_ignored() {
        // 顶层多字段
        let extra_top = r#"{"type":"context","id":1,"payload":{},"extra":true}"#;
        assert_eq!(
            parse_client_frame(extra_top).unwrap_err().code(),
            "INVALID_ARGUMENT"
        );
        // hello 里多字段
        let extra_hello =
            r#"{"type":"hello","protocol_version":1,"client":{"name":"x"},"token":"secret"}"#;
        assert_eq!(
            parse_client_frame(extra_hello).unwrap_err().code(),
            "INVALID_ARGUMENT"
        );
    }

    #[test]
    fn create_proposal_cannot_carry_client_supplied_identity() {
        // 客户端**不得**传 session_id / request_id —— 领域身份由 Broker 生成。
        for field in ["session_id", "request_id"] {
            let json = format!(
                r#"{{"type":"create_proposal","id":2,"payload":{{"section":"reading","context_id":"c","context_hash":"h","base_revision":null,"patch":{{}},"{field}":"x"}}}}"#
            );
            assert_eq!(
                parse_client_frame(&json).unwrap_err().code(),
                "INVALID_ARGUMENT",
                "{field} 必须被拒绝"
            );
        }
    }

    #[test]
    fn malformed_json_and_non_object_are_unknown_frames() {
        assert_eq!(
            parse_client_frame("not json").unwrap_err().code(),
            "HAVEN_BROKER_UNKNOWN_FRAME"
        );
        assert_eq!(
            parse_client_frame("[1,2,3]").unwrap_err().code(),
            "HAVEN_BROKER_UNKNOWN_FRAME"
        );
        assert_eq!(
            parse_client_frame(r#"{"id":1}"#).unwrap_err().code(),
            "HAVEN_BROKER_UNKNOWN_FRAME"
        );
    }

    #[test]
    fn request_size_limit_is_enforced() {
        let big = format!(
            r#"{{"type":"context","id":1,"payload":{{"pad":"{}"}}}}"#,
            "x".repeat(MAX_REQUEST_BYTES)
        );
        assert_eq!(
            parse_client_frame(&big).unwrap_err().code(),
            "HAVEN_BROKER_REQUEST_TOO_LARGE"
        );
    }

    #[test]
    fn framing_round_trips_and_rejects_oversized_length() {
        let bytes = encode_frame("{}");
        assert_eq!(&bytes[..4], &2u32.to_be_bytes());
        assert_eq!(bytes.len(), 6);

        assert_eq!(decode_frame_length([0, 0, 0, 2]), Some(2));
        assert_eq!(decode_frame_length([0, 0, 0, 0]), Some(0));
        // 恰好等于上限：接受
        assert_eq!(
            decode_frame_length((MAX_FRAME_BYTES as u32).to_be_bytes()),
            Some(MAX_FRAME_BYTES)
        );
        // 超过上限：拒绝（调用方必须断连）
        assert_eq!(
            decode_frame_length((MAX_FRAME_BYTES as u32 + 1).to_be_bytes()),
            None
        );
        assert_eq!(decode_frame_length(u32::MAX.to_be_bytes()), None);
    }

    #[test]
    fn welcome_reports_server_authoritative_capabilities() {
        let welcome = WelcomeFrame::new(
            "abc".into(),
            "0.1.0-beta.1".into(),
            AgentCapabilityManifest::for_current_slice(),
        );
        let text = encode_server_frame(&welcome).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["type"], "welcome");
        assert_eq!(value["protocol_version"], 1);
        assert_eq!(value["session_id"], "abc");
        assert_eq!(value["haven"]["capabilities"]["settings_read"], true);
        assert_eq!(
            value["haven"]["capabilities"]["library_summary_read"],
            false
        );
        assert_eq!(value["haven"]["capabilities"]["secret_read"], false);
        assert_eq!(value["haven"]["capabilities"]["filesystem_write"], false);
        assert_eq!(
            value["granted_requests"],
            serde_json::json!(["context", "create_proposal"])
        );
        // 不得夹带任何写权限或身份材料。
        for forbidden in ["token", "credential", "secret_value", "path"] {
            assert!(!text.contains(forbidden), "welcome 不得包含 {forbidden}");
        }
    }

    #[test]
    fn error_frame_has_stable_shape_and_no_payload() {
        let frame = ErrorFrame::new(
            7,
            ErrorBody {
                code: "HAVEN_BROKER_QUOTA_EXCEEDED".into(),
                message: "已超出配额。".into(),
                retryable: false,
            },
        );
        let value: Value = serde_json::from_str(&encode_server_frame(&frame).unwrap()).unwrap();
        assert_eq!(value["type"], "error");
        assert_eq!(value["id"], 7);
        assert_eq!(value["error"]["code"], "HAVEN_BROKER_QUOTA_EXCEEDED");
        assert_eq!(value["error"]["retryable"], false);
        assert!(value.get("payload").is_none(), "错误帧不得携带 payload");
    }

    #[test]
    fn response_limit_is_enforced_without_truncation() {
        let huge = OkFrame::new(
            1,
            serde_json::json!({ "pad": "y".repeat(MAX_RESPONSE_BYTES) }),
        );
        assert_eq!(
            encode_server_frame(&huge).unwrap_err().code(),
            "HAVEN_BROKER_RESPONSE_TOO_LARGE"
        );
    }
}
