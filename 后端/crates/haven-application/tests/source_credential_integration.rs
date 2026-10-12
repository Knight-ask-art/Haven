//! Haven 自定义源凭据生命周期（版本槽 + 持久化清理队列）的集成测试装配点。
//!
//! 与 `source_health_integration.rs` 同一模式：这些用例必须在 integration test 里跑，
//! 因为 `SqliteSettingsUoW`（haven-infrastructure）实现的是 normal haven-application
//! 实例里的 `SettingsUoW`，lib-test 实例无法注入真实事务。用例按内聚分组放在
//! `source_credential/` 子模块里：本文件只声明模块，不含断言。

mod source_credential;
