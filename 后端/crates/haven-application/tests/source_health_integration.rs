//! Haven 来源健康切片（契约 §36.2 / §36.3）的集成测试装配点。
//!
//! 为什么这些用例必须是 integration test，而不是 `src/**` 里的 `#[cfg(test)]` 模块：
//! `SqliteSettingsUoW`（haven-infrastructure）实现的是 **normal** haven-application
//! 实例里的 `SettingsUoW`。`cargo test` 会把 lib 再编译出一个 lib-test 实例，两个
//! crate 实例里的 `SettingsUoW` 不是同一个 item，于是 `Arc::new(SqliteSettingsUoW)`
//! 无法满足 `Arc<dyn SettingsUoW>`（E0277）——这正是「真实 SQLite + 真实事务」的用例
//! 不能留在单元测试里的原因。integration test 链接的是 normal 实例（haven-infrastructure
//! 看到的也是它），所以真实 UoW 可以照常注入，与 `tests/settings_integration.rs` 同一模式。
//!
//! 用例按内聚分组放在 `source_health/` 子模块里：本文件只声明模块，不含断言。

mod source_health;
