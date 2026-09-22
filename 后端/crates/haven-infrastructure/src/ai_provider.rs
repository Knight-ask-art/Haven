//! OpenAI 兼容 Provider 的模型发现适配器（A2 基础切片）。
//!
//! 规范：[`docs/architecture/AI_SYSTEM.md`](../../../../docs/architecture/AI_SYSTEM.md) §7、§8。
//!
//! 这是本切片里**唯一**由用户配置目标地址的出站请求。它复用项目既有的出站约束：
//! - 端点先过 `HttpUrlPolicy::AiProviderEndpoint`（http/https、无 userinfo / fragment、
//!   多标签主机、公网字面地址、允许端口集合）；
//! - 每次请求重新解析 DNS，任一结果非公网即整体拒绝，并把地址固定到连接上
//!   （`resolve_public_http_target` + `pin_client_builder`）；
//! - **不跟随重定向**：3xx 一律当作配置错误返回，而不是"再解析一个新主机"；
//! - 有界的连接/请求超时与响应体大小。
//!
//! API key 只在本模块的调用栈内以 `SecretString` 存在，作为 `Authorization: Bearer`
//! 头一次性使用，随后随栈释放。它不进入 URL、日志、错误消息或任何返回值。
//!
//! 能力绝不从模型名推断：响应里没有显式能力字段就是 `unknown`。
//! `gpt-4o` / `*-vision` / `text-embedding-*` 这类名字不构成任何证据。

use std::time::Duration;

use async_trait::async_trait;

use haven_application::services::ai_provider::{
    AiModelCatalogPort, AiSettingsRecommendationInput, AiSettingsRecommendationPort,
};
use haven_application::wire::PreferenceReadingPatchDto;
use haven_common::network::{HttpUrlPolicy, parse_http_url};
use haven_common::{AppError, ErrorKind};
use haven_domain::ai_provider::{
    AI_PROVIDER_MODEL_ID_MAX_LEN, AI_RECOMMENDATION_EXPLANATION_MAX_LEN,
    AI_RECOMMENDATION_TEXT_FIELD_MAX_LEN, AiModelCapability, AiModelDescriptor, AiProviderKind,
    AiSettingsRecommendation, chat_completions_url_for, models_url_for,
};
use haven_domain::credential::SecretString;
use haven_domain::settings::ReadingPatch;
use serde_json::Value;

use crate::http_security::{pin_client_builder, resolve_public_http_target};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// 模型目录是小型 JSON；1 MiB 足够容纳数千条记录，同时把"被塞一个大响应"挡在外面。
const MAX_MODELS_BYTES: usize = 1024 * 1024;
const USER_AGENT: &str = "Haven/0.1.0 (ai provider model discovery)";
const MAX_RECOMMENDATION_BYTES: usize = 256 * 1024;
const MAX_RECOMMENDATION_PROMPT_BYTES: usize = 8 * 1024;
const RECOMMENDATION_TIMEOUT: Duration = Duration::from_secs(30);
const RECOMMENDATION_USER_AGENT: &str = "Haven/0.1.0 (ai provider settings recommendation)";
const RECOMMENDATION_SYSTEM_PROMPT: &str = "你是 Haven 阅读设置助手。根据给定的当前阅读设置，只输出一个 JSON 对象：{\"settingsPatch\": {<需要调整的字段，不调整的字段为 null>}, \"explanation\": <简短中文说明或 null>}。只能使用 schema 允许的字段与取值；不要输出多余字段、不要调用任何工具、不要执行任何操作。";

/// OpenAI 兼容的模型发现客户端。
#[derive(Clone, Default)]
pub struct OpenAiCompatibleModelCatalog;

impl OpenAiCompatibleModelCatalog {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AiModelCatalogPort for OpenAiCompatibleModelCatalog {
    async fn list_models(
        &self,
        kind: AiProviderKind,
        endpoint: &str,
        secret: &SecretString,
    ) -> Result<Vec<AiModelDescriptor>, AppError> {
        match kind {
            AiProviderKind::OpenAiCompatible => {}
        }
        let url = models_url_for(endpoint)?;
        // 解析 + 固定地址；每次调用都重新解析，因此不会把一次解析结果当作长期授权。
        let target = resolve_public_http_target(&url, HttpUrlPolicy::AiProviderEndpoint)
            .await
            .map_err(target_error)?;
        let builder = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .user_agent(USER_AGENT)
            .http1_only()
            // 用户配置的端点不该把请求带去另一个主机；重定向是配置错误，不是要跟随的语义。
            .redirect(reqwest::redirect::Policy::none());
        let client = pin_client_builder(builder, &target)
            .build()
            .map_err(|_| client_error())?;

        let mut response = client
            .get(target.url.clone())
            .header(reqwest::header::ACCEPT, "application/json")
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", secret.expose()),
            )
            .send()
            .await
            .map_err(|_| unreachable_error())?;

        let status = response.status().as_u16();
        // 先看声明长度（廉价拒绝），再按块累积并逐块校验。
        // 只做 `bytes()` 之后再检查长度是**不够**的：声明长度为空的 chunked 响应会先被
        // 完整读进内存，上界形同虚设。
        if response
            .content_length()
            .is_some_and(|length| length > MAX_MODELS_BYTES as u64)
        {
            return Err(response_too_large());
        }
        let mut body: Vec<u8> = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| unreachable_error())? {
            extend_with_cap(&mut body, &chunk)?;
        }
        interpret_models_response(status, &body)
    }
}

/// 按块累积响应体，并在**每一块**上强制上界。
///
/// 拆成独立函数是为了让"越界即拒绝"这条判据可以在没有服务器的情况下被覆盖：
/// 它是这条路径上唯一防止 chunked 响应撑爆内存的东西。
fn extend_with_cap(body: &mut Vec<u8>, chunk: &[u8]) -> Result<(), AppError> {
    if body.len() + chunk.len() > MAX_MODELS_BYTES {
        return Err(response_too_large());
    }
    body.extend_from_slice(chunk);
    Ok(())
}

/// 把一次 `/models` 响应解释成模型目录。
///
/// 拆成纯函数是刻意的：状态码与响应体的映射是这条路径上最容易写错的逻辑，
/// 而它不需要网络就能被完整覆盖（成功 / 非 2xx / 非法 JSON / 缺能力字段）。
pub(crate) fn interpret_models_response(
    status: u16,
    body: &[u8],
) -> Result<Vec<AiModelDescriptor>, AppError> {
    if !(200..300).contains(&status) {
        return Err(status_error(status));
    }
    let value: Value =
        serde_json::from_slice(body).map_err(|_| invalid_response("响应不是合法 JSON"))?;
    parse_models_payload(&value)
}

/// 解析 OpenAI 兼容 `{ "data": [ { "id": ... } ] }` 载荷。
///
/// 严格规则：
/// - 顶层必须是对象且带 `data` 数组；`object: "list"` 若存在必须是字符串（不强制，
///   因为并非所有兼容实现都返回它）。
/// - 每条记录的 `id` 必须是非空字符串且通过模型 id 校验；缺失或非法的记录被**跳过**
///   而不是伪造一个占位 id。
/// - `name` / `created` / `owned_by` 缺失即 `None`。
/// - 能力只来自显式的 `capabilities` 对象；缺失即 `Unknown`。
fn parse_models_payload(value: &Value) -> Result<Vec<AiModelDescriptor>, AppError> {
    let data = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_response("响应缺少 data 数组"))?;
    let mut models = Vec::with_capacity(data.len());
    for entry in data {
        let Some(object) = entry.as_object() else {
            continue;
        };
        let Some(model_id) = object.get("id").and_then(Value::as_str) else {
            continue;
        };
        if model_id.len() > AI_PROVIDER_MODEL_ID_MAX_LEN {
            continue;
        }
        let capabilities = object.get("capabilities").and_then(Value::as_object);
        let declared = |key: &str| {
            capabilities
                .and_then(|map| map.get(key))
                .and_then(Value::as_bool)
        };
        let descriptor = AiModelDescriptor::new(
            model_id.to_owned(),
            object
                .get("name")
                .or_else(|| object.get("display_name"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            object.get("created").and_then(Value::as_i64),
            object
                .get("owned_by")
                .and_then(Value::as_str)
                .map(str::to_owned),
            AiModelCapability::from_declared(declared("chat")),
            AiModelCapability::from_declared(declared("vision")),
            AiModelCapability::from_declared(declared("embedding")),
        );
        // 非法 model_id 只是被跳过：一条坏记录不该让整份目录不可用，但它也绝不会
        // 以"被清洗过"的形式出现在结果里。
        if let Ok(descriptor) = descriptor {
            models.push(descriptor);
        }
    }
    Ok(models)
}

fn status_error(status: u16) -> AppError {
    if status == 401 || status == 403 {
        return AppError::new(
            "AI_PROVIDER_MODELS_UNAUTHORIZED",
            ErrorKind::Unauthorized,
            "API Key 无效或没有读取模型列表的权限",
            false,
        );
    }
    if (300..400).contains(&status) {
        return AppError::new(
            "AI_PROVIDER_MODELS_UNEXPECTED_REDIRECT",
            ErrorKind::Network,
            "模型目录接口返回了重定向；请把 API 地址改为最终地址",
            false,
        );
    }
    AppError::new(
        "AI_PROVIDER_MODELS_HTTP_ERROR",
        ErrorKind::Network,
        format!("模型目录请求失败（HTTP {status}）"),
        true,
    )
}

fn invalid_response(detail: &'static str) -> AppError {
    AppError::new(
        "AI_PROVIDER_MODELS_INVALID_RESPONSE",
        ErrorKind::Parse,
        detail,
        false,
    )
}

fn response_too_large() -> AppError {
    AppError::new(
        "AI_PROVIDER_MODELS_RESPONSE_TOO_LARGE",
        ErrorKind::Network,
        "模型目录响应超出大小上限",
        false,
    )
}

fn unreachable_error() -> AppError {
    AppError::new(
        "AI_PROVIDER_MODELS_UNREACHABLE",
        ErrorKind::Network,
        "无法连接到模型目录接口，请检查 API 地址与网络",
        true,
    )
}

fn target_error(error: crate::http_security::HttpTargetError) -> AppError {
    use crate::http_security::HttpTargetError as Error;
    match error {
        Error::Invalid | Error::UnsafeAddress => AppError::new(
            "AI_PROVIDER_ENDPOINT_INVALID",
            ErrorKind::Validation,
            "API 地址不被允许（仅接受公网 http/https 主机）",
            false,
        ),
        Error::ResolveFailed => AppError::new(
            "AI_PROVIDER_MODELS_UNREACHABLE",
            ErrorKind::Network,
            "无法解析 API 地址",
            true,
        ),
    }
}

fn client_error() -> AppError {
    AppError::new(
        "AI_PROVIDER_MODELS_CLIENT_INIT_FAILED",
        ErrorKind::Internal,
        "模型发现客户端初始化失败",
        false,
    )
}

/// OpenAI 兼容的**设置建议**客户端。
///
/// 与模型发现适配器同源：同样的 URL 策略、DNS 每个请求重新解析并固定、关闭重定向、
/// 有界超时与响应上界。模型只能返回 typed 建议值；Proposal 的创建仍由 Application
/// service 完成，Provider 没有本地写入能力。
#[derive(Clone, Default)]
pub struct OpenAiCompatibleSettingsRecommender;

impl OpenAiCompatibleSettingsRecommender {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AiSettingsRecommendationPort for OpenAiCompatibleSettingsRecommender {
    async fn recommend_settings(
        &self,
        kind: AiProviderKind,
        endpoint: &str,
        secret: &SecretString,
        input: &AiSettingsRecommendationInput,
    ) -> Result<AiSettingsRecommendation, AppError> {
        match kind {
            AiProviderKind::OpenAiCompatible => {}
        }
        let body = build_recommendation_request(input)?;
        let url = chat_completions_url_for(endpoint)?;
        let target = resolve_public_http_target(&url, HttpUrlPolicy::AiProviderEndpoint)
            .await
            .map_err(recommendation_target_error)?;
        let builder = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(RECOMMENDATION_TIMEOUT)
            .user_agent(RECOMMENDATION_USER_AGENT)
            .http1_only()
            .redirect(reqwest::redirect::Policy::none());
        let client = pin_client_builder(builder, &target)
            .build()
            .map_err(|_| recommendation_client_error())?;

        let mut response = client
            .post(target.url.clone())
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", secret.expose()),
            )
            .json(&body)
            .send()
            .await
            .map_err(|_| recommendation_unreachable())?;

        let status = response.status().as_u16();
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RECOMMENDATION_BYTES as u64)
        {
            return Err(recommendation_response_too_large());
        }
        let mut raw = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| recommendation_unreachable())?
        {
            extend_with_recommendation_cap(&mut raw, &chunk)?;
        }
        interpret_recommendation_response(status, &raw)
    }
}

fn extend_with_recommendation_cap(body: &mut Vec<u8>, chunk: &[u8]) -> Result<(), AppError> {
    if body.len() + chunk.len() > MAX_RECOMMENDATION_BYTES {
        return Err(recommendation_response_too_large());
    }
    body.extend_from_slice(chunk);
    Ok(())
}

/// 构造严格 JSON 请求体。提示词只包含模型 id 与已脱敏的阅读快照；超界即拒绝，
/// 不发起出站请求。
fn build_recommendation_request(input: &AiSettingsRecommendationInput) -> Result<Value, AppError> {
    let reading =
        serde_json::to_value(&input.reading).map_err(|_| recommendation_invalid_input())?;
    let prompt = serde_json::json!({
        "task": "haven_reading_settings_recommendation",
        "model": input.model_id,
        "currentReading": reading,
    })
    .to_string();
    if prompt.len() > MAX_RECOMMENDATION_PROMPT_BYTES {
        return Err(recommendation_invalid_input());
    }
    Ok(serde_json::json!({
        "model": input.model_id,
        "temperature": 0,
        "max_tokens": 512,
        "messages": [
            { "role": "system", "content": RECOMMENDATION_SYSTEM_PROMPT },
            { "role": "user", "content": prompt }
        ],
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "haven_settings_recommendation",
                "strict": true,
                "schema": recommendation_json_schema(),
            }
        }
    }))
}

fn nullable_string(max_len: usize) -> Value {
    serde_json::json!({ "type": ["string", "null"], "maxLength": max_len })
}

fn nullable_enum(values: &[&str]) -> Value {
    let mut variants: Vec<Value> = values
        .iter()
        .map(|value| Value::String((*value).to_owned()))
        .collect();
    variants.push(Value::Null);
    serde_json::json!({ "type": ["string", "null"], "enum": variants })
}

fn recommendation_json_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["settingsPatch", "explanation"],
        "properties": {
            "settingsPatch": {
                "type": "object",
                "additionalProperties": false,
                "required": [
                    "fontFamily", "customFontFamily", "fontSize", "lineHeight",
                    "contentWidth", "theme", "customBackground", "customText",
                    "fontWeight", "letterSpacing", "systemAuto", "pagination"
                ],
                "properties": {
                    "fontFamily": nullable_enum(&["sans", "serif", "kai", "heiti", "fangsong", "mianfei", "custom"]),
                    "customFontFamily": nullable_string(AI_RECOMMENDATION_TEXT_FIELD_MAX_LEN),
                    "fontSize": nullable_enum(&["small", "medium", "large"]),
                    "lineHeight": nullable_enum(&["compact", "comfortable", "airy"]),
                    "contentWidth": nullable_enum(&["narrow", "medium", "wide"]),
                    "theme": nullable_enum(&["system", "paper", "warm", "slate", "dark", "sepia", "eyeCare", "custom"]),
                    "customBackground": nullable_string(AI_RECOMMENDATION_TEXT_FIELD_MAX_LEN),
                    "customText": nullable_string(AI_RECOMMENDATION_TEXT_FIELD_MAX_LEN),
                    "fontWeight": nullable_enum(&["light", "regular", "medium", "semibold", "bold"]),
                    "letterSpacing": nullable_enum(&["tight", "normal", "relaxed", "loose"]),
                    "systemAuto": { "type": ["boolean", "null"] },
                    "pagination": nullable_enum(&["scroll", "paginated", "double"])
                }
            },
            "explanation": nullable_string(AI_RECOMMENDATION_EXPLANATION_MAX_LEN)
        }
    })
}

#[derive(serde::Deserialize)]
struct ChatCompletionEnvelope {
    choices: Vec<ChatCompletionChoice>,
}

#[derive(serde::Deserialize)]
struct ChatCompletionChoice {
    message: ChatCompletionMessage,
}

#[derive(serde::Deserialize)]
struct ChatCompletionMessage {
    content: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RecommendationPayload {
    settings_patch: PreferenceReadingPatchDto,
    explanation: Option<String>,
}

pub(crate) fn interpret_recommendation_response(
    status: u16,
    body: &[u8],
) -> Result<AiSettingsRecommendation, AppError> {
    if !(200..300).contains(&status) {
        return Err(recommendation_status_error(status));
    }
    let envelope: ChatCompletionEnvelope = serde_json::from_slice(body)
        .map_err(|_| recommendation_invalid_response("响应不是合法的 chat/completions JSON"))?;
    let content = envelope
        .choices
        .into_iter()
        .next()
        .and_then(|choice| choice.message.content)
        .ok_or_else(|| recommendation_invalid_response("响应缺少 choices[0].message.content"))?;
    parse_recommendation_content(&content)
}

pub(crate) fn parse_recommendation_content(
    content: &str,
) -> Result<AiSettingsRecommendation, AppError> {
    if content.trim().is_empty() {
        return Err(recommendation_invalid_response("模型没有返回结构化建议"));
    }
    let value: Value = serde_json::from_str(content)
        .map_err(|_| recommendation_invalid_response("模型输出不符合严格建议 schema"))?;
    let object = value
        .as_object()
        .ok_or_else(|| recommendation_invalid_response("模型输出必须是对象"))?;
    const OUTER_KEYS: &[&str] = &["settingsPatch", "explanation"];
    if !has_exact_keys(object, OUTER_KEYS) {
        return Err(recommendation_invalid_response(
            "模型输出包含未知或缺失字段",
        ));
    }
    let patch = object
        .get("settingsPatch")
        .and_then(Value::as_object)
        .ok_or_else(|| recommendation_invalid_response("settingsPatch 必须是对象"))?;
    const PATCH_KEYS: &[&str] = &[
        "fontFamily",
        "customFontFamily",
        "fontSize",
        "lineHeight",
        "contentWidth",
        "theme",
        "customBackground",
        "customText",
        "fontWeight",
        "letterSpacing",
        "systemAuto",
        "pagination",
    ];
    if !has_exact_keys(patch, PATCH_KEYS) {
        return Err(recommendation_invalid_response(
            "settingsPatch 字段集合不完整",
        ));
    }
    let payload: RecommendationPayload = serde_json::from_value(value)
        .map_err(|_| recommendation_invalid_response("模型输出不符合严格建议 schema"))?;
    AiSettingsRecommendation::new(
        ReadingPatch::from(payload.settings_patch),
        payload.explanation,
    )
}

fn has_exact_keys(object: &serde_json::Map<String, Value>, expected: &[&str]) -> bool {
    object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key))
}

fn recommendation_status_error(status: u16) -> AppError {
    if status == 401 || status == 403 {
        return AppError::new(
            "AI_PROVIDER_RECOMMENDATION_UNAUTHORIZED",
            ErrorKind::Unauthorized,
            "API Key 无效或没有调用对话接口的权限",
            false,
        );
    }
    if (300..400).contains(&status) {
        return AppError::new(
            "AI_PROVIDER_RECOMMENDATION_UNEXPECTED_REDIRECT",
            ErrorKind::Network,
            "建议接口返回了重定向；请把 API 地址改为最终地址",
            false,
        );
    }
    AppError::new(
        "AI_PROVIDER_RECOMMENDATION_HTTP_ERROR",
        ErrorKind::Network,
        format!("设置建议请求失败（HTTP {status}）"),
        true,
    )
}

fn recommendation_invalid_response(detail: &'static str) -> AppError {
    AppError::new(
        "AI_PROVIDER_RECOMMENDATION_INVALID_RESPONSE",
        ErrorKind::Parse,
        detail,
        false,
    )
}

fn recommendation_invalid_input() -> AppError {
    AppError::new(
        "AI_PROVIDER_RECOMMENDATION_INPUT_INVALID",
        ErrorKind::Validation,
        "设置建议请求超出允许的大小或形状",
        false,
    )
}

fn recommendation_response_too_large() -> AppError {
    AppError::new(
        "AI_PROVIDER_RECOMMENDATION_RESPONSE_TOO_LARGE",
        ErrorKind::Network,
        "设置建议响应超出大小上限",
        false,
    )
}

fn recommendation_unreachable() -> AppError {
    AppError::new(
        "AI_PROVIDER_RECOMMENDATION_UNREACHABLE",
        ErrorKind::Network,
        "无法连接到设置建议接口，请检查 API 地址与网络",
        true,
    )
}

fn recommendation_client_error() -> AppError {
    AppError::new(
        "AI_PROVIDER_RECOMMENDATION_CLIENT_INIT_FAILED",
        ErrorKind::Internal,
        "设置建议客户端初始化失败",
        false,
    )
}

fn recommendation_target_error(error: crate::http_security::HttpTargetError) -> AppError {
    use crate::http_security::HttpTargetError as Error;
    match error {
        Error::Invalid | Error::UnsafeAddress => AppError::new(
            "AI_PROVIDER_ENDPOINT_INVALID",
            ErrorKind::Validation,
            "API 地址不被允许（仅接受公网 http/https 主机）",
            false,
        ),
        Error::ResolveFailed => recommendation_unreachable(),
    }
}

/// 供组装层做启动期自检：策略必须拒绝非公网端点。
pub fn assert_endpoint_policy_is_fail_closed() -> bool {
    parse_http_url(
        "http://127.0.0.1:11434/v1",
        HttpUrlPolicy::AiProviderEndpoint,
    )
    .is_err()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models_body(payload: Value) -> Vec<u8> {
        serde_json::to_vec(&payload).unwrap()
    }

    #[test]
    fn parses_openai_compatible_success_payload() {
        let body = models_body(serde_json::json!({
            "object": "list",
            "data": [
                {
                    "id": "gpt-4o-mini",
                    "object": "model",
                    "created": 1_700_000_000,
                    "owned_by": "system",
                },
                {
                    "id": "text-embedding-3-small",
                    "created": 1_699_000_000,
                    "owned_by": "system",
                    "capabilities": { "embedding": true }
                }
            ]
        }));
        let models = interpret_models_response(200, &body).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].model_id, "gpt-4o-mini");
        assert_eq!(models[0].created, Some(1_700_000_000));
        assert_eq!(models[0].owned_by.as_deref(), Some("system"));
        // 没有任何能力声明 → 三态全部 unknown，绝不从名字推断。
        assert_eq!(models[0].chat, AiModelCapability::Unknown);
        assert_eq!(models[0].vision, AiModelCapability::Unknown);
        assert_eq!(models[0].embedding, AiModelCapability::Unknown);
        // 显式声明的能力才被采信。
        assert_eq!(models[1].embedding, AiModelCapability::Supported);
        assert_eq!(models[1].chat, AiModelCapability::Unknown);
    }

    #[test]
    fn model_names_never_imply_capabilities() {
        let body = models_body(serde_json::json!({
            "data": [
                { "id": "gpt-4o-vision-preview" },
                { "id": "llava-1.6-34b" },
                { "id": "text-embedding-ada-002" },
                { "id": "whisper-large-v3" }
            ]
        }));
        let models = interpret_models_response(200, &body).unwrap();
        assert_eq!(models.len(), 4);
        for model in &models {
            assert_eq!(
                model.vision,
                AiModelCapability::Unknown,
                "{}",
                model.model_id
            );
            assert_eq!(
                model.embedding,
                AiModelCapability::Unknown,
                "{}",
                model.model_id
            );
            assert_eq!(model.chat, AiModelCapability::Unknown, "{}", model.model_id);
        }
    }

    #[test]
    fn explicit_unsupported_is_distinct_from_unknown() {
        let body = models_body(serde_json::json!({
            "data": [
                { "id": "chat-only", "capabilities": { "chat": true, "vision": false } }
            ]
        }));
        let models = interpret_models_response(200, &body).unwrap();
        assert_eq!(models[0].chat, AiModelCapability::Supported);
        assert_eq!(models[0].vision, AiModelCapability::Unsupported);
        assert_eq!(models[0].embedding, AiModelCapability::Unknown);
    }

    #[test]
    fn empty_catalog_is_an_empty_catalog_not_an_error() {
        let body = models_body(serde_json::json!({ "object": "list", "data": [] }));
        assert!(interpret_models_response(200, &body).unwrap().is_empty());
    }

    #[test]
    fn non_2xx_is_a_stable_actionable_error() {
        let unauthorized = interpret_models_response(401, b"{}").unwrap_err();
        assert_eq!(
            unauthorized.code().as_str(),
            "AI_PROVIDER_MODELS_UNAUTHORIZED"
        );
        assert!(!unauthorized.retryable(), "密钥错误重试没有意义");

        let forbidden = interpret_models_response(403, b"").unwrap_err();
        assert_eq!(forbidden.code().as_str(), "AI_PROVIDER_MODELS_UNAUTHORIZED");

        let server_error = interpret_models_response(503, b"upstream down").unwrap_err();
        assert_eq!(
            server_error.code().as_str(),
            "AI_PROVIDER_MODELS_HTTP_ERROR"
        );
        assert!(server_error.retryable());
        assert!(server_error.user_message().contains("503"));

        let redirect = interpret_models_response(302, b"").unwrap_err();
        assert_eq!(
            redirect.code().as_str(),
            "AI_PROVIDER_MODELS_UNEXPECTED_REDIRECT"
        );
        assert!(!redirect.retryable(), "重定向是配置错误，重试不会变好");
    }

    #[test]
    fn invalid_json_and_wrong_shape_are_distinct_errors() {
        let not_json = interpret_models_response(200, b"<html>gateway</html>").unwrap_err();
        assert_eq!(
            not_json.code().as_str(),
            "AI_PROVIDER_MODELS_INVALID_RESPONSE"
        );
        assert!(!not_json.retryable());

        // 合法 JSON 但形状不对（缺 data 数组）。
        let wrong_shape = interpret_models_response(200, b"{\"models\":[]}").unwrap_err();
        assert_eq!(
            wrong_shape.code().as_str(),
            "AI_PROVIDER_MODELS_INVALID_RESPONSE"
        );

        // data 不是数组。
        let not_array = interpret_models_response(200, b"{\"data\":{}}").unwrap_err();
        assert_eq!(
            not_array.code().as_str(),
            "AI_PROVIDER_MODELS_INVALID_RESPONSE"
        );
    }

    #[test]
    fn malformed_entries_are_skipped_never_faked() {
        let body = models_body(serde_json::json!({
            "data": [
                { "id": "good-model" },
                { "id": "" },
                { "id": "bad model with space" },
                { "id": "bad/id" },
                { "object": "model" },
                "not-an-object",
                { "id": "another-good-model" }
            ]
        }));
        let models = interpret_models_response(200, &body).unwrap();
        let ids: Vec<&str> = models.iter().map(|m| m.model_id.as_str()).collect();
        assert_eq!(ids, ["good-model", "another-good-model"]);
    }

    #[test]
    fn error_messages_never_echo_the_endpoint_or_secret() {
        let error = target_error(crate::http_security::HttpTargetError::UnsafeAddress);
        for forbidden in ["127.0.0.1", "11434", "http://", "Bearer", "sk-"] {
            assert!(
                !error.user_message().contains(forbidden),
                "错误消息不得回显端点或密钥: {}",
                error.user_message()
            );
        }
        // 每一个出站错误的 user_message 都必须是固定文案：
        // 底层 reqwest 错误带着完整 URL 与请求上下文，绝不能被拼进用户可见消息。
        for error in [
            unreachable_error(),
            response_too_large(),
            client_error(),
            target_error(crate::http_security::HttpTargetError::ResolveFailed),
            target_error(crate::http_security::HttpTargetError::Invalid),
            status_error(500),
            status_error(404),
            status_error(401),
            status_error(302),
            invalid_response("响应缺少 data 数组"),
        ] {
            for forbidden in ["https://", "http://", "Bearer", "sk-", "gateway"] {
                assert!(
                    !error.user_message().contains(forbidden),
                    "错误消息不得回显端点或密钥: {}",
                    error.user_message()
                );
            }
        }
    }

    #[test]
    fn streaming_cap_rejects_oversized_chunked_bodies() {
        let mut body: Vec<u8> = Vec::new();
        // 恰好到上界：接受。
        let full = vec![b'a'; MAX_MODELS_BYTES];
        extend_with_cap(&mut body, &full).unwrap();
        assert_eq!(body.len(), MAX_MODELS_BYTES);
        // 再多一块：拒绝，且不得把这一块追加进去。
        let error = extend_with_cap(&mut body, b"x").unwrap_err();
        assert_eq!(
            error.code().as_str(),
            "AI_PROVIDER_MODELS_RESPONSE_TOO_LARGE"
        );
        assert!(!error.retryable());
        assert_eq!(body.len(), MAX_MODELS_BYTES, "越界块不得被累积");

        // 分块累积到上界同样成立（防止"每块都合法但总量超界"绕过）。
        let mut chunked: Vec<u8> = Vec::new();
        let piece = vec![b'b'; 64 * 1024];
        let mut accepted = 0usize;
        while extend_with_cap(&mut chunked, &piece).is_ok() {
            accepted += 1;
            assert!(accepted <= MAX_MODELS_BYTES, "循环必须有上界");
        }
        assert_eq!(chunked.len(), MAX_MODELS_BYTES);
    }

    #[test]
    fn endpoint_policy_is_fail_closed_for_private_targets() {
        assert!(assert_endpoint_policy_is_fail_closed());
        for raw in [
            "http://127.0.0.1:11434/v1/models",
            "http://192.168.1.10/v1/models",
            "http://10.0.0.5/v1/models",
            "https://localhost/v1/models",
            "http://169.254.169.254/latest/meta-data",
        ] {
            assert!(
                parse_http_url(raw, HttpUrlPolicy::AiProviderEndpoint).is_err(),
                "私网/回环端点必须被拒绝: {raw}"
            );
        }
    }

    fn sample_reading() -> haven_domain::agent::AgentReadingSnapshot {
        haven_domain::agent::AgentReadingSnapshot {
            font_family: "sans".into(),
            custom_font_family: None,
            font_size: "medium".into(),
            line_height: "comfortable".into(),
            content_width: "medium".into(),
            theme: "system".into(),
            custom_background: None,
            custom_text: None,
            font_weight: "regular".into(),
            letter_spacing: "normal".into(),
            system_auto: true,
            pagination: "scroll".into(),
            redacted: Default::default(),
        }
    }

    #[test]
    fn recommendation_schema_is_strict_and_closed() {
        let schema = recommendation_json_schema();
        assert_eq!(schema["additionalProperties"], Value::Bool(false));
        assert_eq!(
            schema["required"],
            serde_json::json!(["settingsPatch", "explanation"])
        );
        let patch = &schema["properties"]["settingsPatch"];
        assert_eq!(patch["additionalProperties"], Value::Bool(false));
        assert_eq!(patch["required"].as_array().unwrap().len(), 12);
        assert_eq!(
            schema["properties"]["explanation"]["maxLength"],
            serde_json::json!(AI_RECOMMENDATION_EXPLANATION_MAX_LEN)
        );
    }

    #[test]
    fn recommendation_request_is_bounded_and_has_json_schema_response_format() {
        let body = build_recommendation_request(&AiSettingsRecommendationInput {
            model_id: "chat-model".into(),
            reading: sample_reading(),
        })
        .unwrap();
        assert_eq!(body["model"], "chat-model");
        assert_eq!(body["temperature"], 0);
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(body["response_format"]["json_schema"]["strict"], true);
        assert!(
            !body["messages"][1]["content"]
                .as_str()
                .unwrap()
                .contains("sk-")
        );

        let oversized = AiSettingsRecommendationInput {
            model_id: "chat-model".into(),
            reading: haven_domain::agent::AgentReadingSnapshot {
                custom_background: Some("x".repeat(MAX_RECOMMENDATION_PROMPT_BYTES)),
                ..sample_reading()
            },
        };
        assert_eq!(
            build_recommendation_request(&oversized)
                .unwrap_err()
                .code()
                .as_str(),
            "AI_PROVIDER_RECOMMENDATION_INPUT_INVALID"
        );
    }

    fn complete_recommendation_content(patch: &str, explanation: &str) -> String {
        format!(
            r#"{{"settingsPatch":{{"fontFamily":null,"customFontFamily":null,"fontSize":{patch},"lineHeight":null,"contentWidth":null,"theme":null,"customBackground":null,"customText":null,"fontWeight":null,"letterSpacing":null,"systemAuto":null,"pagination":null}},"explanation":{explanation}}}"#
        )
    }

    #[test]
    fn recommendation_content_is_strictly_typed_and_fail_closed() {
        let content = complete_recommendation_content("\"large\"", "\"调大字号\"");
        let recommendation = parse_recommendation_content(&content).unwrap();
        assert_eq!(
            recommendation.patch().font_size,
            Some(haven_domain::settings::ReadingFontSize::Large)
        );
        assert_eq!(recommendation.explanation(), Some("调大字号"));

        for bad in [
            "",
            "not json",
            r#"{"settingsPatch":{},"explanation":null}"#,
            r#"{"settingsPatch":{"fontFamily":null,"customFontFamily":null,"fontSize":"huge","lineHeight":null,"contentWidth":null,"theme":null,"customBackground":null,"customText":null,"fontWeight":null,"letterSpacing":null,"systemAuto":null,"pagination":null},"explanation":null}"#,
            r#"{"settingsPatch":{"fontFamily":null,"customFontFamily":null,"fontSize":null,"lineHeight":null,"contentWidth":null,"theme":null,"customBackground":null,"customText":null,"fontWeight":null,"letterSpacing":null,"systemAuto":null,"pagination":null},"explanation":null,"extra":true}"#,
        ] {
            assert!(
                parse_recommendation_content(bad).is_err(),
                "必须 fail closed: {bad}"
            );
        }
    }

    #[test]
    fn recommendation_envelope_and_transport_errors_are_stable() {
        let body = serde_json::to_vec(&serde_json::json!({
            "id": "chatcmpl-1",
            "choices": [{
                "message": {
                    "content": complete_recommendation_content("null", "null")
                }
            }]
        }))
        .unwrap();
        let recommendation = interpret_recommendation_response(200, &body).unwrap();
        assert!(recommendation.explanation().is_none());
        assert_eq!(recommendation.patch().font_size, None);

        for (status, code) in [
            (401, "AI_PROVIDER_RECOMMENDATION_UNAUTHORIZED"),
            (302, "AI_PROVIDER_RECOMMENDATION_UNEXPECTED_REDIRECT"),
            (503, "AI_PROVIDER_RECOMMENDATION_HTTP_ERROR"),
        ] {
            assert_eq!(
                interpret_recommendation_response(status, b"upstream")
                    .unwrap_err()
                    .code()
                    .as_str(),
                code
            );
        }
        let mut response = Vec::new();
        extend_with_recommendation_cap(&mut response, &vec![b'a'; MAX_RECOMMENDATION_BYTES])
            .unwrap();
        assert_eq!(
            extend_with_recommendation_cap(&mut response, b"x")
                .unwrap_err()
                .code()
                .as_str(),
            "AI_PROVIDER_RECOMMENDATION_RESPONSE_TOO_LARGE"
        );
    }
}
