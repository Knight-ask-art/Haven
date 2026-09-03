//! haven-infrastructure: 基础设施（SQLite、迁移、凭据存储、本地扫描；后续：HTTP、Storage Provider）。

pub mod app_info;
pub mod artwork_cache;
pub mod cast;
pub mod cms10;
pub mod comic;
pub mod credential;
pub mod db;
pub mod download;
pub mod epub;
pub mod error_report;
mod http_security;
pub mod metadata_sources;
pub mod online_sources;
pub mod opds;
pub mod reader_search;
pub mod scanner;
pub mod trending;
pub mod video_screenshot;

pub use credential::credential_store;
pub use db::Db;
