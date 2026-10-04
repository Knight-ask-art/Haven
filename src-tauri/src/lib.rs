//! 栖阅 Haven Tauri 壳（IPC-TAURI-001A）。
//!
//! Composition Root：setup 打开 DB → AppState 组装 Services → invoke_handler 注册命令。
//! 命令清单必须与 `capabilities/main.json` 保持一致（IPC-TAURI-001A 验收）。

pub mod agent_broker;
mod cloud_background;
pub mod cloud_oauth_browser;
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

// 供 main.rs 调用的 std::sync::Arc 别名（Rust 1.85 无 std Arc 需显式 use）。
use std::sync::Arc;
use tauri::Manager;

use haven_infrastructure::Db;
use session_registry::ClosedSession;
use state::AppState;

/// 注册 invoke_handler。命令集合由 `command-manifest.rs` 单真源驱动：
/// `registered_handlers!()` 展开为同一宏派生的 `tauri::generate_handler![...]`，
/// **不再手写第二份命令列表**（阻塞一）。capability/ACL 生成与测试亦由同一 manifest 派生。
pub fn register_invoke_handler<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.invoke_handler(registered_handlers!())
}

/// 关闭路径的公共尾部：**先**撤销流授权，**再**等阅读记录落地。
///
/// 顺序是安全契约，不是风格：撤销是安全能力，统计只是 best-effort 旁路。反过来写，
/// 一次慢查询、一次写失败，或进程在等待期间退出，都足以让远端流的播放授权继续存活。
/// 两条关闭路径（用户关窗、外部销毁）共用同一个尾部，顺序也就不会各自漂移。
pub(crate) async fn revoke_then_record(
    state: &AppState,
    owner_window_label: &str,
    closed_sessions: &[ClosedSession],
) {
    // 一轮撤销、再一轮登记：授权撤销不跟任何一次统计写排队，全部撤销都发生在
    // 第一次 await 之前。
    for closed in closed_sessions {
        state
            .stream_registry
            .revoke(&closed.session_id, owner_window_label);
    }
    for closed in closed_sessions {
        commands::session::record_closed_session(state, closed).await;
    }
}

/// 用户关窗的收尾：撤销授权 → 等统计写 → 销毁窗口。
///
/// `destroy()` 必须留在最后一行：最后一个窗口的销毁会让整个进程退出，提前销毁
/// 就等于让上面的等待白等——用户关掉应用前读的最后一段阅读就丢了。
/// 调用方（`CloseRequested` 分支）在此之前已经阻止了默认销毁，窗口因此活到这一刻。
///
/// 收尾用的是 `destroy` 而不是 `close`：`close` 会再走一遍 `CloseRequested`，
/// 而这里的目的恰恰是「不再征询、直接销毁」。万一销毁失败，窗口还在、lease 也已
/// 释放，用户再点一次关闭会拿到一个空会话集合的新事务，重试一遍销毁。
async fn drain_window_close<R: tauri::Runtime>(
    state: &AppState,
    owner_window_label: &str,
    closed_sessions: Vec<ClosedSession>,
    window: &tauri::Window<R>,
) {
    // Repository operations use synchronous SQLite behind async ports. Keep
    // that work off Tauri's async executor while the prevented window remains
    // alive, then destroy only after the worker has completed.
    let state = state.clone();
    let owner_window_label = owner_window_label.to_owned();
    let worker_window_label = owner_window_label.clone();
    if let Err(error) = tauri::async_runtime::spawn_blocking(move || {
        tauri::async_runtime::block_on(revoke_then_record(
            &state,
            &worker_window_label,
            &closed_sessions,
        ))
    })
    .await
    {
        eprintln!("[close] 阅读记录 blocking worker 失败: {error}");
    }
    if let Err(error) = window.destroy() {
        eprintln!("[close] 销毁窗口 {owner_window_label} 失败: {error}");
    }
}

/// 应用入口（tauri::Builder 组装）。
fn application_context() -> tauri::Context<tauri::Wry> {
    tauri::generate_context!()
}

/// Read-only installer provenance probe. It uses the same context as the real
/// application and does not open user data, initialize services or create UI.
pub fn embedded_frontend_build_info() -> Option<String> {
    let context = application_context();
    let bytes = context.assets().get(&"build-info.json".into())?;
    String::from_utf8(bytes.into_owned()).ok()
}

pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_window_state::Builder::default().build())
        // Updater only accepts signed HTTPS metadata configured in tauri.conf.json.
        // The signing private key is supplied by the release workflow, never bundled.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .on_window_event(|window, event| match event {
            // 用户关窗：这是**唯一**能等到「撤销 + 统计」落地再退出的时机。
            // 先阻止默认销毁，把该窗口的会话能力（连同结束时刻）在一次同步摘除里
            // 定下来，再让异步任务做完撤销与登记，最后才真正销毁窗口。
            tauri::WindowEvent::CloseRequested { api, .. } => {
                let Some(state) = window.app_handle().try_state::<AppState>() else {
                    // 没有应用状态就没什么可等的，放行默认销毁，别把窗口留住。
                    return;
                };
                // 无论下面走哪条分支都先阻止默认销毁：窗口要么由本次事务销毁，
                // 要么交给已经在进行中的那一次，绝不在这里被默认行为带走。
                api.prevent_close();
                let owner_window_label = window.label().to_owned();
                let Some(mut lease) = state
                    .session_registry
                    .begin_window_close(&owner_window_label)
                else {
                    // 本窗口已经有一次关闭事务在进行中（用户连点两次关闭）：这次请求
                    // 必须什么都不做。它只会摘到一个空会话集合，然后立刻销毁窗口，
                    // 把第一次事务还在等的统计写连同进程一起带走——那正是「关掉应用
                    // 丢掉最后一段阅读」的成因。
                    return;
                };
                // 截图上传的临时字节跟着窗口一起收掉（与销毁兜底路径一致）。
                state.video_screenshot.cancel_owner(&owner_window_label);
                let closed_sessions = lease.take_closed_sessions();
                let close_owner_window_label = lease.owner_window_label().to_owned();
                let state = (*state.inner()).clone();
                let window = window.clone();
                tauri::async_runtime::spawn(async move {
                    // owner 身份取自关闭事务本身：撤销流授权用的就是这次事务识别出的
                    // 那个窗口，不在别处再推导一遍。
                    drain_window_close(&state, &close_owner_window_label, closed_sessions, &window)
                        .await;
                    // 关闭事务到这一刻才算完整结束：窗口已经销毁，占位随 lease 释放。
                    // 提前释放会让重复的关闭请求立刻销毁窗口；一直不放又会挡住
                    // 销毁后重建的同名窗口。
                    drop(lease);
                });
            }
            // 非用户关窗的销毁（外部 destroy、进程退出、平台强制关闭）：窗口已经
            // 不存在了，没有可以等待的时机。会话能力仍在这里如实摘除（结束时刻同样
            // 在摘除那一刻取），但登记只能挂在 detached 任务上尽力而为——进程如果在
            // 它跑完之前退出，这段阅读记录就丢了。这是外部销毁路径真实存在的上限；
            // 正常的用户关窗不走这里，那时会话已经被上一个分支摘走，这里拿到的是空
            // 集合，而撤销与登记都是幂等的。
            tauri::WindowEvent::Destroyed => {
                if let Some(state) = window.app_handle().try_state::<AppState>() {
                    let _ = state.update_preparation.release(window.label());
                    state.video_screenshot.cancel_owner(window.label());
                    let owner_window_label = window.label().to_owned();
                    let closed_sessions = state.session_registry.remove_window(&owner_window_label);
                    let state = (*state.inner()).clone();
                    tauri::async_runtime::spawn(async move {
                        drop(tauri::async_runtime::spawn_blocking(move || {
                            tauri::async_runtime::block_on(revoke_then_record(
                                &state,
                                &owner_window_label,
                                &closed_sessions,
                            ))
                        }));
                    });
                }
            }
            _ => {}
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
        cloud_background::start_credential_cleanup(state.cloud_storage.clone());
        let download = state.download.clone();
        app.manage(state);
        tauri::async_runtime::spawn(async move {
            // 启动恢复失败不会阻止 UI 打开；任务仍保持可解释状态，可在下载页重试。
            let result = tauri::async_runtime::spawn_blocking(move || {
                tauri::async_runtime::block_on(download.resume_startable())
            })
            .await;
            match result {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    eprintln!(
                        "[startup] download resume failed: {}",
                        error.code().as_str()
                    );
                }
                Err(error) => {
                    eprintln!("[startup] download resume worker failed: {error}");
                }
            }
        });
        Ok(())
    })
    .run(application_context())
    .expect("运行栖阅 Haven 失败");
}

#[cfg(test)]
mod command_manifest_tests {
    #[test]
    fn comic_work_catalog_commands_are_registered_once() {
        let names: Vec<&str> = super::TARGET_COMMAND_NAMES.to_vec();
        assert_eq!(
            names
                .iter()
                .filter(|name| **name == "comic_work_chapter_catalog_get")
                .count(),
            1
        );
        assert_eq!(
            names
                .iter()
                .filter(|name| **name == "comic_work_chapter_catalog_refresh")
                .count(),
            1
        );
    }
}

/// 关窗路径的结构契约（源码对接）。
///
/// 「阻止立即销毁」「等统计落地才销毁」「重复请求不销毁」这三条只有在真实的窗口与
/// 事件循环上才观察得到，而本仓库现有抽象给不出这个观察点：桌面窗口生命周期没有
/// 进程内替身，也没有现成的 mock-runtime 夹具。因此这里钉住的是**结构**——分支的
/// 形态与调用的先后；行为上的等价物在 `session_registry`（关闭事务的合并、结束时刻、
/// 占位释放）与 `commands::session`（登记用的是摘除时刻）的单测里。这不是桌面验收，
/// 两者的差距如实留在报告里。
#[cfg(test)]
mod window_close_tests {
    /// `CloseRequested` 分支的源码切片（到销毁兜底分支为止）。
    fn close_requested_branch() -> &'static str {
        include_str!("lib.rs")
            .split_once("tauri::WindowEvent::CloseRequested")
            .expect("关窗路径必须存在")
            .1
            .split_once("tauri::WindowEvent::Destroyed")
            .expect("销毁兜底路径必须存在")
            .0
    }

    /// 关窗请求自己绝不销毁窗口：销毁只能发生在统计写落地之后的收尾函数里，
    /// 否则最后一个窗口会把还在执行的统计任务一起带走。
    #[test]
    fn a_close_request_never_destroys_the_window_by_itself() {
        let branch = close_requested_branch();
        assert!(
            branch.contains("prevent_close()"),
            "必须先阻止默认销毁，否则窗口在统计写落地前就没了"
        );
        assert!(
            !branch.contains(".destroy()"),
            "销毁只能出现在 drain_window_close 里（撤销与统计之后）"
        );
    }

    /// 拿不到关闭事务（本窗口已在关闭中）时，这次请求必须直接返回：
    /// 既不销毁窗口，也不放行默认销毁。
    #[test]
    fn a_coalesced_close_request_only_returns() {
        let branch = close_requested_branch();
        let between_claim_and_spawn = branch
            .split_once("begin_window_close")
            .expect("关窗必须经过关闭事务")
            .1
            .split_once("spawn(")
            .expect("取得事务后必须交给异步收尾")
            .0;
        assert!(
            between_claim_and_spawn.contains("return;"),
            "拿不到事务时必须直接返回"
        );
        assert!(
            !between_claim_and_spawn.contains("destroy"),
            "重复的关闭请求不得销毁窗口：第一次事务的统计写还在等"
        );
    }

    /// 销毁兜底路径保持 best-effort：窗口已经被销毁，这里既不销毁第二次，也不假装
    /// 有人等它——它只能挂一个 detached 任务。这是该路径的真实上限，不是保证。
    #[test]
    fn the_destroyed_fallback_stays_best_effort() {
        let fallback = include_str!("lib.rs")
            .split_once("tauri::WindowEvent::Destroyed")
            .expect("销毁兜底路径必须存在")
            .1
            .split_once("_ => {}")
            .expect("事件匹配必须以兜底分支收尾")
            .0;
        assert!(
            fallback.contains("spawn("),
            "外部销毁没有可等待的时机，只能挂 detached 任务尽力而为"
        );
        assert!(
            !fallback.contains(".destroy()"),
            "窗口已经销毁，不得再销毁一次"
        );
    }

    /// 顺序契约：撤销流授权先于等统计写，销毁窗口在两者之后。
    #[test]
    fn the_close_tail_revokes_before_recording_and_destroys_last() {
        let source = include_str!("lib.rs");

        let tail = source
            .split_once("async fn revoke_then_record")
            .expect("关闭尾部必须存在")
            .1;
        let revoke_at = tail.find(".revoke(").expect("关闭路径必须撤销流授权");
        let record_at = tail
            .find("record_closed_session")
            .expect("关闭路径必须登记真实闭合的会话");
        assert!(
            revoke_at < record_at,
            "撤销流授权必须先于写阅读统计：统计慢或失败不能拖住授权撤销"
        );

        let drain = source
            .split_once("async fn drain_window_close")
            .expect("关窗收尾必须存在")
            .1;
        let drained_at = drain
            .find("revoke_then_record(")
            .expect("收尾必须先撤销并等统计落地");
        let destroy_at = drain.find(".destroy()").expect("收尾必须销毁窗口");
        assert!(
            drained_at < destroy_at,
            "窗口只能在撤销与统计都落地之后销毁，否则最后一个窗口会让进程提前退出"
        );
    }
}
