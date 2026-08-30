//! 结构化错误模型。
//!
//! 规范：`plan/TECHNICAL_ARCHITECTURE.md` §55–§56。
//! 原则：
//! - 所有跨层错误结构化，携带稳定 ErrorCode。
//! - UI 只收到 code / user_message / retryable；详细 cause 只写日志。

use std::fmt;
use std::sync::Arc;

/// 稳定错误分类。IPC 序列化为 SCREAMING_SNAKE_CASE（如 `NOT_FOUND`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorKind {
    Validation,
    NotFound,
    AlreadyExists,
    Conflict,
    Io,
    Network,
    Timeout,
    Database,
    Parse,
    Unauthorized,
    Forbidden,
    Unsupported,
    Security,
    Cancelled,
    Storage,
    Source,
    Download,
    Sync,
    Update,
    Internal,
}

impl ErrorKind {
    pub fn code(&self) -> &'static str {
        match self {
            ErrorKind::Validation => "VALIDATION",
            ErrorKind::NotFound => "NOT_FOUND",
            ErrorKind::AlreadyExists => "ALREADY_EXISTS",
            ErrorKind::Conflict => "CONFLICT",
            ErrorKind::Io => "IO",
            ErrorKind::Network => "NETWORK",
            ErrorKind::Timeout => "TIMEOUT",
            ErrorKind::Database => "DATABASE",
            ErrorKind::Parse => "PARSE",
            ErrorKind::Unauthorized => "UNAUTHORIZED",
            ErrorKind::Forbidden => "FORBIDDEN",
            ErrorKind::Unsupported => "UNSUPPORTED",
            ErrorKind::Security => "SECURITY",
            ErrorKind::Cancelled => "CANCELLED",
            ErrorKind::Storage => "STORAGE",
            ErrorKind::Source => "SOURCE",
            ErrorKind::Download => "DOWNLOAD",
            ErrorKind::Sync => "SYNC",
            ErrorKind::Update => "UPDATE",
            ErrorKind::Internal => "INTERNAL",
        }
    }
}

/// 稳定错误代码（对外契约，不可随意变更）。
/// 示例：`LIBRARY_SCAN_FAILED`、`SOURCE_TIMEOUT`、`DOWNLOAD_DISK_FULL`。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ErrorCode(pub String);

impl ErrorCode {
    pub fn new(code: impl Into<String>) -> Self {
        Self(code.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 结构化应用错误。
#[derive(Clone)]
pub struct AppError {
    code: ErrorCode,
    kind: ErrorKind,
    message: String,
    retryable: bool,
    context_id: Option<String>,
    source: Option<Arc<dyn std::error::Error + Send + Sync>>,
}

impl AppError {
    pub fn new(
        code: impl Into<String>,
        kind: ErrorKind,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            code: ErrorCode(code.into()),
            kind,
            message: message.into(),
            retryable,
            context_id: None,
            source: None,
        }
    }

    pub fn with_context(mut self, context_id: impl Into<String>) -> Self {
        self.context_id = Some(context_id.into());
        self
    }

    pub fn with_source<E>(mut self, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        self.source = Some(Arc::new(source));
        self
    }

    pub fn code(&self) -> &ErrorCode {
        &self.code
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn retryable(&self) -> bool {
        self.retryable
    }

    pub fn context_id(&self) -> Option<&str> {
        self.context_id.as_deref()
    }

    /// 提供给 UI 的用户可读消息（不含内部细节）。
    pub fn user_message(&self) -> &str {
        &self.message
    }
}

impl fmt::Debug for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppError")
            .field("code", &self.code.0)
            .field("kind", &self.kind)
            .field("message", &self.message)
            .field("retryable", &self.retryable)
            .field("context_id", &self.context_id)
            .finish()
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code.0, self.message)
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|s| s.as_ref() as &(dyn std::error::Error + 'static))
    }
}

/// IPC 输出 DTO：UI 只拿到这三样。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ErrorDto {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl From<&AppError> for ErrorDto {
    fn from(e: &AppError) -> Self {
        Self {
            code: e.code.0.clone(),
            message: e.message.clone(),
            retryable: e.retryable,
        }
    }
}

// ---- 便捷构造器 ----

pub fn validation(msg: impl Into<String>) -> AppError {
    AppError::new("VALIDATION", ErrorKind::Validation, msg, false)
}

pub fn not_found(msg: impl Into<String>) -> AppError {
    AppError::new("NOT_FOUND", ErrorKind::NotFound, msg, false)
}

pub fn internal(msg: impl Into<String>) -> AppError {
    AppError::new("INTERNAL", ErrorKind::Internal, msg, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_roundtrip_through_dto() {
        let err = AppError::new("SOURCE_TIMEOUT", ErrorKind::Timeout, "来源请求超时", true);
        let dto: ErrorDto = (&err).into();
        assert_eq!(dto.code, "SOURCE_TIMEOUT");
        assert!(dto.retryable);
        assert_eq!(dto.message, "来源请求超时");
    }

    #[test]
    fn kind_serializes_as_screaming_snake() {
        let json = serde_json::to_string(&ErrorKind::NotFound).unwrap();
        assert_eq!(json, "\"NOT_FOUND\"");
    }
}
