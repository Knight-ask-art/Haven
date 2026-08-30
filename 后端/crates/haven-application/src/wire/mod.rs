//! Wire DTO（IPC 边界类型）。
//!
//! 规范：`plan/FRONTEND_BACKEND_CONTRACT.md` §11–§12、§22；ADR-002 §DTO 放置。
//! - JSON 字段统一 `camelCase`；枚举统一小写 `snake_case`；ID 为 string。
//! - DTO 是页面 Read Model，不是 Domain Entity；Domain Entity 与 DB Row 不出 IPC。
//! - 时间字段为 UTC RFC 3339 字符串（转换由 BE-MAPPER-001 完成）。
//! - 类型为单一事实源：`ts-rs` 生成 TypeScript Binding（`examples/gen_wire_bindings.rs`）。

pub mod dto;
mod generate;

pub use dto::*;
pub use generate::generate_wire_bindings;

/// 生成的 wire.ts 相对 workspace 根路径（一致性测试与 example 共用）。
pub const WIRE_TS_RELATIVE_PATH: &str = "前端/app/src/lib/ipc/generated/wire.ts";
