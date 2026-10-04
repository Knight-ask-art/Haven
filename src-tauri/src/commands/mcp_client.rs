//! 外部 MCP 客户端一键配置的 Tauri 命令。
//!
//! 规范：[`docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md`](../../../docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md) §9.1.1。
//!
//! 授权范围（用户显式给出，代码里也是这个范围）：
//!
//! - Codex → `~/.codex/config.toml`，只动 `[mcp_servers.haven]`；
//! - Claude Code → `~/.claude.json`，只动 `mcpServers.haven`。
//!
//! 命令层只做两件事：把"这台机器现在是什么样"的事实采集齐（用户主目录、两个可能重定向
//! 配置根的环境变量、安装资源目录、平台），以及把 Application 的判定结果映射成
//! `ErrorDto`。**判定规则不在这一层**：它必须能被单测逐条钉住，而这里拿不到测试替身。
//!
//! 这里没有"写任意路径""写任意客户端"的入口：目标来自闭合枚举，路径由 Application
//! 按上面的授权范围算出来。

use std::path::PathBuf;

use tauri::{Manager, State};

use haven_application::services::mcp_client_config::{
    runtime_required_files, McpClientConfigService, McpClientConfigStatusDto,
    McpClientConfigureRequest, McpClientEnvironment,
};
use haven_application::wire::ErrorDto;

use crate::agent_broker::{AgentBrokerManager, BrokerStatus};
use crate::ipc::{run_blocking, to_error_dto};
use crate::state::AppState;

/// 安装资源目录下随包分发的运行时目录名。
///
/// 必须与 `tools/mcp/package-runtime.py` 的 `BUNDLE_DIR_NAME` 是**同一个字符串**：
/// 它决定自动配置写进客户端的路径指向哪里。`tools/mcp/package-runtime.test.py` 会直接从
/// 那个脚本里读常量并与这里的字面量比对，因此改名不会静默漂移。
pub const RUNTIME_BUNDLE_DIR: &str = "haven-mcp";

/// 采集判定所需的全部环境事实。
///
/// 这是本层**唯一**读取环境的地方：`std::env` 与用户目录只在这里出现一次，判定逻辑
/// 因此可以完全在测试里构造。
fn collect_environment<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> McpClientEnvironment {
    McpClientEnvironment {
        home: app.path().home_dir().ok(),
        codex_home: std::env::var("CODEX_HOME").ok(),
        claude_config_dir: std::env::var("CLAUDE_CONFIG_DIR").ok(),
        // 随包分发的运行时目前只有 Windows 形态；其它平台如实说明，不假装可用。
        runtime_supported: cfg!(windows),
        runtime_dir: complete_runtime_dir(
            app.path()
                .resource_dir()
                .ok()
                .map(|directory| directory.join(RUNTIME_BUNDLE_DIR)),
        ),
    }
}

/// 随包分发的运行时根目录，**只有它真的完整时才返回**。
///
/// 只判断"资源目录存在"是不够的：`runtime_ready` 决定界面是否显示"可以写入"，而写进
/// 客户端配置的命令指向的正是 [`runtime_required_files`] 的那两个文件。缺任何一个，
/// 写出去的配置都启动不了——安装包被截断（例如只带了资源目录、没带 `runtime/node.exe`）
/// 正是这里最该拦下的情形，而不是等客户端报"找不到文件"，更不该在界面上显示"已就绪"。
///
/// 文件清单来自 Application 层的 [`runtime_required_files`]：那一处同时决定要写进用户
/// 配置的是什么路径，因此两处不会各写一份字面量。
fn complete_runtime_dir(directory: Option<PathBuf>) -> Option<PathBuf> {
    let directory = directory?;
    runtime_required_files(&directory)
        .iter()
        .all(|path| path.is_file())
        .then_some(directory)
}

/// 当前 Broker 端点。没有开启时是 `None`——那时写配置只会写出一条连不上的命令。
async fn current_endpoint(broker: &AgentBrokerManager) -> Option<String> {
    match broker.status().await {
        BrokerStatus::Listening { endpoint } => Some(endpoint),
        _ => None,
    }
}

// ---------- 命令核心（可在无 Tauri State 的情况下直接调用） ----------

/// 只读状态：两个客户端各一条。**不写入任何东西**。
pub async fn run_mcp_client_config_status(
    service: &McpClientConfigService,
    broker: &AgentBrokerManager,
    environment: McpClientEnvironment,
) -> Result<McpClientConfigStatusDto, ErrorDto> {
    let endpoint = current_endpoint(broker).await;
    service
        .status(&environment, endpoint.as_deref())
        .map_err(|error| to_error_dto(&error))
}

/// 写入被点中的那一个客户端，然后返回完整状态。
pub async fn run_mcp_client_config_apply(
    service: &McpClientConfigService,
    broker: &AgentBrokerManager,
    environment: McpClientEnvironment,
    request: McpClientConfigureRequest,
) -> Result<McpClientConfigStatusDto, ErrorDto> {
    let endpoint = current_endpoint(broker).await;
    service
        .configure(&request, &environment, endpoint.as_deref())
        .map_err(|error| to_error_dto(&error))
}

// ---------- Tauri 命令 ----------

/// `mcp_client_config_status`：读取两个客户端的 Haven 配置状态。
#[tauri::command]
pub async fn mcp_client_config_status<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
) -> Result<McpClientConfigStatusDto, ErrorDto> {
    let environment = collect_environment(&app);
    let service = state.mcp_client_config.clone();
    let broker = state.agent_broker.clone();
    run_blocking(move || async move {
        run_mcp_client_config_status(&service, &broker, environment).await
    })
    .await
}

/// `mcp_client_config_apply`：把 Haven 这一条写进被点中的客户端配置。
#[tauri::command]
pub async fn mcp_client_config_apply<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    request: McpClientConfigureRequest,
) -> Result<McpClientConfigStatusDto, ErrorDto> {
    let environment = collect_environment(&app);
    let service = state.mcp_client_config.clone();
    let broker = state.agent_broker.clone();
    run_blocking(move || async move {
        run_mcp_client_config_apply(&service, &broker, environment, request).await
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_application::services::mcp_client_config::McpClientTargetDto;

    #[test]
    fn the_runtime_bundle_directory_matches_the_packager() {
        // 这条断言与 `tools/mcp/package-runtime.test.py` 里的同名检查成对：两侧都从
        // **脚本**读常量，因此改名会同时让两边失败，而不是让自动配置悄悄指向不存在的路径。
        assert_eq!(RUNTIME_BUNDLE_DIR, "haven-mcp");
        assert!(!RUNTIME_BUNDLE_DIR.contains(['/', '\\', '<', '>']));
    }

    #[test]
    fn the_target_enum_covers_exactly_the_two_authorized_clients() {
        assert_eq!(McpClientTargetDto::ALL.len(), 2);
        assert_eq!(McpClientTargetDto::Codex.server_key(), "haven");
        assert_eq!(McpClientTargetDto::ClaudeCode.server_key(), "haven");
        assert_eq!(McpClientTargetDto::Codex.redirect_env(), "CODEX_HOME");
        assert_eq!(
            McpClientTargetDto::ClaudeCode.redirect_env(),
            "CLAUDE_CONFIG_DIR"
        );
    }

    /// `runtime_ready` 必须表示"命令与入口两个文件真的都在"，而不只是"资源目录存在"。
    ///
    /// 回归点：判定曾经只看 `resource_dir()/haven-mcp` 是不是 `Some`，于是安装包被截断
    /// （目录在、`runtime/node.exe` 不在）时界面照样显示"已就绪"，用户按下按钮得到一份
    /// 启动不了的配置。
    #[test]
    fn a_runtime_directory_counts_only_when_both_required_files_exist() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join(RUNTIME_BUNDLE_DIR);
        let [command, entry] = runtime_required_files(&root);

        // 只有资源目录：不完整（安装包被截断时就是这个样子）。
        std::fs::create_dir_all(root.join("runtime")).unwrap();
        assert_eq!(complete_runtime_dir(Some(root.clone())), None);

        // 只有可执行文件：仍然不完整。
        std::fs::write(&command, b"").unwrap();
        assert_eq!(complete_runtime_dir(Some(root.clone())), None);

        // 两个文件都在：这时才算就绪。
        std::fs::create_dir_all(root.join("dist")).unwrap();
        std::fs::write(&entry, b"").unwrap();
        assert_eq!(complete_runtime_dir(Some(root.clone())), Some(root));

        // 连资源目录都拿不到（例如 `tauri dev` 没跑组装脚本）：不假装就绪。
        assert_eq!(complete_runtime_dir(None), None);
    }
}
