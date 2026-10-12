//! 一次 OPDS 请求的**权威上下文**与凭据注入判定。
//!
//! 出站策略、来源身份与「凭据只允许发往哪个 origin」在这里固定成一份可比较的快照，
//! 这样异步凭据解析前后的复核（`OpdsClient::resolve_credential`）只需要比对同一个
//! 结构，而不需要重新推理来源身份。生产组合根之外的调用方都拿不到这里的东西。

use haven_application::services::source_registry::RegisteredSourceRequestSnapshot;
use haven_common::network::{HttpUrlPolicy, url_origin};
use haven_common::{AppError, ErrorKind};

pub(super) fn opds_source_denied() -> AppError {
    AppError::new(
        "INVALID_ARGUMENT",
        ErrorKind::Validation,
        "未知来源目录",
        false,
    )
}

/// 已配置凭据的来源不允许在明文 HTTP 上发请求：在**发出请求之前**就以固定错误
/// fail closed，而不是把 Basic secret 交给一层可被旁听的传输。
pub(super) fn authenticated_plaintext_denied() -> AppError {
    AppError::new(
        "SECURITY_POLICY_DENIED",
        ErrorKind::Security,
        "该来源已配置访问凭据，目录地址必须使用 HTTPS",
        false,
    )
}

/// 异步凭据解析期间来源上下文已经变化：本次请求 fail closed。
pub(super) fn opds_request_context_changed() -> AppError {
    AppError::new(
        "SOURCE_UNAVAILABLE",
        ErrorKind::Network,
        "来源配置在请求期间已变更，请重试",
        true,
    )
}

/// OPDS 来源身份。
///
/// 只有**注册表里确实登记为自定义 OPDS 书源**的身份才使用用户来源网络策略，只有
/// **内置 Gutenberg** 才使用固定的公开 HTTPS 主机白名单；其余身份要么匿名（没有来源
/// 上下文），要么直接 fail closed。身份只由注册表与内置常量决定，绝不按 `sourceId`
/// 的名字或前缀猜权限。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OpdsSourceIdentity {
    /// 无来源上下文（匿名请求）：固定严格策略、不做凭据解析。
    Anonymous,
    /// 内置 Gutenberg：匿名、固定公开 HTTPS 主机白名单、不允许显式端口。
    BuiltinGutenberg,
    /// 已登记的自定义 OPDS 书源：用户来源策略 + 只在权威登记 origin 上注入凭据。
    RegisteredCustomOpds,
}

/// 一次 OPDS 请求的受控上下文。
///
/// 它把「这次请求是谁、按哪个策略出站、凭据只允许发往哪个 origin」固定成**一份**快照。
/// 异步凭据解析之后的复核（`OpdsClient::resolve_credential`）用它判断上下文是否仍是
/// 同一份，因此解析期间的端点/种类/版本变化不会把新 secret 发往旧端点选定的地址。
/// `Debug` 只打印身份与形状：登记端点、配置版本与来源身份都不进日志或错误文案。
#[derive(Clone, PartialEq, Eq)]
pub(super) struct OpdsRequestContext {
    identity: OpdsSourceIdentity,
    /// 请求路由用的来源键（Basic Auth 的用户名段，与既有凭据格式一致）。只留在后端
    /// 内存与出站请求头里，不出现在 `Debug`、错误文案或 Wire。
    source_id: Option<String>,
    /// 已登记来源的只读快照（仅 `RegisteredCustomOpds` 有值）。
    snapshot: Option<RegisteredSourceRequestSnapshot>,
}

impl OpdsRequestContext {
    pub(super) fn anonymous() -> Self {
        Self {
            identity: OpdsSourceIdentity::Anonymous,
            source_id: None,
            snapshot: None,
        }
    }

    pub(super) fn builtin_gutenberg(source_id: &str) -> Self {
        Self {
            identity: OpdsSourceIdentity::BuiltinGutenberg,
            source_id: Some(source_id.to_owned()),
            snapshot: None,
        }
    }

    pub(super) fn registered_custom_opds(
        source_id: &str,
        snapshot: RegisteredSourceRequestSnapshot,
    ) -> Self {
        Self {
            identity: OpdsSourceIdentity::RegisteredCustomOpds,
            source_id: Some(source_id.to_owned()),
            snapshot: Some(snapshot),
        }
    }

    pub(super) fn source_id(&self) -> Option<&str> {
        self.source_id.as_deref()
    }

    /// 本次请求的身份（只读；身份本身不含任何 secret 或地址）。
    pub(super) fn identity(&self) -> OpdsSourceIdentity {
        self.identity
    }

    /// 本次请求使用的共享网络策略。
    ///
    /// 用户来源端口策略只发给**正向确认过的**自定义 OPDS 书源；未登记、未知与其它
    /// `custom_` 家族的身份根本构造不出上下文。
    pub(super) fn policy(&self) -> HttpUrlPolicy {
        match self.identity {
            OpdsSourceIdentity::RegisteredCustomOpds => HttpUrlPolicy::UserProvidedSource,
            OpdsSourceIdentity::Anonymous | OpdsSourceIdentity::BuiltinGutenberg => {
                HttpUrlPolicy::SourceEndpoint
            }
        }
    }

    /// 该来源当前是否配置了归属自己的凭据引用（不代表 secret 已读出）。
    pub(super) fn credential_configured(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.credential_configured())
    }

    /// 登记端点的权威 origin（`scheme://host[:port]`）。
    pub(super) fn registered_origin(&self) -> Option<&str> {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.endpoint_origin())
    }

    /// 这次请求在异步区间之后是否仍由同一份权威上下文授权。
    pub(super) fn is_current_with(&self, other: &Self) -> bool {
        if self.identity != other.identity {
            return false;
        }
        match (self.snapshot.as_ref(), other.snapshot.as_ref()) {
            (Some(current), Some(next)) => current.is_current_with(next),
            (None, None) => true,
            _ => false,
        }
    }

    /// 该 URL 是否仍在本次请求的权威来源上（重定向护栏）。
    pub(super) fn authorizes_url(&self, url: &reqwest::Url) -> bool {
        match self.identity {
            // 没有来源上下文时保持既有行为：逐跳过策略，但没有来源护栏。
            OpdsSourceIdentity::Anonymous => true,
            OpdsSourceIdentity::BuiltinGutenberg => url
                .host_str()
                .map(str::to_ascii_lowercase)
                .is_some_and(|host| {
                    matches!(host.as_str(), "www.gutenberg.org" | "m.gutenberg.org")
                }),
            OpdsSourceIdentity::RegisteredCustomOpds => self
                .registered_origin()
                .is_some_and(|registered| url_origin(url).as_deref() == Some(registered)),
        }
    }

    /// 这个身份是否是注册表正向确认过的自定义 OPDS 书源。
    pub(super) fn is_registered_custom_opds(&self) -> bool {
        self.identity == OpdsSourceIdentity::RegisteredCustomOpds
    }
}

impl std::fmt::Debug for OpdsRequestContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpdsRequestContext")
            .field("identity", &self.identity)
            .field("endpoint_configured", &self.registered_origin().is_some())
            .field("credential_configured", &self.credential_configured())
            .finish_non_exhaustive()
    }
}

/// 该跳是否允许带上已配置的凭据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HopCredential {
    /// 匿名发送：未配置凭据，或这一跳不在权威登记 origin 上。
    Anonymous,
    /// 允许注入凭据：权威登记 origin + HTTPS + 已配置凭据同时成立。
    Authorized,
}

/// 这一跳是否允许带凭据。
///
/// 凭据只在**登记端点的权威 origin** 上注入，而不是在「本次请求碰巧用的那个地址」
/// 上：被篡改的 OpenSearch 描述文件、目录链接或重定向因此无法把 Basic secret 引到
/// 另一个 origin。配了凭据的来源在明文 HTTP 上直接 fail closed。
pub(super) fn hop_credential(
    context: &OpdsRequestContext,
    target: &reqwest::Url,
) -> Result<HopCredential, AppError> {
    if !context.is_registered_custom_opds() || !context.credential_configured() {
        return Ok(HopCredential::Anonymous);
    }
    let Some(registered) = context.registered_origin() else {
        return Ok(HopCredential::Anonymous);
    };
    if url_origin(target).as_deref() != Some(registered) {
        return Ok(HopCredential::Anonymous);
    }
    if target.scheme() != "https" {
        return Err(authenticated_plaintext_denied());
    }
    Ok(HopCredential::Authorized)
}
