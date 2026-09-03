//! haven-common: 跨层共享（错误模型、时间戳、通用工具）。

pub mod error;
pub mod network;
pub mod time;
pub mod tokenizer;

pub use error::{AppError, ErrorCode, ErrorDto, ErrorKind, internal, not_found, validation};
pub use time::UtcMillis;
