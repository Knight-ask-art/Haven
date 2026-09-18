//! AI Provider Profile（非敏感配置）+ 模型目录投影。
//!
//! 规范：[`docs/architecture/AI_SYSTEM.md`](../../../../docs/architecture/AI_SYSTEM.md) §3、§7、§8。
//!
//! 安全边界（本模块全部职责）：
//! - **secret 不在这里**。`AiProviderProfile` 只有 id / 显示名 / kind / endpoint /
//!   enabled / selectedModelId 与审计时间；API key 只存在于 CredentialStore
//!   （`haven:ai:<profile-id>`）。因此本类型可以安全地进 SQLite、进 wire、进日志。
//! - **endpoint 必须来自用户显式配置且通过共享 HTTP URL 策略**。拒绝非 http(s)、
//!   userinfo、fragment、query、单标签主机、非公网字面地址与超范围端口；不做任何
//!   局域网/服务发现。
//! - **能力不猜**。`AiModelCapability` 是三态闭合集合，第三方响应缺字段即 `Unknown`；
//!   模型名（`gpt-4o`、`*-vision`、`text-embedding-*`）不构成任何能力证据。

use haven_common::network::{HttpUrlError, HttpUrlPolicy, parse_http_url};
use haven_common::{AppError, ErrorKind};

/// Provider 种类（闭合集合）。第一版只支持 OpenAI 兼容协议。
///
/// 新增种类必须同时补齐：Provider 适配器、模型发现路径、wire 枚举与 UI 文案；
/// 仅在这里加变体会让所有 `match` 编译失败，这是有意的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AiProviderKind {
    OpenAiCompatible,
}

impl AiProviderKind {
    /// 持久化 / wire 使用的稳定字符串（snake_case）。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiCompatible => "openai_compatible",
        }
    }

    /// 解析持久化字符串；未知值必须拒绝（脏行不得静默降级为默认种类）。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "openai_compatible" => Some(Self::OpenAiCompatible),
            _ => None,
        }
    }
}

/// profile id 上限。与 `CredentialRef` 的 profile 段上限（60）保持一致，
/// 保证任何合法 profile 都能构造出合法的凭据 target。
pub const AI_PROVIDER_PROFILE_ID_MAX_LEN: usize = 60;
/// 显示名上限（短文本，防滥用；不参与任何寻址语义）。
pub const AI_PROVIDER_DISPLAY_NAME_MAX_LEN: usize = 80;
/// 模型 id 上限。Provider 返回的 id 是外部输入，必须限长后才进 wire。
pub const AI_PROVIDER_MODEL_ID_MAX_LEN: usize = 200;
/// 模型显示名上限。
pub const AI_PROVIDER_MODEL_NAME_MAX_LEN: usize = 200;
/// `ownedBy` 上限。
pub const AI_PROVIDER_MODEL_OWNER_MAX_LEN: usize = 120;

/// 一个非敏感的 Provider Profile。
///
/// `revision` 是 CAS 版本号：`None` 表示该行尚未持久化。写入路径必须携带
/// 期望版本，`cas_upsert` 以此为条件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiProviderProfile {
    profile_id: String,
    display_name: String,
    kind: AiProviderKind,
    endpoint: String,
    enabled: bool,
    selected_model_id: Option<String>,
    revision: Option<String>,
    created_at: i64,
    updated_at: i64,
}

impl AiProviderProfile {
    /// 构造一个新的（尚未持久化的）profile：`revision = None`。
    ///
    /// 所有字段都经过 [`validate_profile_id`] / [`validate_display_name`] /
    /// [`validate_endpoint`] / [`validate_model_id`] 校验；任何一个不合法都拒绝构造，
    /// 因此不存在"半合法 profile 被写进库"的中间态。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        profile_id: impl Into<String>,
        display_name: impl Into<String>,
        kind: AiProviderKind,
        endpoint: impl Into<String>,
        enabled: bool,
        selected_model_id: Option<String>,
        now_ms: i64,
    ) -> Result<Self, AppError> {
        let profile_id = profile_id.into();
        let display_name = display_name.into();
        let endpoint = endpoint.into();
        validate_profile_id(&profile_id)?;
        validate_display_name(&display_name)?;
        let endpoint = validate_endpoint(&endpoint)?;
        if let Some(model_id) = selected_model_id.as_deref() {
            validate_model_id(model_id)?;
        }
        Ok(Self {
            profile_id,
            display_name,
            kind,
            endpoint,
            enabled,
            selected_model_id,
            revision: None,
            created_at: now_ms,
            updated_at: now_ms,
        })
    }

    /// 从持久化行恢复。脏行（非法 id / 非法 endpoint / 未知 kind）必须拒绝：
    /// 一个无法通过校验的 endpoint 不允许被当成"可请求目标"继续流通。
    #[allow(clippy::too_many_arguments)]
    pub fn from_stored(
        profile_id: String,
        display_name: String,
        kind: AiProviderKind,
        endpoint: String,
        enabled: bool,
        selected_model_id: Option<String>,
        revision: String,
        created_at: i64,
        updated_at: i64,
    ) -> Result<Self, AppError> {
        validate_profile_id(&profile_id)?;
        validate_display_name(&display_name)?;
        let endpoint = validate_endpoint(&endpoint)?;
        if let Some(model_id) = selected_model_id.as_deref() {
            validate_model_id(model_id)?;
        }
        if revision.is_empty() {
            return Err(profile_error(
                "AI_PROVIDER_REVISION_INVALID",
                "profile 版本号为空",
            ));
        }
        Ok(Self {
            profile_id,
            display_name,
            kind,
            endpoint,
            enabled,
            selected_model_id,
            revision: Some(revision),
            created_at,
            updated_at,
        })
    }

    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn kind(&self) -> AiProviderKind {
        self.kind
    }

    /// 规范化后的端点（已通过 URL 策略）。**不含** secret。
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn selected_model_id(&self) -> Option<&str> {
        self.selected_model_id.as_deref()
    }

    pub fn revision(&self) -> Option<&str> {
        self.revision.as_deref()
    }

    pub fn created_at(&self) -> i64 {
        self.created_at
    }

    pub fn updated_at(&self) -> i64 {
        self.updated_at
    }

    /// 模型发现要使用的 URL：在 profile.endpoint 下解析兼容 `/models`。
    ///
    /// - `https://gw.example/v1` → `https://gw.example/v1/models`
    /// - `https://gw.example/v1/` → `https://gw.example/v1/models`（去重复斜杠）
    /// - `https://gw.example/v1/models` → `https://gw.example/v1/models`（不重复追加）
    ///
    /// 结果仍会重新过一遍 URL 策略，因此拼接不可能把请求带出策略之外。
    pub fn models_endpoint(&self) -> Result<String, AppError> {
        models_url_for(&self.endpoint)
    }
}

/// profile 删除结果（CAS 语义显式化，避免用 `bool` 混淆"不存在"与"版本冲突"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiProviderProfileDeleteOutcome {
    /// 已按期望版本删除。
    Deleted,
    /// 不存在（幂等删除的成功态由调用方决定）。
    NotFound,
    /// 版本不匹配：存在但未删除。
    RevisionConflict,
}

// ---------- 校验 ----------

/// 校验 profile id：稳定、可寻址、可安全拼进凭据 target。
///
/// 规则：非空、≤ 60、只允许 ASCII 小写字母/数字/`-`/`_`，且必须字母或数字开头。
/// 这条规则同时排除了控制字符、空白、路径分隔符、`:`（凭据 target 分隔符）、
/// `.`（避免 `..` 之类的相对路径形状）与任何非 ASCII 混淆字符。
pub fn validate_profile_id(value: &str) -> Result<(), AppError> {
    if value.is_empty() {
        return Err(profile_error(
            "AI_PROVIDER_PROFILE_ID_INVALID",
            "profile id 不能为空",
        ));
    }
    if value.len() > AI_PROVIDER_PROFILE_ID_MAX_LEN {
        return Err(profile_error(
            "AI_PROVIDER_PROFILE_ID_INVALID",
            "profile id 超长",
        ));
    }
    let mut chars = value.chars();
    let first = chars.next().expect("已确认非空");
    if !first.is_ascii_alphanumeric() {
        return Err(profile_error(
            "AI_PROVIDER_PROFILE_ID_INVALID",
            "profile id 必须以 ASCII 字母或数字开头",
        ));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(profile_error(
            "AI_PROVIDER_PROFILE_ID_INVALID",
            "profile id 只允许 ASCII 字母、数字、连字符与下划线",
        ));
    }
    Ok(())
}

/// 校验显示名：短文本，允许非 ASCII，但拒绝控制字符与路径分隔符。
pub fn validate_display_name(value: &str) -> Result<(), AppError> {
    if value.trim().is_empty() {
        return Err(profile_error(
            "AI_PROVIDER_DISPLAY_NAME_INVALID",
            "显示名不能为空",
        ));
    }
    if value.len() > AI_PROVIDER_DISPLAY_NAME_MAX_LEN {
        return Err(profile_error(
            "AI_PROVIDER_DISPLAY_NAME_INVALID",
            "显示名超长",
        ));
    }
    if value
        .chars()
        .any(|c| c.is_control() || c == '/' || c == '\\' || c == ':')
    {
        return Err(profile_error(
            "AI_PROVIDER_DISPLAY_NAME_INVALID",
            "显示名不得包含控制字符或路径分隔符",
        ));
    }
    Ok(())
}

/// 校验模型 id：来自 Provider，是外部输入，必须限长且不得带控制字符。
pub fn validate_model_id(value: &str) -> Result<(), AppError> {
    if value.trim().is_empty() {
        return Err(profile_error(
            "AI_PROVIDER_MODEL_ID_INVALID",
            "模型 id 不能为空",
        ));
    }
    if value.len() > AI_PROVIDER_MODEL_ID_MAX_LEN {
        return Err(profile_error(
            "AI_PROVIDER_MODEL_ID_INVALID",
            "模型 id 超长",
        ));
    }
    if value
        .chars()
        .any(|c| c.is_control() || c == '/' || c == '\\' || c.is_whitespace())
    {
        return Err(profile_error(
            "AI_PROVIDER_MODEL_ID_INVALID",
            "模型 id 不得包含控制字符、空白或路径分隔符",
        ));
    }
    Ok(())
}

/// 校验并规范化 endpoint。
///
/// 在共享 URL 策略之上再收紧两点（本切片专用）：
/// - 拒绝 query：模型发现要在 endpoint 下拼路径，带 query 的 base 语义不明确，
///   而且 `?key=...` 是常见的"把 secret 塞进 URL"的形态。
/// - 拒绝本地路径形状（`C:\...`、`\\server\share`、`/etc/...`）：这些根本不是 URL，
///   早期拒绝比让 `Url::parse` 报一个含糊的错误更可诊断。
pub fn validate_endpoint(value: &str) -> Result<String, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(profile_error(
            "AI_PROVIDER_ENDPOINT_INVALID",
            "API 地址不能为空",
        ));
    }
    if looks_like_local_path(trimmed) {
        return Err(profile_error(
            "AI_PROVIDER_ENDPOINT_INVALID",
            "API 地址必须是 http/https URL，不能是本地路径",
        ));
    }
    let safe =
        parse_http_url(trimmed, HttpUrlPolicy::AiProviderEndpoint).map_err(endpoint_error)?;
    if safe.as_url().query().is_some() {
        return Err(profile_error(
            "AI_PROVIDER_ENDPOINT_INVALID",
            "API 地址不得包含查询参数",
        ));
    }
    Ok(safe.as_str().trim_end_matches('/').to_owned())
}

/// 在已规范化的 endpoint 下拼 `/models`，不重复 `/v1`、不重复 `/models`。
pub fn models_url_for(endpoint: &str) -> Result<String, AppError> {
    let base = validate_endpoint(endpoint)?;
    let candidate = if base.ends_with("/models") {
        base
    } else {
        format!("{base}/models")
    };
    // 拼接结果必须仍在策略之内（防御未来放宽 endpoint 规则时拼接绕过策略）。
    parse_http_url(&candidate, HttpUrlPolicy::AiProviderEndpoint).map_err(endpoint_error)?;
    Ok(candidate)
}

fn looks_like_local_path(value: &str) -> bool {
    if value.starts_with('/') || value.starts_with('\\') {
        return true;
    }
    // Windows 盘符：`C:\...` / `C:/...`。
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

/// URL 策略错误 → 稳定、可行动的 INVALID_ARGUMENT。
///
/// **不**回显被拒绝的 URL：错误消息本身也是"secret 不得出 wire"边界的一部分。
fn endpoint_error(error: HttpUrlError) -> AppError {
    let detail = match error {
        HttpUrlError::InvalidUrl => "API 地址不是合法的 URL",
        HttpUrlError::UnsupportedScheme => "API 地址只支持 http 或 https",
        HttpUrlError::MissingHost => "API 地址缺少主机名",
        HttpUrlError::UserInfo => "API 地址不得内嵌用户名或密码",
        HttpUrlError::Fragment => "API 地址不得包含片段",
        HttpUrlError::UnsafeHost => "API 地址的地址段不被允许（仅接受公网主机）",
        HttpUrlError::DisallowedPort => "API 地址的端口不被允许（仅 80/443/8080/8443）",
    };
    profile_error("AI_PROVIDER_ENDPOINT_INVALID", detail)
}

fn profile_error(code: &'static str, message: &'static str) -> AppError {
    AppError::new(code, ErrorKind::Validation, message, false)
}

// ---------- 模型目录 ----------

/// 模型能力三态。
///
/// **必须**是三态：Provider 出于兼容考虑常常不声明能力，把"没声明"读成"不支持"
/// 会让用户看不到本来可用的模型；读成"支持"则是伪造。缺字段一律 `Unknown`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiModelCapability {
    Supported,
    Unsupported,
    Unknown,
}

impl AiModelCapability {
    /// 从显式布尔字段映射；`None`（字段缺失）→ `Unknown`。
    pub const fn from_declared(value: Option<bool>) -> Self {
        match value {
            Some(true) => Self::Supported,
            Some(false) => Self::Unsupported,
            None => Self::Unknown,
        }
    }
}

/// 一条模型目录记录的严格投影。
///
/// 只承载"Provider 确实声明了"的事实：`display_name` / `created` / `owned_by`
/// 缺失即 `None`，能力缺失即 `Unknown`。**不**从 `model_id` 推断任何能力。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiModelDescriptor {
    pub model_id: String,
    pub display_name: Option<String>,
    /// Provider 自报的创建时间（Unix 秒）。语义由 Provider 定义，Haven 不解释它。
    pub created: Option<i64>,
    pub owned_by: Option<String>,
    pub chat: AiModelCapability,
    pub vision: AiModelCapability,
    pub embedding: AiModelCapability,
}

impl AiModelDescriptor {
    /// 构造并校验一条模型记录。非法的 `model_id` 直接拒绝，避免脏值进 wire。
    pub fn new(
        model_id: String,
        display_name: Option<String>,
        created: Option<i64>,
        owned_by: Option<String>,
        chat: AiModelCapability,
        vision: AiModelCapability,
        embedding: AiModelCapability,
    ) -> Result<Self, AppError> {
        validate_model_id(&model_id)?;
        Ok(Self {
            model_id,
            display_name: truncate_optional(display_name, AI_PROVIDER_MODEL_NAME_MAX_LEN),
            created,
            owned_by: truncate_optional(owned_by, AI_PROVIDER_MODEL_OWNER_MAX_LEN),
            chat,
            vision,
            embedding,
        })
    }
}

/// Provider 自报的可选字符串字段：清理控制字符、限长；清空后视为缺失。
///
/// 第三方响应里的显示名/所有者是任意文本，可能是攻击面（终端转义序列、超长串）。
/// 这里统一做"要么给出干净短文本，要么当作没有"的收敛。
fn truncate_optional(value: Option<String>, max_len: usize) -> Option<String> {
    let value = value?;
    let cleaned: String = value.chars().filter(|c| !c.is_control()).collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() <= max_len {
        return Some(trimmed.to_owned());
    }
    // 按字符边界截断，避免切断 UTF-8 序列。
    let mut end = max_len;
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    Some(trimmed[..end].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(endpoint: &str) -> Result<AiProviderProfile, AppError> {
        AiProviderProfile::new(
            "gw-main",
            "自建网关",
            AiProviderKind::OpenAiCompatible,
            endpoint,
            true,
            None,
            1_700_000_000_000,
        )
    }

    #[test]
    fn kind_is_closed_and_round_trips() {
        assert_eq!(
            AiProviderKind::OpenAiCompatible.as_str(),
            "openai_compatible"
        );
        assert_eq!(
            AiProviderKind::parse("openai_compatible"),
            Some(AiProviderKind::OpenAiCompatible)
        );
        for unknown in ["anthropic", "OpenAI_Compatible", "", "openai.compatible"] {
            assert!(
                AiProviderKind::parse(unknown).is_none(),
                "未知 kind 必须拒绝: {unknown:?}"
            );
        }
    }

    #[test]
    fn valid_profile_normalizes_endpoint_trailing_slash() {
        let value = profile("https://gateway.example.invalid/v1/").unwrap();
        assert_eq!(value.endpoint(), "https://gateway.example.invalid/v1");
        assert_eq!(value.kind(), AiProviderKind::OpenAiCompatible);
        assert!(value.revision().is_none(), "新建 profile 尚未持久化");
        assert_eq!(
            value.models_endpoint().unwrap(),
            "https://gateway.example.invalid/v1/models"
        );
    }

    #[test]
    fn models_endpoint_does_not_duplicate_segments() {
        assert_eq!(
            models_url_for("https://gateway.example.invalid/v1").unwrap(),
            "https://gateway.example.invalid/v1/models"
        );
        assert_eq!(
            models_url_for("https://gateway.example.invalid").unwrap(),
            "https://gateway.example.invalid/models"
        );
        assert_eq!(
            models_url_for("https://gateway.example.invalid/v1/models").unwrap(),
            "https://gateway.example.invalid/v1/models"
        );
    }

    #[test]
    fn profile_id_rejects_paths_control_chars_and_credential_shapes() {
        for invalid in [
            "",
            " ",
            "-leading-dash",
            "_leading-underscore",
            "has space",
            "has/slash",
            "has\\backslash",
            "has:colon",
            "has.dot",
            "has\r\nnewline",
            "..",
            "profile\u{0}id",
            "配置文件",
            &"a".repeat(AI_PROVIDER_PROFILE_ID_MAX_LEN + 1),
        ] {
            assert!(
                validate_profile_id(invalid).is_err(),
                "非法 profile id 必须拒绝: {invalid:?}"
            );
        }
        for valid in [
            "gw",
            "gw-1",
            "gw_1",
            "0",
            "a".repeat(AI_PROVIDER_PROFILE_ID_MAX_LEN).as_str(),
        ] {
            assert!(
                validate_profile_id(valid).is_ok(),
                "合法 profile id: {valid:?}"
            );
        }
    }

    #[test]
    fn display_name_rejects_paths_and_control_chars() {
        for invalid in ["", "   ", "a/b", "a\\b", "a\tb", "c:name"] {
            assert!(
                validate_display_name(invalid).is_err(),
                "非法显示名必须拒绝: {invalid:?}"
            );
        }
        assert!(validate_display_name("自建网关").is_ok());
    }

    #[test]
    fn model_id_rejects_control_chars_and_paths() {
        for invalid in ["", " ", "a b", "a/b", "a\\b", "a\u{7}b"] {
            assert!(
                validate_model_id(invalid).is_err(),
                "非法模型 id 必须拒绝: {invalid:?}"
            );
        }
        assert!(validate_model_id("gpt-4o-mini").is_ok());
    }

    #[test]
    fn endpoint_rejects_local_paths_and_bad_schemes() {
        for invalid in [
            "",
            "   ",
            "C:\\models\\gateway",
            "c:/gateway",
            "\\\\server\\share",
            "/var/run/gateway.sock",
            "ftp://gateway.example.invalid",
            "file:///etc/passwd",
            "ws://gateway.example.invalid",
            "https://user:secret@gateway.example.invalid/v1",
            "https://gateway.example.invalid/v1?key=secret",
            "https://gateway.example.invalid/v1#frag",
            "https://localhost/v1",
            "https://gateway/v1",
            "https://127.0.0.1:11434/v1",
            "https://192.168.1.10/v1",
            "https://gateway.example.invalid:12345/v1",
        ] {
            let error = validate_endpoint(invalid).unwrap_err();
            assert_eq!(
                error.code().as_str(),
                "AI_PROVIDER_ENDPOINT_INVALID",
                "端点必须被稳定拒绝: {invalid:?}"
            );
            // 错误消息不得回显被拒绝的 URL（可能内嵌凭据）。
            assert!(
                !error.user_message().contains("secret"),
                "错误消息不得回显端点内容: {invalid:?}"
            );
        }
    }

    #[test]
    fn endpoint_accepts_http_and_https_public_hosts() {
        for valid in [
            "https://gateway.example.invalid/v1",
            "http://gateway.example.invalid:8080/openai",
            "https://gateway.example.invalid:8443/compatible/v1",
        ] {
            assert!(validate_endpoint(valid).is_ok(), "合法端点: {valid:?}");
        }
    }

    #[test]
    fn from_stored_rejects_dirty_rows() {
        assert!(
            AiProviderProfile::from_stored(
                "gw".into(),
                "网关".into(),
                AiProviderKind::OpenAiCompatible,
                "https://127.0.0.1/v1".into(),
                true,
                None,
                "rev-1".into(),
                1,
                1,
            )
            .is_err(),
            "脏 endpoint 不得从存储恢复"
        );
        assert!(
            AiProviderProfile::from_stored(
                "gw".into(),
                "网关".into(),
                AiProviderKind::OpenAiCompatible,
                "https://gateway.example.invalid/v1".into(),
                true,
                None,
                String::new(),
                1,
                1,
            )
            .is_err(),
            "空 revision 不得从存储恢复"
        );
    }

    #[test]
    fn capability_never_infers_from_missing_or_name() {
        assert_eq!(
            AiModelCapability::from_declared(None),
            AiModelCapability::Unknown
        );
        assert_eq!(
            AiModelCapability::from_declared(Some(true)),
            AiModelCapability::Supported
        );
        assert_eq!(
            AiModelCapability::from_declared(Some(false)),
            AiModelCapability::Unsupported
        );

        // 一个名字里带 "vision" 的模型，没有任何能力声明 → 仍然 unknown。
        let descriptor = AiModelDescriptor::new(
            "gpt-4o-vision-preview".into(),
            None,
            None,
            None,
            AiModelCapability::Unknown,
            AiModelCapability::Unknown,
            AiModelCapability::Unknown,
        )
        .unwrap();
        assert_eq!(descriptor.vision, AiModelCapability::Unknown);
        assert_eq!(descriptor.embedding, AiModelCapability::Unknown);
        assert_eq!(descriptor.chat, AiModelCapability::Unknown);
    }

    #[test]
    fn descriptor_sanitizes_optional_text() {
        let descriptor = AiModelDescriptor::new(
            "gte-base".into(),
            Some("上面\u{1b}[31m红字\u{7}".into()),
            Some(1_700_000_000),
            Some("   ".into()),
            AiModelCapability::Unknown,
            AiModelCapability::Unknown,
            AiModelCapability::Supported,
        )
        .unwrap();
        assert_eq!(descriptor.display_name.as_deref(), Some("上面[31m红字"));
        assert_eq!(descriptor.owned_by, None, "纯空白视为缺失");
        assert_eq!(descriptor.embedding, AiModelCapability::Supported);
    }

    #[test]
    fn descriptor_truncates_on_char_boundary() {
        let long = "模".repeat(AI_PROVIDER_MODEL_NAME_MAX_LEN);
        let descriptor = AiModelDescriptor::new(
            "model".into(),
            Some(long),
            None,
            None,
            AiModelCapability::Unknown,
            AiModelCapability::Unknown,
            AiModelCapability::Unknown,
        )
        .unwrap();
        let name = descriptor.display_name.expect("截断后仍有值");
        assert!(name.len() <= AI_PROVIDER_MODEL_NAME_MAX_LEN);
        assert!(name.chars().all(|c| c == '模'), "不得截断出非法 UTF-8");
    }
}
