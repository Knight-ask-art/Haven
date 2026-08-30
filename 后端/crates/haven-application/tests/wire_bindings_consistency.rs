//! 生成物一致性测试（IPC-WIRE-001 验收项：生成物可复现、无漂移）。
//!
//! 生成结果必须与 checked-in `wire.ts` 完全一致：
//! - Rust 源新增/修改 Wire DTO 后，本测试失败 → 运行 example 重新生成并提交。
//! - 防止"声明导出但未落盘"的漂移（审查发现的问题 1）。

use std::path::PathBuf;

use haven_application::wire::{WIRE_TS_RELATIVE_PATH, generate_wire_bindings};

#[test]
fn checked_in_wire_ts_matches_generated_output() {
    let generated = generate_wire_bindings();
    let checked_in_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../../{WIRE_TS_RELATIVE_PATH}"));
    let checked_in = std::fs::read_to_string(&checked_in_path)
        .unwrap_or_else(|e| panic!("读取 checked-in wire.ts 失败: {e}"));
    if generated != checked_in {
        panic!(
            "wire.ts 与 Rust 源漂移。请运行 `cargo run -p haven-application --example gen_wire_bindings` 重新生成并提交。\n\
             生成 {generated_len} 字节 vs checked-in {checked_in_len} 字节",
            generated_len = generated.len(),
            checked_in_len = checked_in.len(),
        );
    }
}
