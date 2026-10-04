//! 外部 MCP 客户端「一键配置」的 Application 入口。
//!
//! 规范：[`docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md`](../../../../docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md) §9.1.1。
//!
//! 这一层回答三个问题，且只回答这三个：
//!
//! 1. **该写哪个文件**（用户授权范围被钉死在两个固定路径上）；
//! 2. **能不能写**（环境变量重定向、运行时缺失、端点缺失、文件畸形一律 fail closed）；
//! 3. **现在是什么状态**（未配置 / 已配置 / 内容过期 / 畸形 / 被拦下）。
//!
//! 真正的字节写入在 Infrastructure 的 [`McpClientConfigPort`] 实现里；本层不碰文件系统，
//! 因此可以只用测试替身把"判定规则"钉住。
//!
//! ## 授权边界（用户显式授权的范围，代码里也是这个范围）
//!
//! | 客户端 | 文件 | 只动的键 |
//! | --- | --- | --- |
//! | Codex | `~/.codex/config.toml` | `[mcp_servers.haven]` |
//! | Claude Code | `~/.claude.json` | `mcpServers.haven` |
//!
//! 除这两个键之外的一切内容由 Infrastructure 做**结构化合并**后原样保留。本层
//! **不提供**"写任意路径""写任意客户端""传一段配置片段"的入口：目标来自闭合枚举，
//! 内容来自本层自己算出的启动命令。
//!
//! ## 为什么必须 fail closed 而不是"尽力而为"
//!
//! `CODEX_HOME` / `CLAUDE_CONFIG_DIR` 一旦被设置，客户端读的就不是我们算出的路径。
//! 那时"顺手写到默认位置"不会生效，用户却会看到一个绿色的"已配置"——这正是最难自查的
//! 一类失败。因此只要重定向生效就拒绝写入，并把原因如实显示出来。

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use haven_common::{AppError, ErrorKind};
use serde::{Deserialize, Serialize};

/// 被支持的客户端。闭合集合：这里没有"自定义路径"这一项，也不会有。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpClientTargetDto {
    Codex,
    ClaudeCode,
}

impl McpClientTargetDto {
    /// 全部目标，顺序固定（状态投影的顺序与它一致）。
    pub const ALL: [McpClientTargetDto; 2] = [Self::Codex, Self::ClaudeCode];

    /// 用户可见名。
    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::ClaudeCode => "Claude Code",
        }
    }

    /// 会重定向配置根的环境变量名。
    pub fn redirect_env(self) -> &'static str {
        match self {
            Self::Codex => "CODEX_HOME",
            Self::ClaudeCode => "CLAUDE_CONFIG_DIR",
        }
    }

    /// 默认（未被重定向时）配置文件相对用户主目录的位置。仅用于说明文案。
    fn default_relative_path(self) -> &'static str {
        match self {
            Self::Codex => ".codex/config.toml",
            Self::ClaudeCode => ".claude.json",
        }
    }

    /// 重定向根目录下配置文件的名字。
    fn redirected_file_name(self) -> &'static str {
        match self {
            Self::Codex => "config.toml",
            Self::ClaudeCode => ".claude.json",
        }
    }

    /// Haven 这一条在该客户端里的名字（两个客户端都用 `haven`）。
    pub fn server_key(self) -> &'static str {
        "haven"
    }
}

/// 客户端配置文件的位置判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpClientTargetLocation {
    /// 未被重定向时将要写入的绝对路径。
    pub path: PathBuf,
    /// 环境变量把客户端指到了别的地方：此时**一律拒绝写入**。
    pub redirected: bool,
}

/// 判定配置位置所依据的**环境事实**（由外壳层采集，本层只做判断）。
///
/// 之所以把它做成一个显式入参而不是在本层读 `std::env` 与用户目录：判定规则要能被单测
/// 逐条钉住，而"测试时读到开发者机器上的 `CODEX_HOME`"正是那种只在别人机器上炸的失败。
/// 它同时保证组合根之外没有任何地方能悄悄换掉写入目标。
#[derive(Debug, Clone, Default)]
pub struct McpClientEnvironment {
    /// 用户主目录。缺失即无法确定写入目标。
    pub home: Option<PathBuf>,
    /// `CODEX_HOME` 的当前值。
    pub codex_home: Option<String>,
    /// `CLAUDE_CONFIG_DIR` 的当前值。
    pub claude_config_dir: Option<String>,
    /// 当前平台是否支持随包分发的 MCP 运行时。
    ///
    /// 由外壳层按 `cfg!(windows)` 采集：**判定**（不支持就不写）仍在本层，只有"这个平台
    /// 支持不支持"这一件事实来自编译目标。默认 `false`，因此忘记采集时是 fail closed。
    pub runtime_supported: bool,
    /// 随安装包分发的 MCP 运行时根目录（安装资源目录下的 `haven-mcp`）。
    ///
    /// **契约：只有确认 [`runtime_required_files`] 列出的两个文件都真实存在时才应是 `Some`。**
    /// 存在性检查由采集它的外壳层（`src-tauri` 的命令层）完成——本层不碰文件系统。
    pub runtime_dir: Option<PathBuf>,
}

impl McpClientEnvironment {
    fn redirect_value(&self, target: McpClientTargetDto) -> Option<&str> {
        match target {
            McpClientTargetDto::Codex => self.codex_home.as_deref(),
            McpClientTargetDto::ClaudeCode => self.claude_config_dir.as_deref(),
        }
        .map(str::trim)
        .filter(|value| !value.is_empty())
    }

    /// 解析目标位置；`home` 缺失时返回 `None`（调用方按"被拦下"处理）。
    pub fn resolve(&self, target: McpClientTargetDto) -> Option<McpClientTargetLocation> {
        let home = self.home.as_ref()?;
        let default_path = home.join(target.default_relative_path());
        let Some(value) = self.redirect_value(target) else {
            return Some(McpClientTargetLocation {
                path: default_path,
                redirected: false,
            });
        };
        let redirected_path = Path::new(value).join(target.redirected_file_name());
        // 指到同一个文件不算重定向（把变量设成默认值是无害的写法）。相对路径一定是重定向：
        // 它的基准是客户端进程的工作目录，不是用户主目录，我们无从判断它落在哪里。
        let same = Path::new(value).is_absolute()
            && lexically_normalized(&redirected_path) == lexically_normalized(&default_path);
        Some(McpClientTargetLocation {
            path: if same { default_path } else { redirected_path },
            redirected: !same,
        })
    }
}

/// 纯词法归一化：不访问文件系统，因此对不存在的路径同样有效。
///
/// 刻意**不**用 `canonicalize`：它要求路径存在，还会跟随符号链接——而"目标是不是链接"
/// 正是需要被独立判定的事实，先跟随再判断就失去了证据。
fn lexically_normalized(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push("..");
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// 期望写入客户端配置的启动命令：命令与参数都是**安装后的绝对路径**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct McpServerLaunchSpecDto {
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<McpServerEnvVarDto>,
}

/// 一项环境变量。用列表而不是映射：顺序稳定，写入结果可逐字节复现。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct McpServerEnvVarDto {
    pub name: String,
    pub value: String,
}

/// 客户端配置里 Haven 这一条的当前状态（由 Infrastructure 读盘后给出）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpClientEntryState {
    /// 文件不存在，或文件里没有 Haven 这一条。
    Missing,
    /// 存在且与期望的启动命令一致。
    Current,
    /// 存在但内容不同（例如应用重装后资源路径变了）。
    Outdated,
    /// 文件存在但**无法安全编辑**（解析失败、期望的结构不是期望的类型、目标是链接…）。
    Malformed,
}

/// 一次读盘的结果。`detail` 面向用户，**不得**包含文件内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpClientConfigInspection {
    pub file_exists: bool,
    pub entry: McpClientEntryState,
    pub detail: String,
}

/// 配置文件读写端口（实现见 `haven-infrastructure`）。
///
/// 路径由本层算好后传入：端口不解析用户主目录、不读环境变量，因此它**无法**被诱导去写
/// 授权范围之外的文件。
pub trait McpClientConfigPort: Send + Sync {
    /// 读取现状：不写入任何东西。
    fn inspect(
        &self,
        target: McpClientTargetDto,
        spec: &McpServerLaunchSpecDto,
        config_path: &Path,
    ) -> Result<McpClientConfigInspection, AppError>;

    /// 结构化合并写入 Haven 这一条，保留文件里其它一切内容。
    fn apply(
        &self,
        target: McpClientTargetDto,
        spec: &McpServerLaunchSpecDto,
        config_path: &Path,
    ) -> Result<McpClientConfigInspection, AppError>;
}

/// 单个客户端的配置状态。闭合集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpClientConfigStateDto {
    /// 已经写好，且与当前要写的命令一致。
    Configured,
    /// 还没有这一条（文件可能都不存在）。
    NotConfigured,
    /// 有这一条，但内容与当前要写的命令不同（例如重装后路径变了）。
    Outdated,
    /// 文件存在却无法安全编辑：我们什么都不做，并如实说明。
    Malformed,
    /// 当前条件下**不允许写入**（运行时缺失 / 端点缺失 / 环境变量重定向 / 平台不支持）。
    Blocked,
}

/// 单个客户端的状态投影。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct McpClientTargetStatusDto {
    pub target: McpClientTargetDto,
    pub label: String,
    /// 将要写入（或已经写入）的配置文件绝对路径；无法确定时为 `null`。
    pub config_path: Option<String>,
    pub state: McpClientConfigStateDto,
    /// 现在按下按钮会不会真的写。`false` 的原因写在 `detail` 里。
    pub writable: bool,
    /// 面向用户的安全说明：只有原因，**不含**任何文件内容或凭据。
    pub detail: String,
}

/// `mcp_client_config_status` / `mcp_client_config_apply` 的响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct McpClientConfigStatusDto {
    pub schema_version: u32,
    /// 随包分发的 MCP 运行时是否就绪（可执行文件 + 入口都在安装资源目录里）。
    pub runtime_ready: bool,
    /// 运行时不可用时的原因（安全文案）；可用时为空串。
    pub runtime_detail: String,
    pub targets: Vec<McpClientTargetStatusDto>,
}

/// `mcp_client_config_apply` 请求：只有目标，没有路径、没有内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct McpClientConfigureRequest {
    pub target: McpClientTargetDto,
}

/// 一个目标这一次的完整计划：位置 + 启动命令。
struct McpTargetPlan {
    location: McpClientTargetLocation,
    spec: McpServerLaunchSpecDto,
}

/// 客户端自动配置服务。
#[derive(Clone)]
pub struct McpClientConfigService {
    port: Arc<dyn McpClientConfigPort>,
}

impl McpClientConfigService {
    pub fn new(port: Arc<dyn McpClientConfigPort>) -> Self {
        Self { port }
    }

    /// 只读状态：两个客户端各一条。**不写入任何东西**。
    pub fn status(
        &self,
        environment: &McpClientEnvironment,
        endpoint: Option<&str>,
    ) -> Result<McpClientConfigStatusDto, AppError> {
        let runtime = runtime_state(environment);
        let mut targets = Vec::with_capacity(McpClientTargetDto::ALL.len());
        for target in McpClientTargetDto::ALL {
            targets.push(self.project(target, environment, endpoint, &runtime));
        }
        Ok(McpClientConfigStatusDto {
            schema_version: 1,
            runtime_ready: runtime.ready,
            runtime_detail: runtime.detail.clone(),
            targets,
        })
    }

    /// 写入**一个**目标，然后返回完整状态（调用方因此不需要再发一次状态请求）。
    ///
    /// 被拦下、文件畸形、写入失败一律返回 `AppError`：这三种情况都**没有**完成用户请求的
    /// 那件事，返回一个"看起来成功"的状态会让用户以为配置已经生效。
    pub fn configure(
        &self,
        request: &McpClientConfigureRequest,
        environment: &McpClientEnvironment,
        endpoint: Option<&str>,
    ) -> Result<McpClientConfigStatusDto, AppError> {
        let runtime = runtime_state(environment);
        let target = request.target;
        if let Some(reason) = self.blocking_reason(target, environment, endpoint, &runtime) {
            return Err(AppError::new(
                "MCP_CLIENT_CONFIG_BLOCKED",
                ErrorKind::Unsupported,
                reason,
                false,
            ));
        }
        let Some(plan) = self.plan(target, environment, endpoint) else {
            // 判定说"可以写"却算不出计划，只可能是两条规则漂移了。宁可拒绝也不 panic：
            // 一次 panic 会把"没写"变成"应用崩了"，而用户要的只是一个结论。
            return Err(AppError::new(
                "MCP_CLIENT_CONFIG_BLOCKED",
                ErrorKind::Unsupported,
                "当前条件下无法确定要写入的启动命令。".to_owned(),
                false,
            ));
        };
        let inspection = self.port.apply(target, &plan.spec, &plan.location.path)?;
        if inspection.entry == McpClientEntryState::Malformed {
            // 端口已经拒绝写入；这里再兜一次，避免将来某个实现"写完才发现读不回来"。
            return Err(AppError::new(
                "MCP_CLIENT_CONFIG_MALFORMED",
                ErrorKind::Parse,
                inspection.detail,
                false,
            ));
        }
        self.status(environment, endpoint)
    }

    /// 为什么现在不能写。返回 `Some(原因)` 即拒绝。
    ///
    /// 这里的每一条都必须给出**原因**而不是 `None`：返回 `None` 表示"可以写"，而
    /// "算不出启动命令"绝不能被当成可以写——那会在下一步炸成一次 panic，或者更糟，
    /// 写出一条永远连不上的配置。
    fn blocking_reason(
        &self,
        target: McpClientTargetDto,
        environment: &McpClientEnvironment,
        endpoint: Option<&str>,
        runtime: &RuntimeState,
    ) -> Option<String> {
        if !runtime.ready {
            return Some(runtime.detail.clone());
        }
        if let Some(reason) = launch_spec_blocked_reason(environment, endpoint) {
            return Some(reason);
        }
        match environment.resolve(target) {
            None => Some("无法确定用户主目录，栖阅不会猜测客户端配置文件的位置。".to_owned()),
            Some(location) if location.redirected => Some(format!(
                "{} 被环境变量 {} 指向了别处，栖阅不会写到别的路径。请先清除该变量，或在客户端里手动配置。",
                target.label(),
                target.redirect_env()
            )),
            Some(_) => None,
        }
    }

    fn plan(
        &self,
        target: McpClientTargetDto,
        environment: &McpClientEnvironment,
        endpoint: Option<&str>,
    ) -> Option<McpTargetPlan> {
        Some(McpTargetPlan {
            location: environment.resolve(target)?,
            spec: launch_spec(environment, endpoint)?,
        })
    }

    fn project(
        &self,
        target: McpClientTargetDto,
        environment: &McpClientEnvironment,
        endpoint: Option<&str>,
        runtime: &RuntimeState,
    ) -> McpClientTargetStatusDto {
        let location = environment.resolve(target);
        let config_path = location
            .as_ref()
            .filter(|location| !location.redirected)
            .map(|location| location.path.to_string_lossy().into_owned());

        if let Some(reason) = self.blocking_reason(target, environment, endpoint, runtime) {
            return McpClientTargetStatusDto {
                target,
                label: target.label().to_owned(),
                config_path,
                state: McpClientConfigStateDto::Blocked,
                writable: false,
                detail: reason,
            };
        }
        let Some(plan) = self.plan(target, environment, endpoint) else {
            return McpClientTargetStatusDto {
                target,
                label: target.label().to_owned(),
                config_path,
                state: McpClientConfigStateDto::Blocked,
                writable: false,
                detail: "当前条件下无法确定要写入的启动命令。".to_owned(),
            };
        };
        match self.port.inspect(target, &plan.spec, &plan.location.path) {
            Ok(inspection) => {
                let (state, writable) = match inspection.entry {
                    McpClientEntryState::Current => (McpClientConfigStateDto::Configured, false),
                    McpClientEntryState::Missing => (McpClientConfigStateDto::NotConfigured, true),
                    McpClientEntryState::Outdated => (McpClientConfigStateDto::Outdated, true),
                    McpClientEntryState::Malformed => (McpClientConfigStateDto::Malformed, false),
                };
                McpClientTargetStatusDto {
                    target,
                    label: target.label().to_owned(),
                    config_path,
                    state,
                    writable,
                    detail: inspection.detail,
                }
            }
            // 读不出来（权限、IO）不是"没有配置"，也不能被显示成可写。
            Err(error) => McpClientTargetStatusDto {
                target,
                label: target.label().to_owned(),
                config_path,
                state: McpClientConfigStateDto::Blocked,
                writable: false,
                detail: error.user_message().to_owned(),
            },
        }
    }
}

/// 运行时是否就绪 + 面向用户的原因。
struct RuntimeState {
    ready: bool,
    detail: String,
}

/// 运行时（随包分发的 Node + 已编译 server）必须存在，否则写进去的配置启动不了。
///
/// 这里**不**做"文件在不在"的探测：本层不碰文件系统，真正的形状校验属于打包器与 CI
/// （见 `tools/mcp/package-runtime.py`）。但"资源目录存在"本身**不足以**说明运行时可用
/// ——`McpClientEnvironment::runtime_dir` 的契约是"外壳层已经确认
/// [`runtime_required_files`] 两个文件都在"。`ready` 因此表示的是"配置写得出去，而且
/// 客户端启动得起来"；漏掉那一步，界面会显示"已就绪"，用户按下去拿到的却是一份指向
/// 不存在文件的配置。
fn runtime_state(environment: &McpClientEnvironment) -> RuntimeState {
    if !environment.runtime_supported {
        return RuntimeState {
            ready: false,
            detail: "随包分发的 MCP 运行时目前只有 Windows 形态：当前平台不会写入任何客户端配置。"
                .to_owned(),
        };
    }
    match environment.runtime_dir.as_ref() {
        Some(_) => RuntimeState {
            ready: true,
            detail: String::new(),
        },
        None => RuntimeState {
            ready: false,
            detail:
                "找不到随包分发的 MCP 运行时目录，安装包可能不完整，暂时不会写入任何客户端配置。"
                    .to_owned(),
        },
    }
}

/// [`launch_spec`] 返回 `None` 时的原因。两者必须逐条对应：少了任何一条，用户看到的
/// 就是"按钮不能按，但没人告诉我为什么"。
fn launch_spec_blocked_reason(
    environment: &McpClientEnvironment,
    endpoint: Option<&str>,
) -> Option<String> {
    let Some(runtime_dir) = environment.runtime_dir.as_ref() else {
        return Some(
            "找不到随包分发的 MCP 运行时目录，安装包可能不完整，暂时不会写入任何客户端配置。"
                .to_owned(),
        );
    };
    if endpoint
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        return Some(
            "还没有可用的外部 Agent 端点：请先在上面开启「外部 Agent 接入」，栖阅不会写入一份连不上的配置。"
                .to_owned(),
        );
    }
    if runtime_required_files(runtime_dir)
        .iter()
        .any(|path| !is_writable_absolute(path))
    {
        return Some(
            "随包分发的 MCP 运行时路径不可用（不是绝对路径，或仍是打包模板占位符），栖阅不会把它写进客户端配置。"
                .to_owned(),
        );
    }
    None
}

/// 运行时根目录下**必须同时存在**的两个文件：随包分发的可执行文件与 server 入口。
///
/// 这是这套布局的**唯一**声明处：写进客户端配置的启动命令、以及"运行时到底能不能用"的
/// 判定都从这里取。两处各写一份字面量的话，任何一次重命名都会让其中一处指向不存在的
/// 路径，而另一处照旧显示"已就绪"——用户按下按钮，得到的是一份启动不了的配置。
pub fn runtime_required_files(runtime_dir: &Path) -> [PathBuf; 2] {
    [
        runtime_dir.join("runtime").join("node.exe"),
        runtime_dir.join("dist").join("index.js"),
    ]
}

/// 期望的启动命令：随包分发的真实可执行文件 + 入口脚本 + 桥接环境变量。
///
/// 命令**不是** `.cmd` 启动器：`.cmd` 不是可执行映像，客户端用 `shell: false` 创建不了它，
/// 而 `cmd.exe /D /S /C` 那一层引号由客户端的参数序列化决定、不由我们决定。两个普通
/// 绝对路径参数则由任何客户端按标准 Windows 引号规则原样送达（含空格路径）。
fn launch_spec(
    environment: &McpClientEnvironment,
    endpoint: Option<&str>,
) -> Option<McpServerLaunchSpecDto> {
    let runtime_dir = environment.runtime_dir.as_ref()?;
    let endpoint = endpoint.map(str::trim).filter(|value| !value.is_empty())?;
    let [command, entry] = runtime_required_files(runtime_dir);
    // 防御性断言：写进用户配置的必须是绝对路径，且绝不能带着打包器的占位符。
    if !is_writable_absolute(&command) || !is_writable_absolute(&entry) {
        return None;
    }
    Some(McpServerLaunchSpecDto {
        command: command.to_string_lossy().into_owned(),
        args: vec![entry.to_string_lossy().into_owned()],
        env: vec![
            McpServerEnvVarDto {
                name: "HAVEN_MCP_BRIDGE".to_owned(),
                value: "live".to_owned(),
            },
            McpServerEnvVarDto {
                name: "HAVEN_MCP_ENDPOINT".to_owned(),
                value: endpoint.to_owned(),
            },
        ],
    })
}

/// 可以写进客户端配置的路径：绝对、且不含打包器模板里的 `<...>` 占位符。
fn is_writable_absolute(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let text = path.to_string_lossy();
    !text.contains('<') && !text.contains('>')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 只记事的端口替身：不碰文件系统，判定规则因此可以被逐条钉住。
    ///
    /// `entry` 是**可变**的：真实端口写入之后，下一次 `inspect` 必须读到新内容。存成不可变
    /// 初值的话，`configure` 末尾那次状态投影会读回写入**之前**的旧值，于是"写成功 →
    /// 状态已配置"这条链路被替身自己的失真盖住，测的就不再是服务的行为。
    struct FakePort {
        entry: Mutex<McpClientEntryState>,
        applied: Mutex<Vec<(McpClientTargetDto, McpServerLaunchSpecDto, PathBuf)>>,
    }

    impl FakePort {
        fn new(entry: McpClientEntryState) -> Arc<Self> {
            Arc::new(Self {
                entry: Mutex::new(entry),
                applied: Mutex::new(Vec::new()),
            })
        }

        /// 读盘结果的唯一构造处：`inspect` 与 `apply` 都从这里取，两者的 `file_exists`
        /// 与 `entry` 因此不会各自漂移。
        fn inspection(entry: McpClientEntryState, detail: &str) -> McpClientConfigInspection {
            McpClientConfigInspection {
                file_exists: entry != McpClientEntryState::Missing,
                entry,
                detail: detail.to_owned(),
            }
        }
    }

    impl McpClientConfigPort for FakePort {
        fn inspect(
            &self,
            _target: McpClientTargetDto,
            _spec: &McpServerLaunchSpecDto,
            _config_path: &Path,
        ) -> Result<McpClientConfigInspection, AppError> {
            let entry = self.entry.lock().unwrap().clone();
            Ok(Self::inspection(entry, "替身状态"))
        }

        fn apply(
            &self,
            target: McpClientTargetDto,
            spec: &McpServerLaunchSpecDto,
            config_path: &Path,
        ) -> Result<McpClientConfigInspection, AppError> {
            self.applied
                .lock()
                .unwrap()
                .push((target, spec.clone(), config_path.to_path_buf()));
            // 写入成功即条目已就位：存下来，随后的 `inspect`（`configure` 末尾的状态投影
            // 会走到）读到的就是写后的内容，而不是写入前的旧值。
            *self.entry.lock().unwrap() = McpClientEntryState::Current;
            let entry = self.entry.lock().unwrap().clone();
            Ok(Self::inspection(entry, "替身写入"))
        }
    }

    /// 平台无关的绝对路径：判定规则要在所有 CI 平台上给出同一个结论，因此测试不写死
    /// Windows 形态的字符串（`C:\…` 在 Unix 上不是绝对路径，会让这些用例在 Linux 上静默
    /// 变成另一件事）。
    fn home() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\Users\tester")
        } else {
            PathBuf::from("/home/tester")
        }
    }

    fn runtime_root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\Program Files\Haven\resources\haven-mcp")
        } else {
            PathBuf::from("/opt/haven/resources/haven-mcp")
        }
    }

    fn file_name_of(path: &str) -> Option<String> {
        Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
    }

    fn parent_name_of(path: &str) -> Option<String> {
        Path::new(path)
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .map(str::to_owned)
    }

    fn test_environment() -> McpClientEnvironment {
        McpClientEnvironment {
            home: Some(home()),
            codex_home: None,
            claude_config_dir: None,
            runtime_supported: true,
            runtime_dir: Some(runtime_root()),
        }
    }

    const ENDPOINT: &str = r"\\.\pipe\haven-agent-v1-abc";

    fn target_status<'a>(
        status: &'a McpClientConfigStatusDto,
        target: McpClientTargetDto,
    ) -> &'a McpClientTargetStatusDto {
        status
            .targets
            .iter()
            .find(|entry| entry.target == target)
            .expect("每个目标都必须有一条状态")
    }

    #[test]
    fn default_paths_are_the_two_authorized_files() {
        let environment = test_environment();
        let codex = environment.resolve(McpClientTargetDto::Codex).unwrap();
        assert_eq!(codex.path, home().join(".codex").join("config.toml"));
        assert!(!codex.redirected);

        let claude = environment.resolve(McpClientTargetDto::ClaudeCode).unwrap();
        assert_eq!(claude.path, home().join(".claude.json"));
        assert!(!claude.redirected);
    }

    #[test]
    fn a_redirecting_environment_variable_blocks_the_write() {
        let mut environment = test_environment();
        environment.codex_home = Some(if cfg!(windows) {
            r"D:\elsewhere".to_owned()
        } else {
            "/elsewhere".to_owned()
        });
        let location = environment.resolve(McpClientTargetDto::Codex).unwrap();
        assert!(location.redirected, "CODEX_HOME 指到别处时必须判定为重定向");

        let port = FakePort::new(McpClientEntryState::Missing);
        let service = McpClientConfigService::new(port.clone());
        let status = service.status(&environment, Some(ENDPOINT)).unwrap();
        let codex = target_status(&status, McpClientTargetDto::Codex);
        assert_eq!(codex.state, McpClientConfigStateDto::Blocked);
        assert!(!codex.writable);
        assert!(codex.config_path.is_none(), "被重定向时不显示任何路径");
        assert!(codex.detail.contains("CODEX_HOME"));

        let error = service
            .configure(
                &McpClientConfigureRequest {
                    target: McpClientTargetDto::Codex,
                },
                &environment,
                Some(ENDPOINT),
            )
            .expect_err("被重定向时必须拒绝写入");
        assert_eq!(error.code().as_str(), "MCP_CLIENT_CONFIG_BLOCKED");
        assert!(
            port.applied.lock().unwrap().is_empty(),
            "拒绝路径必须零写入"
        );
    }

    #[test]
    fn a_redundant_environment_variable_is_not_a_redirect() {
        let mut environment = test_environment();
        environment.codex_home = Some(home().join(".codex").to_string_lossy().into_owned());
        let location = environment.resolve(McpClientTargetDto::Codex).unwrap();
        assert!(!location.redirected);
        assert_eq!(location.path, home().join(".codex").join("config.toml"));
    }

    #[test]
    fn a_missing_endpoint_blocks_the_write_instead_of_writing_a_dead_config() {
        let port = FakePort::new(McpClientEntryState::Missing);
        let service = McpClientConfigService::new(port.clone());
        let environment = test_environment();
        let status = service.status(&environment, None).unwrap();
        for target in McpClientTargetDto::ALL {
            let entry = target_status(&status, target);
            assert_eq!(entry.state, McpClientConfigStateDto::Blocked);
            assert!(!entry.writable);
        }
        assert!(
            service
                .configure(
                    &McpClientConfigureRequest {
                        target: McpClientTargetDto::ClaudeCode,
                    },
                    &environment,
                    None,
                )
                .is_err()
        );
        assert!(port.applied.lock().unwrap().is_empty());
    }

    #[test]
    fn an_unsupported_platform_blocks_every_target_without_probing_the_disk() {
        let port = FakePort::new(McpClientEntryState::Missing);
        let service = McpClientConfigService::new(port.clone());
        let environment = McpClientEnvironment {
            runtime_supported: false,
            ..test_environment()
        };
        let status = service.status(&environment, Some(ENDPOINT)).unwrap();
        assert!(!status.runtime_ready);
        assert_eq!(status.targets.len(), McpClientTargetDto::ALL.len());
        for target in McpClientTargetDto::ALL {
            assert_eq!(
                target_status(&status, target).state,
                McpClientConfigStateDto::Blocked
            );
        }
        assert!(
            service
                .configure(
                    &McpClientConfigureRequest {
                        target: McpClientTargetDto::Codex,
                    },
                    &environment,
                    Some(ENDPOINT),
                )
                .is_err()
        );
        assert!(port.applied.lock().unwrap().is_empty());
    }

    #[test]
    fn applying_writes_the_installed_runtime_command_and_reports_it() {
        let port = FakePort::new(McpClientEntryState::Missing);
        let service = McpClientConfigService::new(port.clone());
        let environment = test_environment();
        let status = service
            .configure(
                &McpClientConfigureRequest {
                    target: McpClientTargetDto::Codex,
                },
                &environment,
                Some(ENDPOINT),
            )
            .unwrap();

        let applied = port.applied.lock().unwrap();
        assert_eq!(applied.len(), 1, "只允许写被点中的那一个客户端");
        assert_eq!(applied[0].0, McpClientTargetDto::Codex);
        assert_eq!(applied[0].2, home().join(".codex").join("config.toml"));
        let spec = &applied[0].1;
        // 命令是**随包分发的真实可执行文件**加入口脚本两个普通参数，不是 `.cmd` 启动器。
        assert_eq!(file_name_of(&spec.command).as_deref(), Some("node.exe"));
        assert_eq!(parent_name_of(&spec.command).as_deref(), Some("runtime"));
        assert_eq!(spec.args.len(), 1);
        assert_eq!(file_name_of(&spec.args[0]).as_deref(), Some("index.js"));
        assert_eq!(parent_name_of(&spec.args[0]).as_deref(), Some("dist"));
        // 布局只有一个声明处：写进用户配置的两个路径必须**就是**
        // [`runtime_required_files`] 给出的那两个。谁只改其中一处（例如把入口挪到别处），
        // 这条断言会立刻失败，而不是让"就绪判定"与"写入内容"悄悄指向不同的文件。
        assert_eq!(
            runtime_required_files(&runtime_root()),
            [
                PathBuf::from(spec.command.clone()),
                PathBuf::from(spec.args[0].clone())
            ]
        );
        // 占位符绝不能进入用户配置。
        assert!(!spec.command.contains('<') && !spec.args[0].contains('<'));
        assert_eq!(
            spec.env,
            vec![
                McpServerEnvVarDto {
                    name: "HAVEN_MCP_BRIDGE".to_owned(),
                    value: "live".to_owned(),
                },
                McpServerEnvVarDto {
                    name: "HAVEN_MCP_ENDPOINT".to_owned(),
                    value: ENDPOINT.to_owned(),
                },
            ]
        );
        assert_eq!(
            target_status(&status, McpClientTargetDto::Codex).state,
            McpClientConfigStateDto::Configured
        );
    }

    #[test]
    fn a_malformed_file_is_never_reported_as_writable() {
        let port = FakePort::new(McpClientEntryState::Malformed);
        let service = McpClientConfigService::new(port);
        let status = service.status(&test_environment(), Some(ENDPOINT)).unwrap();
        let entry = target_status(&status, McpClientTargetDto::Codex);
        assert_eq!(entry.state, McpClientConfigStateDto::Malformed);
        assert!(!entry.writable, "畸形文件不得显示成可写");
    }

    #[test]
    fn an_outdated_entry_is_reported_as_outdated_and_remains_writable() {
        let port = FakePort::new(McpClientEntryState::Outdated);
        let service = McpClientConfigService::new(port);
        let status = service.status(&test_environment(), Some(ENDPOINT)).unwrap();
        let entry = target_status(&status, McpClientTargetDto::ClaudeCode);
        assert_eq!(entry.state, McpClientConfigStateDto::Outdated);
        assert!(entry.writable, "内容过期正是需要重写的情形");
    }

    #[test]
    fn a_current_entry_is_not_reported_as_writable() {
        let port = FakePort::new(McpClientEntryState::Current);
        let service = McpClientConfigService::new(port);
        let status = service.status(&test_environment(), Some(ENDPOINT)).unwrap();
        for target in McpClientTargetDto::ALL {
            let entry = target_status(&status, target);
            assert_eq!(entry.state, McpClientConfigStateDto::Configured);
            assert!(!entry.writable);
        }
    }

    #[test]
    fn a_missing_home_directory_blocks_every_target() {
        let port = FakePort::new(McpClientEntryState::Missing);
        let service = McpClientConfigService::new(port);
        let environment = McpClientEnvironment {
            home: None,
            ..test_environment()
        };
        let status = service.status(&environment, Some(ENDPOINT)).unwrap();
        for target in McpClientTargetDto::ALL {
            let entry = target_status(&status, target);
            assert_eq!(entry.state, McpClientConfigStateDto::Blocked);
            assert!(entry.config_path.is_none());
        }
    }

    #[test]
    fn the_launch_spec_never_references_a_repository_path_or_a_bare_node() {
        let spec = launch_spec(&test_environment(), Some(ENDPOINT))
            .expect("运行时与端点齐备时必须能算出启动命令");
        assert!(!spec.command.contains("mcp/haven-mcp"));
        assert_eq!(file_name_of(&spec.command).as_deref(), Some("node.exe"));
        // 命令是绝对路径：客户端可能在任何工作目录下启动它。
        assert!(Path::new(&spec.command).is_absolute());
    }
}
