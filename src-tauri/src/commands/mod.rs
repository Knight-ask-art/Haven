//! Tauri Command 层（IPC-TAURI-001A/B）。
//!
//! Command 职责边界（ADR-002）：
//! - 不可信输入校验（类型/范围校验在 Service 或命令层手动反序列化）。
//! - DTO 映射与 Service 调用；错误统一映射为 ErrorDto。
//! - **禁止**在 Command 内写 SQL 或触碰 DB Row。
//! - favorite_set 只发布 changed=true 的 favorite.changed。

pub mod app_info;
pub mod cache;
pub mod cast;
pub mod comic;
pub mod credential;
pub mod download;
pub mod enrichment;
pub mod error_report;
pub mod favorite;
pub mod history;
pub mod home;
pub mod library;
pub mod marker;
pub mod progress;
pub mod reader;
pub mod resource;
pub mod resource_preferences;
pub mod scan;
pub mod search_history;
pub mod search_source;
pub mod session;
pub mod settings;
pub mod source_custom;
pub mod source_registry;
pub mod source_runtime;
pub mod storage_location;
pub mod trending;
pub mod video_screenshot;
pub mod work;
