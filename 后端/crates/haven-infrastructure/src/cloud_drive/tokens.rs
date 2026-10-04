//! Google OAuth 令牌交换与刷新（Infrastructure）。
//!
//! 固定端点 `https://oauth2.googleapis.com/token`，标准 `application/x-www-form-urlencoded`
//! 正文：exchange 带授权码 + PKCE verifier + redirect_uri，refresh 带 refresh_token；
//! `client_secret` 完全可选，存在时只以 [`SecretString`] 参与表单，不进错误 / 日志 / Debug。
//! 响应按严格有界 schema 解析（`token_type` 必须是 Bearer、`expires_in` 有界且为正、
//! `scope` 出现时必须正好是固定只读 scope、exchange 必须带回 refresh token、refresh 缺失新
//! token 时回退到调用方持有的旧 refresh token），解析结果只用 [`SecretString`] 持有，原始
//! 响应字节在所有路径上清零。400/401/403 的 `invalid_grant` 映射为
//! `CLOUD_DRIVE_UNAUTHORIZED`（不可重试），上游其余状态与畸形响应分别映射为「暂时不可用」
//! 与「响应无法识别」；上游 error / error_description / 正文 / URL 一律不回显。本模块只做
//! 令牌：不选配置、不开浏览器、不监听端口、不落库。

use std::fmt;
use std::sync::Arc;

use serde::Deserialize;
use zeroize::Zeroize;

use haven_application::services::cloud_storage::ports::CloudDriveCredential;
use haven_common::{AppError, ErrorKind, UtcMillis, internal};
use haven_domain::credential::SecretString;

use crate::cloud_drive::handshake::{
    GOOGLE_DRIVE_READONLY_SCOPE, validate_client_id, validate_loopback_redirect_uri,
};
use crate::cloud_drive::http::{
    GoogleMethod, GoogleRequest, GoogleResponse, GoogleTransport, JSON_LIMIT,
};

/// 固定令牌端点；不接受任何调用方提供的地址。
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

/// 稳定错误码（对外契约）；kind / 文案 / 可重试性在本模块内固定。
const CLOUD_DRIVE_UNAUTHORIZED: &str = "CLOUD_DRIVE_UNAUTHORIZED";
const CLOUD_DRIVE_UNAVAILABLE: &str = "CLOUD_DRIVE_UNAVAILABLE";
const CLOUD_DRIVE_RESPONSE_INVALID: &str = "CLOUD_DRIVE_RESPONSE_INVALID";
const CLOUD_OAUTH_CONFIG_INVALID: &str = "CLOUD_OAUTH_CONFIG_INVALID";
const CLOUD_OAUTH_PROVIDER_REJECTED: &str = "CLOUD_OAUTH_PROVIDER_REJECTED";

/// 响应整体上限：远低于 HTTP 层 JSON 上限，token 端点只会返回几百字节。
const MAX_TOKEN_RESPONSE_BYTES: usize = 8 * 1024;
/// 单个 access / refresh token 的上限（可见 ASCII，无空白）。
const MAX_TOKEN_LEN: usize = 4096;
/// 可选 client secret 的上限，同样要求可见 ASCII。
const MAX_CLIENT_SECRET_LEN: usize = 128;
/// `expires_in` 上限 24 小时，防止上游给出夸张有效期。
const MAX_EXPIRES_IN_SECS: i64 = 24 * 60 * 60;

/// 稳定错误码 → 固定 kind / 文案 / 可重试性；状态码、正文、URL 与令牌都不进入错误。
fn err(code: &'static str) -> AppError {
    let (kind, message, retryable) = match code {
        CLOUD_DRIVE_UNAUTHORIZED => (ErrorKind::Unauthorized, "云盘授权已失效，请重新连接", false),
        CLOUD_DRIVE_UNAVAILABLE => (ErrorKind::Network, "云盘暂时不可用", true),
        CLOUD_DRIVE_RESPONSE_INVALID => (ErrorKind::Parse, "云盘返回的数据无法识别", false),
        CLOUD_OAUTH_CONFIG_INVALID => (ErrorKind::Validation, "OAuth 客户端配置不合法", false),
        // 其余（含 CLOUD_OAUTH_PROVIDER_REJECTED）：上游拒绝了这次令牌请求。
        _ => (ErrorKind::Forbidden, "Google 拒绝了本次令牌请求", false),
    };
    AppError::new(code, kind, message, retryable)
}

fn malformed() -> AppError {
    err(CLOUD_DRIVE_RESPONSE_INVALID)
}

/// Google 令牌端点客户端：客户端配置由配置选择方注入，本模块不读配置、不选账号。
pub(crate) struct GoogleOAuthTokens {
    client_id: Option<String>,
    client_secret: Option<SecretString>,
    transport: Arc<dyn GoogleTransport>,
}

impl GoogleOAuthTokens {
    pub(crate) fn new(
        client_id: Option<String>,
        client_secret: Option<SecretString>,
        transport: Arc<dyn GoogleTransport>,
    ) -> Self {
        Self {
            client_id,
            client_secret,
            transport,
        }
    }

    /// 只校验本地客户端配置，不发请求；配置选择方在打开浏览器之前调用。
    pub(crate) fn ensure_configured(&self) -> Result<(), AppError> {
        self.config().map(|_| ())
    }

    pub(crate) fn authorization_url(
        &self,
        redirect_uri: &str,
        challenge: &crate::cloud_drive::handshake::OAuthChallenge,
    ) -> Result<String, AppError> {
        let (client_id, _) = self.config()?;
        crate::cloud_drive::handshake::authorization_url(client_id, redirect_uri, challenge)
    }

    /// 授权码 + PKCE verifier 换令牌；回调地址必须与授权阶段完全一致。
    pub(crate) async fn exchange(
        &self,
        code: &SecretString,
        verifier: &SecretString,
        redirect_uri: &str,
    ) -> Result<CloudDriveCredential, AppError> {
        let (client_id, secret) = self.config()?;
        if !visible_ascii(code.expose(), 4096)
            || !(43..=128).contains(&verifier.expose().len())
            || !verifier
                .expose()
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
        {
            return Err(err(CLOUD_OAUTH_CONFIG_INVALID));
        }
        // 与授权阶段共用同一份 loopback 校验，失败早于任何网络 IO。
        validate_loopback_redirect_uri(redirect_uri)?;
        let mut form = vec![field("client_id", client_id)];
        if let Some(secret) = secret {
            form.push(field("client_secret", secret.expose()));
        }
        form.push(field("code", code.expose()));
        form.push(field("code_verifier", verifier.expose()));
        form.push(field("grant_type", "authorization_code"));
        form.push(field("redirect_uri", redirect_uri));
        // 首次授权必须带回 refresh token，没有旧 token 可以回退。
        self.send(form, None).await
    }

    /// 用已持久化的 refresh token 换新 access token；Google 未轮换时沿用旧 token。
    pub(crate) async fn refresh(
        &self,
        refresh_token: &SecretString,
    ) -> Result<CloudDriveCredential, AppError> {
        let (client_id, secret) = self.config()?;
        if !visible_ascii(refresh_token.expose(), MAX_TOKEN_LEN) {
            return Err(err(CLOUD_DRIVE_UNAUTHORIZED));
        }
        let mut form = vec![field("client_id", client_id)];
        if let Some(secret) = secret {
            form.push(field("client_secret", secret.expose()));
        }
        form.push(field("grant_type", "refresh_token"));
        form.push(field("refresh_token", refresh_token.expose()));
        self.send(form, Some(refresh_token)).await
    }

    /// 本地配置：client id 复用握手模块校验（缺失 / 非法各有稳定错误码），client secret
    /// 可选，出现时必须是有界的可见 ASCII。
    fn config(&self) -> Result<(&str, Option<&SecretString>), AppError> {
        let client_id = validate_client_id(self.client_id.as_deref().unwrap_or_default())?;
        match &self.client_secret {
            None => Ok((client_id, None)),
            Some(secret) if visible_ascii(secret.expose(), MAX_CLIENT_SECRET_LEN) => {
                Ok((client_id, Some(secret)))
            }
            Some(_) => Err(err(CLOUD_OAUTH_CONFIG_INVALID)),
        }
    }

    /// 唯一出网入口：固定端点、POST 表单、无 bearer、无 Range、上限 JSON_LIMIT；传输层错误
    /// 按原样上抛（HTTP 适配器已归一化网络错误，与 drive 适配器同一约定）。
    async fn send(
        &self,
        form: Vec<(String, SecretString)>,
        fallback_refresh: Option<&SecretString>,
    ) -> Result<CloudDriveCredential, AppError> {
        let request = GoogleRequest {
            url: reqwest::Url::parse(TOKEN_ENDPOINT).map_err(|_| internal("OAuth 令牌端点无效"))?,
            method: GoogleMethod::PostForm,
            bearer: None,
            form,
            range: None,
            max_bytes: JSON_LIMIT,
        };
        let response = self.transport.send(request).await?;
        credential(response, fallback_refresh)
    }
}

/// 表单字段一律以 [`SecretString`] 持有（client id 也走同一路径，避免明文副本）。
fn field(name: &str, value: &str) -> (String, SecretString) {
    (name.to_owned(), SecretString::new(value))
}

/// 状态映射先于解析；原始响应字节无论走哪条路径都在返回前清零。
fn credential(
    response: GoogleResponse,
    fallback_refresh: Option<&SecretString>,
) -> Result<CloudDriveCredential, AppError> {
    let body = zeroize::Zeroizing::new(response.bytes);
    let outcome = match response.status {
        200 if body.len() <= MAX_TOKEN_RESPONSE_BYTES => parse(&body, fallback_refresh),
        200 => Err(malformed()),
        400 | 401 | 403 => Err(rejection(&body)),
        // 5xx / 429 / 其它意外状态都当作上游暂时不可用，可重试。
        _ => Err(err(CLOUD_DRIVE_UNAVAILABLE)),
    };
    outcome
}

/// 严格有界 schema：不合规的形状一律拒绝，不做兼容猜测。
fn parse(
    body: &[u8],
    fallback_refresh: Option<&SecretString>,
) -> Result<CloudDriveCredential, AppError> {
    let raw: RawToken = serde_json::from_slice(body).map_err(|_| malformed())?;
    if !raw
        .token_type
        .as_deref()
        .is_some_and(|kind| kind.eq_ignore_ascii_case("Bearer"))
    {
        return Err(malformed());
    }
    if raw
        .scope
        .as_deref()
        .is_some_and(|scope| scope != GOOGLE_DRIVE_READONLY_SCOPE)
    {
        return Err(malformed());
    }
    let seconds = raw
        .expires_in
        .filter(|seconds| *seconds > 0 && *seconds <= MAX_EXPIRES_IN_SECS)
        .ok_or_else(malformed)?;
    let refresh_token = match raw.refresh_token {
        Some(token) => token.into_secret(),
        // 刷新时 Google 可能不回传新 refresh token：沿用调用方持有的旧 token。
        None => match fallback_refresh {
            Some(token) => SecretString::new(token.expose()),
            None => return Err(malformed()),
        },
    };
    let expires_at_ms = seconds
        .checked_mul(1000)
        .and_then(|millis| UtcMillis::now().0.checked_add(millis))
        .ok_or_else(malformed)?;
    Ok(CloudDriveCredential {
        access_token: raw.access_token.into_secret(),
        refresh_token,
        expires_at_ms,
        scope: raw.scope,
    })
}

/// 400/401/403：只有平铺的 `{"error":"invalid_grant"}` 判为授权失效，其余一律是固定的
/// 「上游拒绝」；`error_description` 与正文不读取、不回显。
fn rejection(body: &[u8]) -> AppError {
    let invalid_grant = if body.len() > MAX_TOKEN_RESPONSE_BYTES {
        false
    } else {
        serde_json::from_slice::<RawTokenError>(body)
            .ok()
            .and_then(|raw| raw.error)
            .is_some_and(|error| error == "invalid_grant")
    };
    if invalid_grant {
        err(CLOUD_DRIVE_UNAUTHORIZED)
    } else {
        err(CLOUD_OAUTH_PROVIDER_REJECTED)
    }
}

/// 非空、有界、只含可见 ASCII；空白 / 控制字符 / 非 ASCII / 超长一律拒绝。
fn visible_ascii(value: &str, max_len: usize) -> bool {
    !value.is_empty() && value.len() <= max_len && value.bytes().all(|byte| byte.is_ascii_graphic())
}

/// 只从 JSON 字符串进入、直接持有底层 `String` 的一次性字段：无 `Clone` / `Debug` /
/// `Serialize`；`visit_string` 接管 serde 已分配的缓冲区，Drop 时随 [`SecretString`] 清零。
struct RawSecret(SecretString);

impl RawSecret {
    fn into_secret(self) -> SecretString {
        self.0
    }
}

impl<'de> Deserialize<'de> for RawSecret {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Bounded;

        impl<'de> serde::de::Visitor<'de> for Bounded {
            type Value = RawSecret;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("有界的可见 ASCII 令牌字符串")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<RawSecret, E> {
                if !visible_ascii(value, MAX_TOKEN_LEN) {
                    return Err(E::custom("token"));
                }
                Ok(RawSecret(SecretString::new(value)))
            }

            fn visit_string<E: serde::de::Error>(self, mut value: String) -> Result<RawSecret, E> {
                if !visible_ascii(&value, MAX_TOKEN_LEN) {
                    value.zeroize();
                    return Err(E::custom("token"));
                }
                Ok(RawSecret(SecretString::new(value)))
            }
        }

        deserializer.deserialize_str(Bounded)
    }
}

/// 成功响应：只读取下列字段，未知字段忽略，已知字段必须合规。
#[derive(Deserialize)]
struct RawToken {
    access_token: RawSecret,
    refresh_token: Option<RawSecret>,
    token_type: Option<String>,
    expires_in: Option<i64>,
    scope: Option<String>,
}

/// 失败响应只读平铺的 `error` 字符串；`error_description` / `error_uri` 一律忽略。
#[derive(Deserialize)]
struct RawTokenError {
    error: Option<String>,
}

#[cfg(test)]
#[path = "tokens_tests.rs"]
mod tests;
