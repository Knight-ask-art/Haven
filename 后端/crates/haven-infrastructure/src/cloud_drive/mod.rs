//! Google Drive / OAuth 只读接入；固定端点 HTTP、握手、令牌、Drive 适配器与生产组合各自承载职责。

pub mod auth;
pub mod drive;
pub mod factory;
pub(crate) mod handshake;
pub(crate) mod http;
pub(crate) mod tokens;

pub use factory::GoogleDrivePorts;
