//! Domain ↔ Wire Projection 映射（BE-MAPPER-001）。
//!
//! 规范：FRONTEND_BACKEND_CONTRACT.md §11–§12、§22；ADR-002（Domain Entity 与 DB Row 不出 IPC）。
//! 职责：纯函数映射（无 IO、无 Repository）；数据组装由 Application Service 负责。
//! 铁律：映射不改变领域语义；时间统一 UTC RFC 3339；枚举转 wire snake_case。

pub mod locator;
pub mod progress;
pub mod time;
pub mod work_card;

pub use locator::locator_to_dto;
pub use progress::progress_summary;
pub use time::utc_millis_to_rfc3339;
pub use work_card::{WorkCardInput, primary_action};
