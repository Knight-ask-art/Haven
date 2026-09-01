// Windows 发布版和本地运行版都使用 GUI subsystem，避免启动栖阅时额外弹出
// 一个承载 Rust stdout/stderr 的命令窗口。诊断信息仍由应用自身的错误报告
// 链路处理，不依赖控制台窗口。
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() {
    haven_tauri_lib::run()
}
