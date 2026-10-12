//! 来源健康切片用例的内聚分组。
//!
//! - [`fixtures`]：共享夹具（真实 SQLite + 真实 `SettingsUoW` 的注册表、观测构造、落库读取、
//!   凭据端口替身）。没有断言。
//! - [`persistence`]：观测 → 投影/落库的持久化事实（指标、有界窗口、重启恢复、只读面不伪造）。
//! - [`invalidation`]：配置变更（端点 A→B→A、凭据、删除、乱序完成）使旧观测作废。
//! - [`transactions`]：并发配置变更与健康写入共用事务纪律的原子性回归。
//! - [`dispatch`]：从公开分发入口（`SearchSourceService::start` / `cancel`）驱动真实搜索观测。

pub mod dispatch;
pub mod fixtures;
pub mod invalidation;
pub mod persistence;
pub mod transactions;
