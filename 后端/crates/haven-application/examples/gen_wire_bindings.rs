//! 生成器调用：写盘 wire.ts（IPC-WIRE-001）。
//!
//! 运行：`cargo run -p haven-application --example gen_wire_bindings`
//! 一致性由 `tests/wire_bindings_consistency.rs` 保证（生成结果 vs checked-in 文件）。

use haven_application::wire::{WIRE_TS_RELATIVE_PATH, generate_wire_bindings};
use std::path::PathBuf;

fn main() {
    let out = generate_wire_bindings();
    let target =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../../{WIRE_TS_RELATIVE_PATH}"));
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).expect("创建生成目录失败");
    }
    std::fs::write(&target, out).expect("写入 wire.ts 失败");
    println!("wire.ts generated -> {}", target.display());
}
