//! 云盘 CloudStorageService / CloudBrowseService 的离线跨层集成测试。
//!
//! 真实 SQLite（内存库 + 完整迁移）、真实仓储与四端口编排；网络、keystore、OAuth 与
//! 浏览器全部替换为确定性替身，不触网、不睡眠、不启动浏览器。

#[path = "cloud_storage/browse.rs"]
mod browse;
#[path = "cloud_storage/cleanup.rs"]
mod cleanup;
#[path = "cloud_storage/cloud_session.rs"]
mod cloud_session;
#[path = "cloud_storage/lifecycle.rs"]
mod lifecycle;
#[path = "cloud_storage/pdf.rs"]
mod pdf;
#[path = "cloud_storage/support.rs"]
mod support;

#[path = "cloud_storage/fake_drive.rs"]
mod fake_drive;
