//! 栖阅 Haven Tauri 壳（IPC-TAURI-001A）。
//!
//! Composition Root：setup 打开 DB → AppState 组装 Services → invoke_handler 注册命令。
//! 命令清单必须与 `capabilities/main.json` 保持一致（IPC-TAURI-001A 验收）。

pub mod commands;
pub mod download_sink;
pub mod ipc;
pub mod reader_search_sink;
mod resource_protocol;
pub mod scan_sink;
pub mod search_sink;
pub(crate) mod session_registry;
pub mod state;
pub mod stream_registry;

// 命令名单唯一事实源（R-MAIN-06：与 build.rs/capability 测试同源，防漂移）。
// 由宏展开生成常量（rustdoc 不为其生成文档，故用普通注释）。
include!("../command-manifest.rs");
pub const COMMAND_MANIFEST: &[(&str, &str)] = TARGET_COMMANDS;

use tauri::Manager;

use haven_infrastructure::Db;
use state::AppState;

/// 注册 invoke_handler。命令集合由 `command-manifest.rs` 单真源驱动：
/// `registered_handlers!()` 展开为同一宏派生的 `tauri::generate_handler![...]`，
/// **不再手写第二份命令列表**（阻塞一）。capability/ACL 生成与测试亦由同一 manifest 派生。
pub fn register_invoke_handler<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.invoke_handler(registered_handlers!())
}

/// 应用入口（tauri::Builder 组装）。
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_window_state::Builder::default().build())
        // Updater only accepts signed HTTPS metadata configured in tauri.conf.json.
        // The signing private key is supplied by the release workflow, never bundled.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                if let Some(state) = window.app_handle().try_state::<AppState>() {
                    state.video_screenshot.cancel_owner(window.label());
                    let _ = state.session_registry.remove_window(window.label());
                }
            }
        });
    register_invoke_handler(crate::resource_protocol::register_resource_protocol(
        builder,
    ))
    .setup(|app| {
        // 数据库在应用数据目录打开一次；后续全部复用（禁止第二套 DB）。
        let data_dir = app
            .path()
            .app_data_dir()
            .map_err(|e| tauri::Error::AssetNotFound(format!("无法解析应用数据目录: {e}")))?;
        std::fs::create_dir_all(&data_dir)
            .map_err(|e| tauri::Error::AssetNotFound(format!("无法创建应用数据目录: {e}")))?;
        let db_path = data_dir.join("haven.db");
        let db = Arc::new(Db::open(&db_path).map_err(|e| {
            tauri::Error::AssetNotFound(format!("无法打开数据库: {}", e.user_message()))
        })?);
        let state = AppState::try_new(db).map_err(|e| {
            tauri::Error::AssetNotFound(format!("无法初始化应用状态: {}", e.user_message()))
        })?;
        // metadata.changed 广播出口绑定 AppHandle（契约 §36.8）。
        state.metadata_sink.bind(app.handle().clone());
        let download = state.download.clone();
        app.manage(state);
        tauri::async_runtime::spawn(async move {
            // 启动恢复失败不会阻止 UI 打开；任务仍保持可解释状态，可在下载页重试。
            let _ = download.resume_startable().await;
        });
        Ok(())
    })
    .run(tauri::generate_context!())
    .expect("运行栖阅 Haven 失败");
}

// 供 main.rs 调用的 std::sync::Arc 别名（Rust 1.85 无 std Arc 需显式 use）。
use std::sync::Arc;
