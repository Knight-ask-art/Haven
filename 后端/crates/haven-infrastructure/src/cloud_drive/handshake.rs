//! Google Drive 只读连接的 OAuth 握手（PKCE S256 + 回调校验）：生成一次性 `state` /
//! `code_verifier`、推导 S256 `code_challenge`、拼装固定授权地址、校验 loopback 回调
//! query。不监听端口、不发请求、不换 token、不落库、不写日志。
//!
//! 安全约定：`state` / `code_verifier` 只以 [`SecretString`] 保存，无 `Clone` /
//! `Serialize`，`Debug` 脱敏，且禁止进入日志 / 错误 / DTO / 持久化；回调 query 是不可信
//! 输入，错误文案固定、不回显 query / code / state / provider 文本；授权端点、scope 与
//! 回调路径是本模块常量，调用方只能给 client id 与 loopback 端口；随机数取 3 个独立
//! UUID v4（48 字节 → base64url 无填充 64 字符，扣掉版本 / 变体位仍有 366 位熵）。

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use haven_common::{AppError, ErrorKind};
use haven_domain::credential::SecretString;

/// 固定授权端点与只读 scope；host 与路径由本模块决定。
pub(crate) const GOOGLE_AUTHORIZATION_ENDPOINT: &str =
    "https://accounts.google.com/o/oauth2/v2/auth";
pub(crate) const GOOGLE_DRIVE_READONLY_SCOPE: &str =
    "https://www.googleapis.com/auth/drive.readonly";
/// loopback 回调路径；运行时监听器必须用同一路径注册路由。
pub(crate) const LOOPBACK_CALLBACK_PATH: &str = "/oauth2/callback";

/// 稳定错误码（对外契约）；四个文案都是固定字符串。
const NOT_CONFIGURED: &str = "CLOUD_OAUTH_NOT_CONFIGURED";
const CONFIG_INVALID: &str = "CLOUD_OAUTH_CONFIG_INVALID";
const CALLBACK_INVALID: &str = "CLOUD_OAUTH_CALLBACK_INVALID";
const PROVIDER_REJECTED: &str = "CLOUD_OAUTH_PROVIDER_REJECTED";

/// 回调、授权码与标准附加字段有界；独立于本机 HTTP 请求头上限。
const MAX_QUERY_LEN: usize = 8192;
const MAX_QUERY_PARAMS: usize = 16;
const MAX_PARAM_NAME_LEN: usize = 64;
const MAX_STATE_LEN: usize = 128;
const MAX_CODE_LEN: usize = 4096;
const MAX_ERROR_LEN: usize = 64;
const MAX_EXTRA_VALUE_LEN: usize = 1024;
const MAX_CLIENT_ID_LEN: usize = 128;
const MAX_REDIRECT_URI_LEN: usize = 128;
/// 每个 token 拼 3 个独立 UUID v4 的 16 字节。
const TOKEN_BYTES: usize = 48;

/// 未配置 OAuth 客户端；运行时映射为「不可用」。
fn not_configured() -> AppError {
    AppError::new(
        NOT_CONFIGURED,
        ErrorKind::Unsupported,
        "未配置 Google OAuth 客户端",
        false,
    )
}
/// client id / 回调地址不合规。
fn config_invalid() -> AppError {
    AppError::new(
        CONFIG_INVALID,
        ErrorKind::Validation,
        "OAuth 客户端配置不合法",
        false,
    )
}
/// 回调 query 不合法。
fn callback_invalid() -> AppError {
    AppError::new(
        CALLBACK_INVALID,
        ErrorKind::Validation,
        "OAuth 回调参数不合法",
        false,
    )
}
/// Provider 拒绝授权（非用户取消）；不回显 provider 的 error / error_description。
fn provider_rejected() -> AppError {
    AppError::new(
        PROVIDER_REJECTED,
        ErrorKind::Forbidden,
        "Google 拒绝了本次授权请求",
        true,
    )
}

/// 一次授权尝试的握手材料。字段对 crate 可见，但**禁止**进入日志 / 错误 / DTO / 持久化。
pub(crate) struct OAuthChallenge {
    pub(crate) state: SecretString,
    pub(crate) code_verifier: SecretString,
    pub(crate) code_challenge: String,
}

impl OAuthChallenge {
    /// 独立生成 state 与 verifier，并推导 S256 challenge。
    pub(crate) fn generate() -> Self {
        let code_verifier = random_token();
        let code_challenge = s256_challenge(&code_verifier);
        Self {
            state: SecretString::new(random_token()),
            code_verifier: SecretString::new(code_verifier),
            code_challenge,
        }
    }

    /// 校验 loopback 回调 query；成功给出授权码或固定的「用户取消」结果。
    pub(crate) fn parse_callback(&self, raw_query: &str) -> Result<CallbackOutcome, AppError> {
        parse_callback_query(raw_query, self.state.expose())
    }
}

/// `Debug` 只打印公开的 challenge（它本来就会出现在授权 URL 里）。
impl fmt::Debug for OAuthChallenge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthChallenge")
            .field("code_challenge", &self.code_challenge)
            .finish_non_exhaustive()
    }
}

/// 一次回调的结果；不实现 `Clone` / `Serialize`，`Debug` 不打印授权码。
pub(crate) enum CallbackOutcome {
    /// 授权码：秘密，只在 Rust 调用栈内使用。
    Code(SecretString),
    /// 用户在 Google 侧取消（`error=access_denied`）：正常结果，不是错误。
    Denied,
}

impl fmt::Debug for CallbackOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Code(_) => "CallbackOutcome::Code([REDACTED])",
            Self::Denied => "CallbackOutcome::Denied",
        })
    }
}

/// 48 字节 CSPRNG 材料 → base64url 无填充。
fn random_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    for chunk in bytes.chunks_exact_mut(16) {
        chunk.copy_from_slice(Uuid::new_v4().as_bytes());
    }
    URL_SAFE_NO_PAD.encode(bytes)
}

/// RFC 7636 §4.2：`code_challenge = base64url(SHA256(ASCII(code_verifier)))`。
pub(crate) fn s256_challenge(code_verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()))
}

/// 本机回环回调地址；端口由运行时在监听成功后提供。
pub(crate) fn loopback_redirect_uri(port: u16) -> Result<String, AppError> {
    if port == 0 {
        return Err(config_invalid());
    }
    Ok(format!("http://127.0.0.1:{port}{LOOPBACK_CALLBACK_PATH}"))
}

/// 拼装固定授权 URL；query_pairs_mut 按标准编码所有参数值。
pub(crate) fn authorization_url(
    client_id: &str,
    redirect_uri: &str,
    challenge: &OAuthChallenge,
) -> Result<String, AppError> {
    let client_id = validate_client_id(client_id)?;
    validate_loopback_redirect_uri(redirect_uri)?;
    let mut url =
        reqwest::Url::parse(GOOGLE_AUTHORIZATION_ENDPOINT).map_err(|_| config_invalid())?;
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", GOOGLE_DRIVE_READONLY_SCOPE)
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent")
        .append_pair("code_challenge", &challenge.code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", challenge.state.expose());
    Ok(url.into())
}

/// client id：非空、有界，且只含 Google 实际使用的字符集。
pub(crate) fn validate_client_id(client_id: &str) -> Result<&str, AppError> {
    if client_id.trim().is_empty() {
        return Err(not_configured());
    }
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
    if client_id.len() > MAX_CLIENT_ID_LEN || !client_id.chars().all(allowed) {
        return Err(config_invalid());
    }
    Ok(client_id)
}

/// 只接受 `http://127.0.0.1:<port>/oauth2/callback`：拒绝其它主机、路径、query、fragment、
/// 反斜杠与空白，避免授权码被送到本机之外的地址。
pub(crate) fn validate_loopback_redirect_uri(redirect_uri: &str) -> Result<(), AppError> {
    if redirect_uri.len() > MAX_REDIRECT_URI_LEN {
        return Err(config_invalid());
    }
    let rest = redirect_uri
        .strip_prefix("http://127.0.0.1:")
        .ok_or_else(config_invalid)?;
    let (port, path) = rest.split_once('/').ok_or_else(config_invalid)?;
    let digits = !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit());
    if !digits || port.parse::<u16>().map_or(true, |value| value == 0) {
        return Err(config_invalid());
    }
    if format!("/{path}") != LOOPBACK_CALLBACK_PATH {
        return Err(config_invalid());
    }
    Ok(())
}

/// 校验回调 query：长度 / 参数个数先于解析，重复项先于语义判断；失败一律固定文案。
pub(crate) fn parse_callback_query(
    raw_query: &str,
    expected_state: &str,
) -> Result<CallbackOutcome, AppError> {
    let query = raw_query.strip_prefix('?').unwrap_or(raw_query);
    if query.len() > MAX_QUERY_LEN || query.split('&').count() > MAX_QUERY_PARAMS {
        return Err(callback_invalid());
    }
    let (mut state, mut code, mut error) = (None, None, None);
    for segment in query.split('&').filter(|segment| !segment.is_empty()) {
        let (raw_name, raw_value) = segment.split_once('=').unwrap_or((segment, ""));
        let name = percent_decode(raw_name)?;
        if name.len() > MAX_PARAM_NAME_LEN {
            return Err(callback_invalid());
        }
        let value = percent_decode(raw_value)?;
        match name.as_str() {
            "state" => take_once(&mut state, value, MAX_STATE_LEN)?,
            "code" => take_once(&mut code, value, MAX_CODE_LEN)?,
            "error" => take_once(&mut error, value, MAX_ERROR_LEN)?,
            // Google 标准附加字段（scope / authuser / prompt / error_description）与未知
            // 字段都只做有界长度校验，不解释语义、不参与判断。
            _ if value.len() > MAX_EXTRA_VALUE_LEN => return Err(callback_invalid()),
            _ => {}
        }
    }
    let state = state.ok_or_else(callback_invalid)?;
    if state != expected_state {
        return Err(callback_invalid());
    }
    match (code, error) {
        (Some(code), None) => Ok(CallbackOutcome::Code(SecretString::new(code))),
        (None, Some(reason)) if reason == "access_denied" => Ok(CallbackOutcome::Denied),
        (None, Some(_)) => Err(provider_rejected()),
        _ => Err(callback_invalid()),
    }
}

/// 只允许出现一次的参数：重复、空值或超长都直接拒绝。
fn take_once(slot: &mut Option<String>, value: String, max_len: usize) -> Result<(), AppError> {
    if value.is_empty() || value.len() > max_len || slot.is_some() {
        return Err(callback_invalid());
    }
    *slot = Some(value);
    Ok(())
}

/// 有界 form query 解码：`+` 解码为空格，授权码中的字面 `+` 必须用 `%2B` 传输。
/// 拒绝非法转义、控制字符与非 UTF-8 结果。
fn percent_decode(raw: &str) -> Result<String, AppError> {
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = bytes
                .get(index + 1)
                .and_then(|b| char::from(*b).to_digit(16));
            let low = bytes
                .get(index + 2)
                .and_then(|b| char::from(*b).to_digit(16));
            let (Some(high), Some(low)) = (high, low) else {
                return Err(callback_invalid());
            };
            decoded.push(((high as u8) << 4) | (low as u8));
            index += 3;
        } else {
            decoded.push(if bytes[index] == b'+' {
                b' '
            } else {
                bytes[index]
            });
            index += 1;
        }
    }
    if decoded.iter().any(u8::is_ascii_control) {
        return Err(callback_invalid());
    }
    String::from_utf8(decoded).map_err(|_| callback_invalid())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// RFC 7636 Appendix B 的公开样例；SENTINEL 用来证明错误不回显输入。
    const RFC_VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    const RFC_CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
    const SENTINEL: &str = "SENTINEL";

    /// 断言 query 被拒，且错误对象既不回显 query 也不回显真实 state。
    fn rejected(challenge: &OAuthChallenge, query: &str) {
        let err = challenge.parse_callback(query).unwrap_err();
        assert_eq!(err.code().as_str(), CALLBACK_INVALID, "query={query}");
        let rendered = format!("{err:?}");
        assert!(!rendered.contains(SENTINEL), "错误回显了 query: {rendered}");
        assert!(
            !rendered.contains(challenge.state.expose()),
            "错误回显了 state"
        );
    }

    #[test]
    fn s256_and_tokens_match_rfc7636_rules() {
        assert_eq!(s256_challenge(RFC_VERIFIER), RFC_CHALLENGE);
        let (mut states, mut verifiers) = (HashSet::new(), HashSet::new());
        for _ in 0..64 {
            let challenge = OAuthChallenge::generate();
            let (state, verifier) = (challenge.state.expose(), challenge.code_verifier.expose());
            assert_eq!(challenge.code_challenge, s256_challenge(verifier));
            assert_ne!(state, verifier);
            assert!(states.insert(state.to_owned()) && verifiers.insert(verifier.to_owned()));
            for token in [state, verifier] {
                assert_eq!(token.len(), 64);
                let bytes = URL_SAFE_NO_PAD.decode(token).expect("base64url");
                assert_eq!(bytes.len(), TOKEN_BYTES);
                for chunk in bytes.chunks_exact(16) {
                    assert_eq!(
                        (chunk[6] >> 4, chunk[8] >> 6),
                        (4, 0b10),
                        "UUID v4 + RFC 4122"
                    );
                }
            }
        }
    }

    #[test]
    fn debug_never_prints_state_verifier_or_code() {
        let challenge = OAuthChallenge::generate();
        let rendered = format!("{challenge:?}");
        assert!(!rendered.contains(challenge.state.expose()));
        assert!(!rendered.contains(challenge.code_verifier.expose()));
        let outcome = CallbackOutcome::Code(SecretString::new(SENTINEL));
        assert_eq!(format!("{outcome:?}"), "CallbackOutcome::Code([REDACTED])");
    }

    #[test]
    fn authorization_url_is_fixed_and_config_is_bounded() {
        let challenge = OAuthChallenge::generate();
        let redirect = loopback_redirect_uri(51_234).expect("loopback");
        let client_id = "1234567890-abc.apps.googleusercontent.com";
        assert_eq!(redirect, "http://127.0.0.1:51234/oauth2/callback");
        let url = authorization_url(client_id, &redirect, &challenge).expect("url");
        let parsed = reqwest::Url::parse(&url).unwrap();
        assert_eq!(
            parsed.origin().ascii_serialization(),
            "https://accounts.google.com"
        );
        assert_eq!(parsed.path(), "/o/oauth2/v2/auth");
        let values: std::collections::HashMap<_, _> = parsed.query_pairs().collect();
        assert_eq!(values.get("redirect_uri").unwrap().as_ref(), redirect);
        assert_eq!(
            values.get("scope").unwrap().as_ref(),
            GOOGLE_DRIVE_READONLY_SCOPE
        );
        assert_eq!(
            values.get("state").unwrap().as_ref(),
            challenge.state.expose()
        );
        assert_eq!(
            values.get("code_challenge_method").unwrap().as_ref(),
            "S256"
        );
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A"));
        let zero_port = loopback_redirect_uri(0).unwrap_err();
        assert_eq!(zero_port.code().as_str(), CONFIG_INVALID);
        let blank = authorization_url(" ", &redirect, &challenge).unwrap_err();
        assert_eq!(blank.code().as_str(), NOT_CONFIGURED);
        let long_client_id = "a".repeat(MAX_CLIENT_ID_LEN + 1);
        for bad_client in ["id&x=1", "id?x=1", long_client_id.as_str()] {
            assert!(authorization_url(bad_client, &redirect, &challenge).is_err());
        }
        for bad_redirect in [
            "https://127.0.0.1:8080/oauth2/callback",
            "http://localhost:8080/oauth2/callback",
            "http://127.0.0.1:8080",
            "http://user@127.0.0.1:8080/oauth2/callback",
            "http://127.0.0.1.evil.example:8080/oauth2/callback",
            "http://127.0.0.1:0/oauth2/callback",
            "http://127.0.0.1:8080/other",
            "http://127.0.0.1:8080/oauth2/callback?code=x",
        ] {
            let err = authorization_url(client_id, bad_redirect, &challenge).unwrap_err();
            assert_eq!(err.code().as_str(), CONFIG_INVALID, "{bad_redirect}");
        }
    }

    #[test]
    fn callback_returns_code_denial_or_sanitized_provider_error() {
        let challenge = OAuthChallenge::generate();
        let state = challenge.state.expose();
        let query = format!(
            "?code=4%2F0Aabc-def_ghi&state={state}&scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fdrive.readonly\
             &authuser=0&prompt=consent&hd=example.com&future_field=x"
        );
        match challenge.parse_callback(&query) {
            Ok(CallbackOutcome::Code(code)) => assert_eq!(code.expose(), "4/0Aabc-def_ghi"),
            other => panic!("期望授权码，得到 {other:?}"),
        }
        let plus = format!("code=a%2Bb&state={state}&scope=first+second");
        match challenge.parse_callback(&plus).unwrap() {
            CallbackOutcome::Code(code) => assert_eq!(code.expose(), "a+b"),
            _ => panic!("encoded plus must survive"),
        }
        assert_eq!(percent_decode("first+second").unwrap(), "first second");
        let denied = format!("error=access_denied&state={state}&error_description={SENTINEL}");
        assert!(matches!(
            challenge.parse_callback(&denied),
            Ok(CallbackOutcome::Denied)
        ));
        let refused = format!("error=invalid_scope&state={state}&error_description={SENTINEL}");
        let err = challenge.parse_callback(&refused).unwrap_err();
        assert_eq!(err.code().as_str(), PROVIDER_REJECTED);
        assert!(!format!("{err:?}").contains(SENTINEL));
    }

    #[test]
    fn callback_rejects_every_invalid_query() {
        let challenge = OAuthChallenge::generate();
        let state = challenge.state.expose();
        let queries = [
            format!("code={SENTINEL}&state=WRONG"),
            format!("code={SENTINEL}"),
            format!("code={SENTINEL}&state={state}&state={state}"),
            format!("code={SENTINEL}&code={SENTINEL}&state={state}"),
            format!("error=access_denied&error=access_denied&state={state}"),
            format!("code={SENTINEL}&error=access_denied&state={state}"),
            format!("state={state}"),
            format!("code=&state={state}"),
            format!("code={SENTINEL}&state={state}&x=%ZZ"),
            format!("code={SENTINEL}&state={state}&x=%00"),
            format!("code={}&state={state}", "A".repeat(MAX_CODE_LEN + 1)),
            format!("state={state}&pad={}", "b".repeat(MAX_QUERY_LEN)),
            format!("state={state}{}", "&pad=1".repeat(MAX_QUERY_PARAMS)),
            format!("state={state}&pad={}", "c".repeat(MAX_EXTRA_VALUE_LEN + 1)),
            format!(
                "state={state}&{}={}",
                "n".repeat(MAX_PARAM_NAME_LEN + 1),
                "v"
            ),
        ];
        for query in queries {
            rejected(&challenge, &query);
        }
        let at_limit = format!(
            "code=abc&state={state}&scope={}",
            "c".repeat(MAX_EXTRA_VALUE_LEN)
        );
        assert!(matches!(
            challenge.parse_callback(&at_limit),
            Ok(CallbackOutcome::Code(_))
        ));
    }
}
