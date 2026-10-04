//! 外部 MCP 客户端配置的结构化读写（Infrastructure 实现）。
//!
//! 规范：[`docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md`](../../../../docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md) §9.1.1。
//! 判定规则（该不该写、写到哪、写什么）在
//! [`haven_application::services::mcp_client_config`]；这里只负责**字节**。
//!
//! ## 写入契约（每条都对应一种会真实发生的破坏）
//!
//! - **结构化合并，不是整文件覆盖。** Codex 走 `toml_edit`（保留原文件的注释、缩进与
//!   键顺序），Claude Code 走 `serde_json::Value`。除了 `[mcp_servers.haven]` /
//!   `mcpServers.haven` 这一条，**其它一切内容原样保留**；连 `haven` 这一条里我们没管理的
//!   键（用户自己加的 `env` 项、`timeout` 之类）也不会被删掉。
//! - **不可读就不写。** 解析失败、期望的结构不是期望的类型、目标是符号链接/重解析点、
//!   文件过大、不是 UTF-8——一律拒绝，绝不"先备份再覆盖试试看"。用户的客户端配置里可能有
//!   我们完全不知道的字段，猜错一次的代价是他打不开自己的客户端。
//! - **写入前先留一份可恢复备份**（`<配置文件名>.haven-backup`，同目录、同样的权限收紧）。
//!   备份**先在内存里定稿，再在磁盘上分三步落地**：同目录建一个只有本进程能写的新文件 →
//!   把原文件的访问控制贴上去 → 落字节并 `sync_all`，最后才 `rename` 提升成备份名。
//!   顺序不能反：`std::fs::copy` 这类"先建文件、事后补权限"的写法会让一份含访问令牌的
//!   副本以父目录继承出来的权限短暂躺在用户目录里。暂存 + 提升还保证"要么还是上一份备份，
//!   要么就是这一份"——暂存或提升失败时旧备份原样保留，不会落到一份残缺的备份上。
//!   之后**同目录**建临时文件、`rename` 原子替换。跨目录的临时文件会让"替换"退化成
//!   "先删后写"，中途失败就是文件没了。
//! - **替换不得把权限放宽，也不得改变它的继承状态。** Unix 一侧本体与备份都设 0600；
//!   Windows 一侧没有等价的模式位，于是改为**原样保留原文件的访问控制**——DACL 与
//!   `SE_DACL_PROTECTED` 这一位一起带走，备份与替换件都在落字节**之前**贴好。只贴 DACL
//!   是不够的：一份被用户显式收窄成"不受父目录继承影响"的配置，若在替换后重新继承父目录
//!   的可继承 ACE，权限反而比原来更宽——那正是这里最不能接受的失败。贴完之后**读回**：
//!   控制位与 ACE 集合都必须与原来一致，否则失败关闭。DACL 读不出来、贴不上去、或读回
//!   对不上，一律失败关闭：一份含访问令牌的配置因为我们重写了一次而变得比原来更宽，
//!   比不写更糟。细节见下面的 `capture_preserved_acl` 与 `apply_preserved_acl`。
//! - **读后写前再核一次，比的是字节。** 替换前把文件整个重读一遍，与"我们据以算出新内容的
//!   那份字节"逐字节比较；不一致就放弃。只比长度 + 修改时间挡不住同长度同时间的改写
//!   （修改时间在不少文件系统上粒度粗到秒）。客户端（尤其 Claude Code）随时可能自己重写
//!   这份文件，不核对就会把它的写入覆盖掉。
//! - **错误消息里没有文件内容**，也没有凭据：只说明"哪一类问题"。被拒绝的文件很可能正是
//!   含 token 的那一份，把内容拼进错误文案等于把它散播到日志、UI 和崩溃报告里。
//!
//! ## 已知取舍
//!
//! JSON 一侧是**结构化重写**（`serde_json`）：所有键与值都在，但键顺序可能与本机原文件
//! 不同（`serde_json` 默认按字典序输出对象键）。仓库刻意不开 `preserve_order`——那会改变
//! 其它模块里"canonical JSON"摘要的字节，代价远大于这里的一点美观。写入前的备份保证这一步
//! 永远可回退。

use std::io::Write;
use std::path::{Path, PathBuf};

use haven_application::services::mcp_client_config::{
    McpClientConfigInspection, McpClientConfigPort, McpClientEntryState, McpClientTargetDto,
    McpServerLaunchSpecDto,
};
use haven_common::{AppError, ErrorKind};
use serde_json::{Map as JsonMap, Value as JsonValue};
use toml_edit::{Array, Document, Item};

/// 单份客户端配置的大小上限。
///
/// 这不是性能保护，而是**形状保护**：`~/.claude.json` 正常是几十 KB 量级，一份 8 MiB 的
/// "配置"更可能说明我们找错了文件（比如被环境变量指到了别的地方）。宁可不写。
const MAX_CONFIG_BYTES: u64 = 8 * 1024 * 1024;

/// 备份文件后缀。固定名字（滚动覆盖）：用户要的是"回到上一次写入之前"，
/// 而不是一份需要自己挑日期的备份历史。
const BACKUP_SUFFIX: &str = ".haven-backup";

/// 本机实现：直接读写用户配置目录里的那两份文件。
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalMcpClientConfig;

impl LocalMcpClientConfig {
    pub fn new() -> Self {
        Self
    }
}

impl McpClientConfigPort for LocalMcpClientConfig {
    fn inspect(
        &self,
        target: McpClientTargetDto,
        spec: &McpServerLaunchSpecDto,
        config_path: &Path,
    ) -> Result<McpClientConfigInspection, AppError> {
        let text = match read_config(config_path)? {
            ReadOutcome::Absent => {
                return Ok(McpClientConfigInspection {
                    file_exists: false,
                    entry: McpClientEntryState::Missing,
                    detail: format!(
                        "{} 还没有配置文件；写入时会新建，且只创建 Haven 这一条。",
                        target.label()
                    ),
                });
            }
            ReadOutcome::Malformed(reason) => {
                return Ok(McpClientConfigInspection {
                    file_exists: true,
                    entry: McpClientEntryState::Malformed,
                    detail: reason,
                });
            }
            ReadOutcome::Text(text) => text,
        };

        match read_entry(target, &text) {
            Err(reason) => Ok(McpClientConfigInspection {
                file_exists: true,
                entry: McpClientEntryState::Malformed,
                detail: reason,
            }),
            Ok(None) => Ok(McpClientConfigInspection {
                file_exists: true,
                entry: McpClientEntryState::Missing,
                detail: format!(
                    "{} 的配置文件里还没有 Haven 这一条；写入时会新增，其它配置保持不变。",
                    target.label()
                ),
            }),
            Ok(Some(entry)) if entry.matches(spec) => Ok(McpClientConfigInspection {
                file_exists: true,
                entry: McpClientEntryState::Current,
                detail: format!(
                    "{} 已经指向随包分发的运行时，与当前要写入的内容一致。",
                    target.label()
                ),
            }),
            Ok(Some(_)) => Ok(McpClientConfigInspection {
                file_exists: true,
                entry: McpClientEntryState::Outdated,
                detail: format!(
                    "{} 里已有的 Haven 配置与当前运行时不一致（例如应用更新后路径变了）；重新写入会只更新这一条。",
                    target.label()
                ),
            }),
        }
    }

    fn apply(
        &self,
        target: McpClientTargetDto,
        spec: &McpServerLaunchSpecDto,
        config_path: &Path,
    ) -> Result<McpClientConfigInspection, AppError> {
        ensure_writable_path(config_path)?;

        // 读取的结果就是"我们据以计算新内容的那份字节"，下面所有核对都以它为准：
        // 写前核对比的是**内容**，不是长度与修改时间（见 `verify_unchanged`）。
        let original: Option<String> = match read_config(config_path)? {
            ReadOutcome::Absent => None,
            ReadOutcome::Malformed(reason) => {
                return Err(AppError::new(
                    "MCP_CLIENT_CONFIG_MALFORMED",
                    ErrorKind::Parse,
                    reason,
                    false,
                ));
            }
            ReadOutcome::Text(text) => Some(text),
        };
        let text = original.as_deref().unwrap_or_default();

        let updated = match target {
            McpClientTargetDto::Codex => apply_toml_entry(text, spec)?,
            McpClientTargetDto::ClaudeCode => apply_json_entry(text, spec)?,
        };

        // 顺序是有意的：访问控制在动任何文件**之前**取。取不到就一份文件都没碰过。
        let preserved = capture_preserved_acl(config_path, original.is_some())?;
        // 备份：先暂存（建文件 → 贴原权限 → 落字节 → flush），提升留给 `write_atomic`
        // 在原子替换前一步做。中间任何一步失败，旧备份都还在原地。
        let staged_backup = match original.as_deref() {
            Some(bytes) => Some(stage_backup(
                config_path,
                bytes.as_bytes(),
                preserved.as_ref(),
            )?),
            None => None,
        };
        write_atomic(
            config_path,
            updated.as_bytes(),
            original.as_deref().map(str::as_bytes),
            preserved.as_ref(),
            staged_backup.as_ref(),
        )?;

        // 写完再读一次：只有"读得回来且是对的"才算成功。这一步也顺带证明我们没写出
        // 一份连自己都解析不了的配置。
        let inspection = self.inspect(target, spec, config_path)?;
        if inspection.entry == McpClientEntryState::Current {
            Ok(McpClientConfigInspection {
                file_exists: true,
                entry: McpClientEntryState::Current,
                detail: format!("已写入 {} 的配置，其它配置项未改动。", target.label()),
            })
        } else {
            Err(AppError::new(
                "MCP_CLIENT_CONFIG_VERIFY_FAILED",
                ErrorKind::Internal,
                "写入后无法确认客户端配置内容，请重新读取状态核对。",
                true,
            ))
        }
    }
}

// ---------- 读 ----------

enum ReadOutcome {
    Absent,
    Text(String),
    /// 拒绝编辑的原因（安全文案，不含文件内容）。
    Malformed(String),
}

fn io_failure() -> AppError {
    AppError::new(
        "MCP_CLIENT_CONFIG_IO",
        ErrorKind::Io,
        "读写客户端配置文件失败（权限或文件系统错误）。",
        false,
    )
}

fn malformed(reason: &str) -> AppError {
    AppError::new(
        "MCP_CLIENT_CONFIG_MALFORMED",
        ErrorKind::Parse,
        reason.to_owned(),
        false,
    )
}

fn read_config(path: &Path) -> Result<ReadOutcome, AppError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ReadOutcome::Absent);
        }
        Err(_) => return Err(io_failure()),
    };
    if is_link_like(&metadata) {
        return Ok(ReadOutcome::Malformed(
            "客户端配置文件是一个符号链接或重解析点，栖阅不会跟随它写入。".to_owned(),
        ));
    }
    if !metadata.is_file() {
        return Ok(ReadOutcome::Malformed(
            "客户端配置路径上不是一个普通文件，栖阅不会覆盖它。".to_owned(),
        ));
    }
    if metadata.len() > MAX_CONFIG_BYTES {
        return Ok(ReadOutcome::Malformed(
            "客户端配置文件异常大，栖阅不会改写它（请先确认这就是客户端的配置文件）。".to_owned(),
        ));
    }
    let bytes = std::fs::read(path).map_err(|_| io_failure())?;
    match String::from_utf8(bytes) {
        Ok(text) => Ok(ReadOutcome::Text(text)),
        Err(_) => Ok(ReadOutcome::Malformed(
            "客户端配置文件不是 UTF-8 文本，栖阅不会改写它。".to_owned(),
        )),
    }
}

// ---------- 结构 ----------

/// 客户端配置里 Haven 这一条的投影（只含我们管理的三个键）。
#[derive(Debug, Default, PartialEq, Eq)]
struct EntryProjection {
    command: Option<String>,
    args: Option<Vec<String>>,
    env: Vec<(String, String)>,
}

impl EntryProjection {
    /// 与期望一致：命令与参数逐字相同；我们管理的环境变量都在且值相同。
    ///
    /// **不**要求条目里没有别的键：用户自己加的键不算"不一致"，因为重写时它们会被保留，
    /// 把这种情况报成"过期"会让按钮永远停在"需要重新写入"。
    fn matches(&self, spec: &McpServerLaunchSpecDto) -> bool {
        if self.command.as_deref() != Some(spec.command.as_str()) {
            return false;
        }
        if self.args.as_deref() != Some(spec.args.as_slice()) {
            return false;
        }
        spec.env.iter().all(|expected| {
            self.env
                .iter()
                .any(|(name, value)| name == &expected.name && value == &expected.value)
        })
    }
}

/// 读一条条目：`Ok(None)` = 没有这一条；`Err(原因)` = 结构不对，拒绝编辑。
fn read_entry(target: McpClientTargetDto, text: &str) -> Result<Option<EntryProjection>, String> {
    match target {
        McpClientTargetDto::Codex => read_toml_entry(text),
        McpClientTargetDto::ClaudeCode => read_json_entry(text),
    }
}

fn read_toml_entry(text: &str) -> Result<Option<EntryProjection>, String> {
    let document: Document = text
        .parse()
        .map_err(|_| "客户端配置不是合法的 TOML，栖阅不会覆盖它。".to_owned())?;
    let Some(servers) = document.get("mcp_servers") else {
        return Ok(None);
    };
    let Some(servers) = servers.as_table_like() else {
        return Err("客户端配置里的 mcp_servers 不是表，栖阅不会覆盖它。".to_owned());
    };
    let Some(entry) = servers.get("haven") else {
        return Ok(None);
    };
    let Some(entry) = entry.as_table_like() else {
        return Err("客户端配置里的 mcp_servers.haven 不是表，栖阅不会覆盖它。".to_owned());
    };
    // 读的时候就把"写的时候会拒绝"的结构报出来：否则界面会显示"内容已过期 + 可写"，
    // 用户按下按钮才收到错误——那时我们已经准备去碰他的配置文件了。
    if let Some(env) = entry.get("env") {
        if env.as_table_like().is_none() {
            return Err("客户端配置里的 mcp_servers.haven.env 不是表，栖阅不会覆盖它。".to_owned());
        }
    }

    let args = entry.get("args").and_then(Item::as_array).map(|array| {
        array
            .iter()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect::<Vec<String>>()
    });
    let env = entry
        .get("env")
        .and_then(Item::as_table_like)
        .map(|table| {
            table
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .as_str()
                        .map(|value| (name.to_owned(), value.to_owned()))
                })
                .collect::<Vec<(String, String)>>()
        })
        .unwrap_or_default();

    Ok(Some(EntryProjection {
        command: entry
            .get("command")
            .and_then(Item::as_str)
            .map(str::to_owned),
        args,
        env,
    }))
}

fn read_json_entry(text: &str) -> Result<Option<EntryProjection>, String> {
    let root = parse_json(text)?;
    let Some(root) = root.as_object() else {
        return Err("客户端配置的顶层不是 JSON 对象，栖阅不会覆盖它。".to_owned());
    };
    let Some(servers) = root.get("mcpServers") else {
        return Ok(None);
    };
    let Some(servers) = servers.as_object() else {
        return Err("客户端配置里的 mcpServers 不是对象，栖阅不会覆盖它。".to_owned());
    };
    let Some(entry) = servers.get("haven") else {
        return Ok(None);
    };
    let Some(entry) = entry.as_object() else {
        return Err("客户端配置里的 mcpServers.haven 不是对象，栖阅不会覆盖它。".to_owned());
    };
    if let Some(env) = entry.get("env") {
        if env.as_object().is_none() {
            return Err(
                "客户端配置里的 mcpServers.haven.env 不是对象，栖阅不会覆盖它。".to_owned(),
            );
        }
    }

    let args = entry
        .get("args")
        .and_then(JsonValue::as_array)
        .map(|array| {
            array
                .iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect::<Vec<String>>()
        });
    let env = entry
        .get("env")
        .and_then(JsonValue::as_object)
        .map(|object| {
            object
                .iter()
                .filter_map(|(name, value)| {
                    value.as_str().map(|value| (name.clone(), value.to_owned()))
                })
                .collect::<Vec<(String, String)>>()
        })
        .unwrap_or_default();

    Ok(Some(EntryProjection {
        command: entry
            .get("command")
            .and_then(JsonValue::as_str)
            .map(str::to_owned),
        args,
        env,
    }))
}

/// 空文件与纯空白视作空对象：`~/.claude.json` 被清空过不算"畸形到不能写"。
fn parse_json(text: &str) -> Result<JsonValue, String> {
    if text.trim().is_empty() {
        return Ok(JsonValue::Object(JsonMap::new()));
    }
    serde_json::from_str(text).map_err(|_| "客户端配置不是合法的 JSON，栖阅不会覆盖它。".to_owned())
}

// ---------- 写 ----------

fn apply_toml_entry(text: &str, spec: &McpServerLaunchSpecDto) -> Result<String, AppError> {
    let mut document: Document = text
        .parse()
        .map_err(|_| malformed("客户端配置不是合法的 TOML，栖阅不会覆盖它。"))?;

    // 先校验结构再写：`IndexMut` 会把类型不对的值**换成**表，那正是我们要避免的静默破坏。
    if let Some(existing) = document.get("mcp_servers") {
        if existing.as_table_like().is_none() {
            return Err(malformed(
                "客户端配置里的 mcp_servers 不是表，栖阅不会覆盖它。",
            ));
        }
        if let Some(entry) = document
            .get("mcp_servers")
            .and_then(Item::as_table_like)
            .and_then(|servers| servers.get("haven"))
        {
            let Some(entry) = entry.as_table_like() else {
                return Err(malformed(
                    "客户端配置里的 mcp_servers.haven 不是表，栖阅不会覆盖它。",
                ));
            };
            if let Some(env) = entry.get("env") {
                if env.as_table_like().is_none() {
                    return Err(malformed(
                        "客户端配置里的 mcp_servers.haven.env 不是表，栖阅不会覆盖它。",
                    ));
                }
            }
        }
    }

    let entry = &mut document["mcp_servers"]["haven"];
    entry["command"] = toml_edit::value(spec.command.as_str());
    let mut args = Array::new();
    for arg in &spec.args {
        args.push(arg.as_str());
    }
    entry["args"] = toml_edit::value(args);
    // `env` 是**逐键合并**而不是整块替换：用户自己在 `env` 里加的变量（以及 `haven`
    // 表里其它键，例如 `timeout`）必须原样保留。整块替换会在用户毫不知情的情况下删掉
    // 他自己写的东西——那不是"更新 Haven 这一条"，那是数据丢失。
    let env = &mut entry["env"];
    for variable in &spec.env {
        env[variable.name.as_str()] = toml_edit::value(variable.value.as_str());
    }

    Ok(document.to_string())
}

fn apply_json_entry(text: &str, spec: &McpServerLaunchSpecDto) -> Result<String, AppError> {
    let mut document = parse_json(text).map_err(|reason| malformed(&reason))?;
    {
        let root = document
            .as_object_mut()
            .ok_or_else(|| malformed("客户端配置的顶层不是 JSON 对象，栖阅不会覆盖它。"))?;

        let servers = root
            .entry("mcpServers")
            .or_insert_with(|| JsonValue::Object(JsonMap::new()));
        let servers = servers
            .as_object_mut()
            .ok_or_else(|| malformed("客户端配置里的 mcpServers 不是对象，栖阅不会覆盖它。"))?;
        let entry = servers
            .entry("haven")
            .or_insert_with(|| JsonValue::Object(JsonMap::new()));
        let entry = entry.as_object_mut().ok_or_else(|| {
            malformed("客户端配置里的 mcpServers.haven 不是对象，栖阅不会覆盖它。")
        })?;

        entry.insert(
            "command".to_owned(),
            JsonValue::String(spec.command.clone()),
        );
        entry.insert(
            "args".to_owned(),
            JsonValue::Array(
                spec.args
                    .iter()
                    .map(|arg| JsonValue::String(arg.clone()))
                    .collect(),
            ),
        );
        // 与 TOML 一侧同理：`env` 逐键合并，用户自己加的变量不会被删掉。
        let env = entry
            .entry("env")
            .or_insert_with(|| JsonValue::Object(JsonMap::new()));
        let env = env.as_object_mut().ok_or_else(|| {
            malformed("客户端配置里的 mcpServers.haven.env 不是对象，栖阅不会覆盖它。")
        })?;
        for variable in &spec.env {
            env.insert(
                variable.name.clone(),
                JsonValue::String(variable.value.clone()),
            );
        }
    }

    let mut serialized = serde_json::to_string_pretty(&document)
        .map_err(|_| malformed("无法序列化客户端配置，栖阅没有写入任何东西。"))?;
    serialized.push('\n');
    Ok(serialized)
}

/// 目标路径本身是否可写：路径存在时不能是链接；父目录不能是链接。
fn ensure_writable_path(path: &Path) -> Result<(), AppError> {
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if is_link_like(&metadata) {
            return Err(malformed(
                "客户端配置文件是一个符号链接或重解析点，栖阅不会跟随它写入。",
            ));
        }
    }
    if let Some(parent) = path.parent() {
        if let Ok(metadata) = std::fs::symlink_metadata(parent) {
            if is_link_like(&metadata) {
                return Err(AppError::new(
                    "MCP_CLIENT_CONFIG_UNSAFE_PATH",
                    ErrorKind::Security,
                    "客户端配置目录是一个符号链接或重解析点，栖阅不会写到它指向的位置。",
                    false,
                ));
            }
        }
    }
    Ok(())
}

/// 已经写好、贴好权限、`sync_all` 过，但**还没有提升**到备份名的暂存文件。
///
/// "暂存"这一步存在的理由：备份里装的是含访问令牌的原配置，而"先建文件、事后补权限"
/// 会让这些字节以父目录继承出来的权限短暂躺在用户目录里。因此顺序固定为
/// 建文件（Unix 上直接 0600）→ 贴原访问控制 → 落字节 → flush，最后才提升。
struct StagedBackup {
    staging: PathBuf,
    destination: PathBuf,
}

fn backup_path(path: &Path) -> Result<PathBuf, AppError> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(path.with_file_name(format!("{name}{BACKUP_SUFFIX}")))
}

/// 在**同目录**里暂存一份可恢复备份。
///
/// 提升（`rename` 到 `<配置文件名>.haven-backup`）由 `write_atomic` 在原子替换前一步
/// 完成：在那之前旧备份一直原样躺在那里，因此暂存失败、或这次写入被放弃时，
/// **不会少一份可回退的副本**。提升本身也是原子的——不会出现"半份备份"。
fn stage_backup(
    path: &Path,
    original: &[u8],
    preserved: Option<&PreservedAcl>,
) -> Result<StagedBackup, AppError> {
    let destination = backup_path(path)?;
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config".to_owned());
    let staging = directory.join(format!(
        ".{name}{BACKUP_SUFFIX}.haven-{}-{}.tmp",
        std::process::id(),
        unique_suffix()
    ));

    let staged = (|| -> Result<(), AppError> {
        let mut handle = create_private_file(&staging).map_err(|_| backup_failure())?;
        // 权限**先于字节**：这份文件此刻是空的，含令牌的内容不会以更宽的权限存在过。
        if let Some(descriptor) = preserved {
            apply_preserved_acl(&staging, descriptor)?;
        }
        handle.write_all(original).map_err(|_| backup_failure())?;
        handle.sync_all().map_err(|_| backup_failure())?;
        restrict_permissions(&staging);
        Ok(())
    })();

    if let Err(failure) = staged {
        // 只清理自己的暂存文件；路径是刚构造出来的唯一名字。
        let _ = std::fs::remove_file(&staging);
        return Err(failure);
    }
    Ok(StagedBackup {
        staging,
        destination,
    })
}

fn backup_failure() -> AppError {
    AppError::new(
        "MCP_CLIENT_CONFIG_BACKUP_FAILED",
        ErrorKind::Io,
        "无法在写入前备份客户端配置，栖阅已放弃写入。",
        false,
    )
}

/// 同目录临时文件 + `rename` 原子替换。
///
/// 跨目录的临时文件会让 `rename` 退化成"复制 + 删除"，中途失败就是用户没有配置文件了。
/// `create_new` 保证我们不会覆盖另一个进程的临时文件。
///
/// `preserved`（Windows：原文件的访问控制）在**写入内容之前**贴到临时文件上：临时文件也是
/// 新建的，先贴权限再落字节，免得这份含令牌的配置以目录继承的权限短暂可读。
fn write_atomic(
    path: &Path,
    contents: &[u8],
    expected: Option<&[u8]>,
    preserved: Option<&PreservedAcl>,
    staged_backup: Option<&StagedBackup>,
) -> Result<(), AppError> {
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config".to_owned());
    let temp = directory.join(format!(
        ".{file_name}.haven-{}-{}.tmp",
        std::process::id(),
        unique_suffix()
    ));

    let result = (|| -> Result<(), AppError> {
        std::fs::create_dir_all(directory).map_err(|_| io_failure())?;
        let mut handle = create_private_file(&temp).map_err(|_| io_failure())?;
        if let Some(descriptor) = preserved {
            apply_preserved_acl(&temp, descriptor)?;
        }
        handle.write_all(contents).map_err(|_| io_failure())?;
        handle.sync_all().map_err(|_| io_failure())?;
        restrict_permissions(&temp);
        drop(handle);

        // 读后写前再核一次：客户端可能在这中间自己重写了配置。
        verify_unchanged(path, expected)?;

        // 提升备份与原子替换之间不再有任何 I/O：这两步相加就是这次写入对外可见的全部。
        if let Some(backup) = staged_backup {
            promote_backup(backup)?;
        }

        std::fs::rename(&temp, path).map_err(|_| io_failure())
    })();

    if result.is_err() {
        // 只清理自己的临时文件；路径是刚构造出来的唯一名字。备份的暂存文件同理——
        // 已提升的情况下这里删不到东西，那正是"提升是原子的"。
        let _ = std::fs::remove_file(&temp);
        if let Some(backup) = staged_backup {
            let _ = std::fs::remove_file(&backup.staging);
        }
    }
    result
}

/// 把暂存文件提升成备份名。失败时旧备份原样保留（`rename` 要么整个发生，要么整个不发生）。
fn promote_backup(staged: &StagedBackup) -> Result<(), AppError> {
    std::fs::rename(&staged.staging, &staged.destination).map_err(|_| backup_failure())
}

/// 读后写前的最后一次核对。
///
/// 比对的是**字节**而不是"长度 + 修改时间"：修改时间在不少文件系统上粒度粗到秒，
/// 同长度、同时间的改写（或另一个文件顶了同一个名字）能整片骗过那一对数字。逐字节相同
/// 才等价于"这份文件还是我们据以算出新内容的那一份"。
///
/// `expected == None` 表示"读取时还没有这个文件"，**不**表示"现在也没有"。少了这一支，
/// 客户端在我们读取之后**新建**出来的配置会被这次 rename 直接覆盖：而备份只在"读取时
/// 已经有文件"时才做，所以这一次覆盖连可回退的副本都不存在——那是数据丢失，不是
/// "配置写入"。两个方向都必须核对：原本没有的不能凭空出现，原本有的不能变样。
///
/// **残余竞态（已知，未消除）**：本函数读完到 `rename` 之间，另一个进程仍可以替换文件。
/// 平台没有提供路径级的 compare-and-swap（`MoveFileEx` / `ReplaceFileW` 都没有"比对"
/// 这一步），而客户端并不配合我们加锁；把核对窗口压到"一次读 + 一次 rename"已经是这条
/// 路径上能做到的极限。因此这里**不声称**竞态已被消除。
fn verify_unchanged(path: &Path, expected: Option<&[u8]>) -> Result<(), AppError> {
    let observed = match read_config(path)? {
        ReadOutcome::Absent => None,
        ReadOutcome::Text(text) => Some(text),
        // 读取时还好好的文件，现在读不了了（变成链接 / 不再是普通文件 / 超限 / 非 UTF-8）：
        // 一律按"变了"处理。绝不在这种状态下替换它。
        ReadOutcome::Malformed(_) => return Err(config_changed()),
    };
    let unchanged = match expected {
        Some(bytes) => observed.as_deref().map(str::as_bytes) == Some(bytes),
        None => observed.is_none(),
    };
    if !unchanged {
        return Err(config_changed());
    }
    Ok(())
}

fn config_changed() -> AppError {
    AppError::new(
        "MCP_CLIENT_CONFIG_CHANGED",
        ErrorKind::Conflict,
        "客户端配置在写入前发生了变化（或刚刚被创建），栖阅没有覆盖它；请重新读取状态后再试。",
        true,
    )
}

/// 建一个"只有本进程能写"的新文件。
///
/// Unix 上直接用 0600 打开，而不是"先按 umask 建出来、事后再 `chmod`"：后者会让含访问
/// 令牌的字节在这两步之间以更宽的权限存在过。Windows 上没有等价的模式位，权限由 DACL
/// 决定，调用方在落字节**之前**调 `apply_preserved_acl`。
fn create_private_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// 唯一后缀：进程内计数器 + 纳秒时间。够用且不引入额外依赖。
fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}-{counter:x}")
}

/// 尽量收紧权限（Unix：0600）。
///
/// 两份客户端配置都可能含有访问令牌，备份必须与本体同级保密。
///
/// Windows 一侧没有等价的模式位：那里改为在写入路径上**原样保留**原文件的访问控制，
/// 见 `capture_preserved_acl` / `apply_preserved_acl`。要守的不是"比原来更严"，
/// 而是"不比原来更宽"——收紧一份我们没构造过的 DACL 需要重建 ACL，风险更高，
/// 也不会让文件比用户自己设定的那份更安全。
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

fn is_link_like(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

// ---------- 访问控制（Windows：原样保留原有配置的访问控制，含继承状态） ----------
//
// 这一节的全部类型都是跨平台定义的（`apply` / `write_atomic` 的签名在两种平台上一致），
// 但只有 Windows 会真正构造与使用它们；Unix 一侧权限由 `restrict_permissions` 处理。

/// 一条访问控制项的规范化投影，**只用于比较**。
///
/// 保留"谁能拿到什么"：ACE 头之后的原始字节（`AccessMask` + SID；对象 ACE 还含 GUID）。
/// 直接比字节，不需要按 ACE 类型分别解析 SID，也不会漏掉任何 ACE 类型。
///
/// 刻意丢掉两样东西：
///
/// - **ACE 的排列顺序**：比较用集合。自动继承的合并会重排 DACL，那不是权限变化。
/// - **`INHERITED_ACE` 位**：同一条从父目录继承来的 ACE，在"我们贴上去的那一份"与
///   "系统重新传播出来的那一份"里这一位可能不同，那也不是权限变化。其余继承标记
///   （`CONTAINER_INHERIT` / `OBJECT_INHERIT` / `INHERIT_ONLY` …）**保留**：一条
///   "会传给子对象"的 ACE 与一条不会传的，不是同一件事。
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone)]
struct AclEntry {
    ace_type: u8,
    ace_flags: u8,
    /// ACE 头之后的原始字节（`AccessMask` + SID；对象 ACE 还含 GUID）。
    body: Vec<u8>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl AclEntry {
    /// 比较键：ACE 类型 + 继承标记 + ACE 头之后的原始字节。
    ///
    /// 显式拼出来而不是交给 `derive(Ord)`：参与比较的到底是哪几段字节，是函数体里能读到
    /// 的事实，而不是藏在派生实现里的默认行为——这条路径上"多比了一位 / 少比了一位"
    /// 正是安全与不安全的分界。
    fn key(&self) -> Vec<u8> {
        let mut key = Vec::with_capacity(2 + self.body.len());
        key.push(self.ace_type);
        key.push(self.ace_flags);
        key.extend_from_slice(&self.body);
        key
    }
}

/// `ACE_HEADER` 的固定布局（`winnt.h`）：`BYTE AceType; BYTE AceFlags; WORD AceSize;`。
#[cfg_attr(not(windows), allow(dead_code))]
const ACE_HEADER_BYTES: usize = 4;
/// `INHERITED_ACE`（`winnt.h`）：这一项是从父容器继承来的。
#[cfg_attr(not(windows), allow(dead_code))]
const ACE_FLAG_INHERITED: u8 = 0x10;
/// `SE_DACL_PROTECTED`（`winnt.h` 的 `SECURITY_DESCRIPTOR_CONTROL` 之一）：
/// 这份 DACL 不受父容器继承影响。
#[cfg_attr(not(windows), allow(dead_code))]
const SE_DACL_PROTECTED: u16 = 0x1000;
/// `SECURITY_DESCRIPTOR` 的固定布局（`winnt.h`）：
/// `BYTE Revision; BYTE Sbz1; WORD Control; DWORD Owner; DWORD Group; DWORD Sacl; DWORD Dacl;`
///
/// 直接按这个偏移读 `Control`，而不走 `GetSecurityDescriptorControl`：后者的形参类型在
/// 绑定层是一个 `WORD` typedef 的包装，是否被生成不由本仓库决定；而这段布局是 `winnt.h`
/// 里冻结的、自相对描述符的公开契约，和上面两条 `ACE_HEADER` 常量是同一类事实。
/// 长度前提由 `GetSecurityDescriptorLength` 在读取前兜住。
#[cfg_attr(not(windows), allow(dead_code))]
const SECURITY_DESCRIPTOR_CONTROL_OFFSET: usize = 2;
/// 一份合法的 `SECURITY_DESCRIPTOR` 至少这么长（上面那七个字段之和）。
#[cfg_attr(not(windows), allow(dead_code))]
const SECURITY_DESCRIPTOR_MIN_BYTES: usize = 20;

/// 一份 DACL 的状态：是否受继承保护 + 它的 ACE。
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Default)]
struct AccessState {
    dacl_protected: bool,
    /// 规范化后的全部 ACE。
    entries: Vec<AclEntry>,
    /// 其中**不是**继承来的那一部分（`INHERITED_ACE` 未置位）。写入时必须原样还在——
    /// 它们是用户自己显式设过的授权，丢了会让他自己都打不开这份配置。
    explicit: Vec<AclEntry>,
}

/// 原配置文件的访问控制。
///
/// 两件东西必须一起带走：
///
/// - **DACL**：谁能拿到什么权限；
/// - **`SE_DACL_PROTECTED`**：这份 DACL 是否"不受父容器继承影响"。
///
/// 少了第二项，一份被用户显式收窄（protected）的配置在替换后会重新继承父目录的可继承
/// ACE——权限反而比原来更宽，而那正是这里最不能接受的失败。
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone)]
struct PreservedAcl {
    /// 自相对安全描述符的字节副本。用 `u32` 为单位保存，是因为自相对描述符要求 DWORD
    /// 对齐——让这件事由类型兜住，而不是靠一句注释。内部只有偏移、没有绝对指针，
    /// 因此这份副本仍然可以直接交回系统 API。
    descriptor: Vec<u32>,
    state: AccessState,
}

/// 读一份路径上的安全描述符。
#[cfg(windows)]
enum DescriptorOutcome {
    /// 描述符里没有 DACL（空 DACL / 未设 DACL）。
    Missing,
    Present {
        descriptor: Vec<u32>,
        state: AccessState,
    },
}

/// 读出原有配置的访问控制（Windows）。
///
/// 三种结果都要说清楚：
///
/// - **读取失败** → `Err`，调用方放弃写入（fail closed）。读不到权限信息还敢动这份文件，
///   就是在赌"新文件的权限不会更松"，而这正是我们要避免的。
/// - **描述符里没有 DACL**（空 DACL / 未设 DACL）→ `None`：新文件保持父目录继承。
///   空 DACL 等价于"所有人完全控制"，继承出来的权限不可能比它更宽，所以这一支不会放宽权限。
/// - **读取时这份配置还不存在**（`existed == false`）→ `None`：没有"原来的权限"可保留，
///   新文件走 Windows 的常规继承。
#[cfg(windows)]
fn capture_preserved_acl(path: &Path, existed: bool) -> Result<Option<PreservedAcl>, AppError> {
    if !existed {
        return Ok(None);
    }
    match read_security_descriptor(path)? {
        DescriptorOutcome::Missing => Ok(None),
        DescriptorOutcome::Present { descriptor, state } => {
            Ok(Some(PreservedAcl { descriptor, state }))
        }
    }
}

/// 非 Windows：没有等价的"模式位"可读，权限由 `restrict_permissions` 处理。
#[cfg(not(windows))]
fn capture_preserved_acl(_path: &Path, _existed: bool) -> Result<Option<PreservedAcl>, AppError> {
    Ok(None)
}

/// `GetNamedSecurityInfoW` + 字节副本 + 规范化，一次做完并保证描述符被释放。
#[cfg(windows)]
fn read_security_descriptor(path: &Path) -> Result<DescriptorOutcome, AppError> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorLength, PSECURITY_DESCRIPTOR,
    };
    use windows::core::PCWSTR;

    let wide = wide_path(path)?;
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY：`wide` 是以 NUL 结尾的宽字符串且在调用期间存活；两个出参都是本函数的局部
    // 变量，系统只在成功时把 `descriptor` 指向它自己分配（`LocalAlloc`）的内存。
    let status = unsafe {
        GetNamedSecurityInfoW(
            PCWSTR::from_raw(wide.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&mut dacl),
            None,
            &mut descriptor,
        )
    };
    if status.0 != 0 || descriptor.is_invalid() {
        return Err(acl_failure());
    }

    let mut outcome = DescriptorOutcome::Missing;
    let mut failure = false;
    // SAFETY：状态为成功时 `descriptor` 是系统分配的自相对描述符（只填了 DACL）；下面只在
    // `LocalFree` 之前读它——`access_state_of` 只读描述符，字节副本按 DWORD 对齐拷进我们
    // 自己的缓冲区。自相对描述符内部只有偏移、没有绝对指针，因此字节副本仍然可以直接
    // 交回系统 API。
    unsafe {
        if !dacl.is_null() {
            let length = GetSecurityDescriptorLength(descriptor) as usize;
            if length > 0 {
                match access_state_of(descriptor) {
                    Ok(state) => {
                        let mut bytes = vec![0u32; length.div_ceil(4)];
                        std::ptr::copy_nonoverlapping(
                            descriptor.0.cast::<u8>(),
                            bytes.as_mut_ptr().cast::<u8>(),
                            length,
                        );
                        outcome = DescriptorOutcome::Present {
                            descriptor: bytes,
                            state,
                        };
                    }
                    Err(_) => failure = true,
                }
            }
        }
        // SAFETY：`descriptor` 来自 `GetNamedSecurityInfoW` 的分配，只在这里释放一次。
        LocalFree(Some(HLOCAL(descriptor.0)));
    }
    if failure {
        return Err(acl_failure());
    }
    Ok(outcome)
}

/// 读一个**路径**当前的访问控制状态。
///
/// 与 `capture_preserved_acl` 共用同一套规范化（`access_state_of`），因此生产代码贴上去
/// 的东西与测试读回来的东西是同一把尺子量出来的。
#[cfg(windows)]
fn read_access_state(path: &Path) -> Result<AccessState, AppError> {
    match read_security_descriptor(path)? {
        DescriptorOutcome::Missing => Ok(AccessState::default()),
        DescriptorOutcome::Present { state, .. } => Ok(state),
    }
}

/// 读一个自相对安全描述符里的 DACL 状态。
///
/// # Safety
///
/// `descriptor` 必须是一个有效的、自相对的 `SECURITY_DESCRIPTOR`，且在调用期间不被释放
/// 或修改。返回的 `AclEntry` 是字节副本，不借用它。
#[cfg(windows)]
unsafe fn access_state_of(
    descriptor: windows::Win32::Security::PSECURITY_DESCRIPTOR,
) -> Result<AccessState, AppError> {
    use windows::Win32::Security::{
        ACL, GetAce, GetSecurityDescriptorDacl, GetSecurityDescriptorLength,
    };
    use windows::core::BOOL;

    // SAFETY：调用方保证 `descriptor` 是有效的自相对描述符；下面只读它，且先读长度再按
    // 偏移取 `Control`。
    let control = unsafe {
        let length = GetSecurityDescriptorLength(descriptor) as usize;
        if length < SECURITY_DESCRIPTOR_MIN_BYTES {
            return Err(acl_failure());
        }
        let bytes = descriptor.0.cast::<u8>();
        if bytes.is_null() {
            return Err(acl_failure());
        }
        u16::from_le_bytes([
            *bytes.add(SECURITY_DESCRIPTOR_CONTROL_OFFSET),
            *bytes.add(SECURITY_DESCRIPTOR_CONTROL_OFFSET + 1),
        ])
    };

    let mut present = BOOL(0);
    let mut defaulted = BOOL(0);
    let mut acl: *mut ACL = std::ptr::null_mut();
    // SAFETY：调用方保证 `descriptor` 是有效的自相对描述符且在调用期间不被释放；三个出参
    // 都是本函数的局部变量，`acl` 借用 `descriptor` 内部的存储。
    let dacl_read =
        unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) };
    if dacl_read.is_err() {
        return Err(acl_failure());
    }

    let mut state = AccessState {
        dacl_protected: control & SE_DACL_PROTECTED != 0,
        entries: Vec::new(),
        explicit: Vec::new(),
    };
    if !present.as_bool() || acl.is_null() {
        // 未设 DACL：等价于"所有人完全控制"，没有可保留的约束。
        return Ok(state);
    }

    // SAFETY：`present` 为真且 `acl` 非空，说明它是 `descriptor` 内部那个 ACL 的地址；调用期间
    // `descriptor`（连同它内部的这块内存）一直存活。
    let acl = unsafe { &*acl };
    for index in 0..u32::from(acl.AceCount) {
        let mut ace: *mut std::ffi::c_void = std::ptr::null_mut();
        // SAFETY：`acl` 指向 `descriptor` 内部的 ACL，调用期间有效；`index < AceCount`，
        // 出参 `ace` 是本函数的局部变量。
        let ace_read = unsafe { GetAce(acl, index, &mut ace) };
        if ace_read.is_err() || ace.is_null() {
            return Err(acl_failure());
        }
        let header = ace.cast::<u8>();
        // SAFETY：`GetAce` 成功时 `ace` 指向这一项 ACE 的头部（非空，上面刚判过），头 4 字节
        // 就是 `ACE_HEADER`（`AceType` / `AceFlags` / `AceSize`）；只读这 4 字节，不越过这一项
        // ACE 自己的长度。
        let (ace_type, ace_flags, size) = unsafe {
            (
                *header,
                *header.add(1),
                usize::from(u16::from_le_bytes([*header.add(2), *header.add(3)])),
            )
        };
        if size < ACE_HEADER_BYTES {
            return Err(acl_failure());
        }
        // SAFETY：`size` 取自这一项 ACE 自己的 `AceSize`，且上面已确认不小于头长度，因此
        // `header` 之后的 `size - ACE_HEADER_BYTES` 字节都在这一项 ACE 内、可读；只读一次并
        // 立刻拷成 `Vec`，不借用 `descriptor`。
        let body = unsafe {
            std::slice::from_raw_parts(header.add(ACE_HEADER_BYTES), size - ACE_HEADER_BYTES)
                .to_vec()
        };
        let entry = AclEntry {
            ace_type,
            ace_flags: ace_flags & !ACE_FLAG_INHERITED,
            body,
        };
        if ace_flags & ACE_FLAG_INHERITED == 0 {
            state.explicit.push(entry.clone());
        }
        state.entries.push(entry);
    }
    Ok(state)
}

/// 把原来的访问控制贴到备份 / 替换件上（Windows）。失败即失败关闭——绝不"先写下去再说"。
///
/// 关键在 `SecurityInfo` 里的那一位。只贴 DACL 而不声明继承状态时，系统按"不受保护"处理，
/// 于是父目录的可继承 ACE 会被重新传播到这份文件上——一份被用户显式收窄过的配置就这样
/// 变得比原来更宽。`PROTECTED_DACL_SECURITY_INFORMATION` /
/// `UNPROTECTED_DACL_SECURITY_INFORMATION` 是这条语义在 API 上的落点；`SetFileSecurityW`
/// 不接受这两个位，所以这里用 `SetNamedSecurityInfoW`。
#[cfg(windows)]
fn apply_preserved_acl(path: &Path, preserved: &PreservedAcl) -> Result<(), AppError> {
    use windows::Win32::Security::Authorization::{SE_FILE_OBJECT, SetNamedSecurityInfoW};
    use windows::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        UNPROTECTED_DACL_SECURITY_INFORMATION,
    };
    use windows::core::{BOOL, PCWSTR};

    let wide = wide_path(path)?;
    let descriptor = PSECURITY_DESCRIPTOR(
        preserved
            .descriptor
            .as_ptr()
            .cast_mut()
            .cast::<std::ffi::c_void>(),
    );
    let inheritance = if preserved.state.dacl_protected {
        PROTECTED_DACL_SECURITY_INFORMATION
    } else {
        UNPROTECTED_DACL_SECURITY_INFORMATION
    };

    // SAFETY：`preserved.descriptor` 是自相对描述符的字节副本，按 DWORD 对齐且在调用期间
    // 存活；`GetSecurityDescriptorDacl` 与 `SetNamedSecurityInfoW` 都只读它，`acl` 指向
    // 其中的 ACL，随这份缓冲区一起存活到调用结束。
    let status = unsafe {
        let mut present = BOOL(0);
        let mut defaulted = BOOL(0);
        let mut acl: *mut ACL = std::ptr::null_mut();
        if GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted).is_err()
            || !present.as_bool()
            || acl.is_null()
        {
            return Err(acl_failure());
        }
        SetNamedSecurityInfoW(
            PCWSTR::from_raw(wide.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | inheritance,
            None,
            None,
            Some(acl.cast_const()),
            None,
        )
    };
    if status.0 != 0 {
        return Err(acl_failure());
    }

    // 读回：贴 DACL 是"请求"，读回才是"事实"。只贴不读回，等于把"系统有没有按我们说的做"
    // 当成了已知——而继承状态恰恰是这条路径上最容易被系统改写的一位。
    verify_preserved_acl(&preserved.state, &read_access_state(path)?)
}

/// 读回结果必须与原来的访问控制一致。
///
/// 两条判定，方向相反，合起来才是"权限没被改动"：
///
/// - **读回的每一项都必须在原来那份里存在**（`observed ⊆ captured`）：不允许出现任何
///   原来没有的授权——替换件变得比原来更宽，正是要挡住的事。用**集合**而不是多重集：
///   自动继承可能把同一条继承 ACE 再传播一次，重复项不授予任何新权限。
/// - **原来非继承的那几项必须都还在**（`captured.explicit ⊆ observed`）：用户显式设过的
///   授权不能被我们弄丢——那会让他自己都打不开这份配置。
///
/// 再加上控制位（`SE_DACL_PROTECTED`）必须与原来一致。任何一条不成立就失败关闭。
#[cfg(windows)]
fn verify_preserved_acl(captured: &AccessState, observed: &AccessState) -> Result<(), AppError> {
    use std::collections::BTreeSet;

    if observed.dacl_protected != captured.dacl_protected {
        return Err(acl_failure());
    }
    let observed_entries: BTreeSet<Vec<u8>> = observed.entries.iter().map(AclEntry::key).collect();
    let captured_entries: BTreeSet<Vec<u8>> = captured.entries.iter().map(AclEntry::key).collect();
    if !observed_entries.is_subset(&captured_entries) {
        return Err(acl_failure());
    }
    let captured_explicit: BTreeSet<Vec<u8>> =
        captured.explicit.iter().map(AclEntry::key).collect();
    if !captured_explicit.is_subset(&observed_entries) {
        return Err(acl_failure());
    }
    Ok(())
}

/// 非 Windows：权限由 `restrict_permissions` 处理，这里没有要贴回去的东西。
#[cfg(not(windows))]
fn apply_preserved_acl(_path: &Path, _preserved: &PreservedAcl) -> Result<(), AppError> {
    Ok(())
}

/// 访问控制读不出来 / 贴不上去。
///
/// 复用既有的 `MCP_CLIENT_CONFIG_IO`：对调用方来说这就是一次"权限层面的 IO 失败"，
/// 不新增错误码，也就不扩大既有契约。两条路径都必须失败关闭。
#[cfg(windows)]
fn acl_failure() -> AppError {
    AppError::new(
        "MCP_CLIENT_CONFIG_IO",
        ErrorKind::Io,
        "无法保留客户端配置原有的访问权限，栖阅已放弃写入。",
        false,
    )
}

/// 路径 → NUL 结尾的宽字符串。内嵌 NUL 直接拒绝：那会让 API 写到**另一个**路径上去。
#[cfg(windows)]
fn wide_path(path: &Path) -> Result<Vec<u16>, AppError> {
    use std::os::windows::ffi::OsStrExt;

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err(AppError::new(
            "MCP_CLIENT_CONFIG_UNSAFE_PATH",
            ErrorKind::Security,
            "客户端配置路径里有非法字符，栖阅不会写入。",
            false,
        ));
    }
    wide.push(0);
    Ok(wide)
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_application::services::mcp_client_config::McpServerEnvVarDto;

    fn spec(command: &str) -> McpServerLaunchSpecDto {
        McpServerLaunchSpecDto {
            command: command.to_owned(),
            args: vec![r"C:\Program Files\Haven\resources\haven-mcp\dist\index.js".to_owned()],
            env: vec![
                McpServerEnvVarDto {
                    name: "HAVEN_MCP_BRIDGE".to_owned(),
                    value: "live".to_owned(),
                },
                McpServerEnvVarDto {
                    name: "HAVEN_MCP_ENDPOINT".to_owned(),
                    value: r"\\.\pipe\haven-agent-v1-abc".to_owned(),
                },
            ],
        }
    }

    fn target_spec() -> McpServerLaunchSpecDto {
        spec(r"C:\Program Files\Haven\resources\haven-mcp\runtime\node.exe")
    }

    /// 一份**有其它内容**的 Codex 配置：写入用例要证明除了 Haven 那一条，别的都没动。
    const EXISTING_TOML: &str = r#"# 我的 Codex 配置
approval_policy = "on-request"

[mcp_servers.other]
command = "other-server"
args = ["--flag"]

[ui]
theme = "dark"
"#;

    /// "写入确实发生了"的证据：写回的文件必须能**读回**与 `target_spec` 一致的 Haven 条目。
    ///
    /// 这里刻意走 `read_toml_entry`——`inspect` 读一份配置时用的就是这条路径——而不是比
    /// 字符串。新插入的条目在 `toml_edit` 0.20 里**不保证**渲染成 `[mcp_servers.haven]`
    /// 表头：`IndexMut` 对缺失的键先放进 `Item::None`，紧接着赋进去的第一个值会把它变成
    /// inline table，于是这一条可能以 `mcp_servers.haven = { … }` 之类的形式写出。比字符串
    /// 会把一次正确的写入报成失败，而且它本来也证明不了"客户端读得回 Haven 这一条"——
    /// 命令、参数、受管环境变量对不对，只有解析回来才知道。
    fn assert_toml_entry_written(text: &str) {
        let entry = read_toml_entry(text)
            .expect("写回的 TOML 必须仍然可解析")
            .expect("写回后必须存在 mcp_servers.haven 这一条");
        assert!(
            entry.matches(&target_spec()),
            "写回的 haven 条目必须与目标规格一致（命令、参数、受管环境变量），实际：{entry:?}"
        );
    }

    #[test]
    fn toml_write_creates_only_the_haven_entry_and_keeps_everything_else() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, EXISTING_TOML).unwrap();

        let written = LocalMcpClientConfig::new()
            .apply(McpClientTargetDto::Codex, &target_spec(), &path)
            .expect("合法 TOML 必须可写");
        assert_eq!(written.entry, McpClientEntryState::Current);

        let text = std::fs::read_to_string(&path).unwrap();
        // 原有内容与注释逐字保留。
        assert!(text.contains("# 我的 Codex 配置"));
        assert!(text.contains("approval_policy = \"on-request\""));
        assert!(text.contains("[mcp_servers.other]"));
        assert!(text.contains("[ui]"));
        assert!(text.contains("theme = \"dark\""));
        // 只多了 Haven 这一条：解析回来核对内容，而不是比 `[mcp_servers.haven]` 这个表头
        // 字符串（见 `assert_toml_entry_written`）。
        assert_toml_entry_written(&text);
        assert!(text.contains("HAVEN_MCP_BRIDGE"));

        // 再读一次：必须是 Current，而不是每次读都"过期"。
        let inspected = LocalMcpClientConfig::new()
            .inspect(McpClientTargetDto::Codex, &target_spec(), &path)
            .unwrap();
        assert_eq!(inspected.entry, McpClientEntryState::Current);
    }

    #[test]
    fn toml_write_preserves_unknown_keys_inside_the_haven_entry() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(
            &path,
            "[mcp_servers.haven]\ncommand = \"old\"\ntimeout = 30\n\n[mcp_servers.haven.env]\nMY_KEY = \"keep-me\"\n",
        )
        .unwrap();

        let inspected = LocalMcpClientConfig::new()
            .inspect(McpClientTargetDto::Codex, &target_spec(), &path)
            .unwrap();
        assert_eq!(inspected.entry, McpClientEntryState::Outdated);

        LocalMcpClientConfig::new()
            .apply(McpClientTargetDto::Codex, &target_spec(), &path)
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("timeout = 30"),
            "用户自己加的键必须保留：{text}"
        );
        assert!(text.contains("MY_KEY"));
        assert!(text.contains("node.exe"));
    }

    #[test]
    fn toml_write_creates_the_file_and_its_directory_when_absent() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(".codex").join("config.toml");

        let inspected = LocalMcpClientConfig::new()
            .inspect(McpClientTargetDto::Codex, &target_spec(), &path)
            .unwrap();
        assert_eq!(inspected.entry, McpClientEntryState::Missing);
        assert!(!inspected.file_exists);

        LocalMcpClientConfig::new()
            .apply(McpClientTargetDto::Codex, &target_spec(), &path)
            .unwrap();
        assert!(path.is_file());
        // 没有旧文件就没有备份：备份的意义是"回到写入之前"。
        assert!(
            !directory
                .path()
                .join(".codex/config.toml.haven-backup")
                .exists()
        );
    }

    #[test]
    fn a_malformed_toml_is_refused_and_left_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let broken = "[mcp_servers\ncommand = \n";
        std::fs::write(&path, broken).unwrap();

        let inspected = LocalMcpClientConfig::new()
            .inspect(McpClientTargetDto::Codex, &target_spec(), &path)
            .unwrap();
        assert_eq!(inspected.entry, McpClientEntryState::Malformed);

        let error = LocalMcpClientConfig::new()
            .apply(McpClientTargetDto::Codex, &target_spec(), &path)
            .expect_err("畸形 TOML 必须被拒绝");
        assert_eq!(error.code().as_str(), "MCP_CLIENT_CONFIG_MALFORMED");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
    }

    #[test]
    fn a_wrong_typed_section_is_refused_instead_of_being_silently_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let original = "mcp_servers = \"not a table\"\n";
        std::fs::write(&path, original).unwrap();

        let error = LocalMcpClientConfig::new()
            .apply(McpClientTargetDto::Codex, &target_spec(), &path)
            .expect_err("mcp_servers 不是表时必须拒绝");
        assert_eq!(error.code().as_str(), "MCP_CLIENT_CONFIG_MALFORMED");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn json_write_keeps_every_other_entry_and_preserves_unknown_keys() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(".claude.json");
        std::fs::write(
            &path,
            r#"{
  "numStartups": 42,
  "mcpServers": {
    "other": { "command": "other-server", "args": [] },
    "haven": { "command": "old", "args": [], "env": { "KEEP": "1" } }
  },
  "projects": { "/tmp/x": { "allowedTools": ["Read"] } }
}
"#,
        )
        .unwrap();

        LocalMcpClientConfig::new()
            .apply(McpClientTargetDto::ClaudeCode, &target_spec(), &path)
            .unwrap();

        let value: JsonValue =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["numStartups"], 42);
        assert_eq!(value["mcpServers"]["other"]["command"], "other-server");
        assert_eq!(value["projects"]["/tmp/x"]["allowedTools"][0], "Read");
        assert_eq!(value["mcpServers"]["haven"]["env"]["KEEP"], "1");
        assert_eq!(
            value["mcpServers"]["haven"]["command"],
            target_spec().command
        );
        assert_eq!(
            value["mcpServers"]["haven"]["env"]["HAVEN_MCP_BRIDGE"],
            "live"
        );
    }

    #[test]
    fn json_missing_sections_are_created_and_a_second_write_is_stable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(".claude.json");
        std::fs::write(&path, "{}\n").unwrap();

        let client = LocalMcpClientConfig::new();
        client
            .apply(McpClientTargetDto::ClaudeCode, &target_spec(), &path)
            .unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        let inspected = client
            .inspect(McpClientTargetDto::ClaudeCode, &target_spec(), &path)
            .unwrap();
        assert_eq!(inspected.entry, McpClientEntryState::Current);

        client
            .apply(McpClientTargetDto::ClaudeCode, &target_spec(), &path)
            .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
    }

    #[test]
    fn a_backup_of_the_previous_content_is_written_before_replacing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, EXISTING_TOML).unwrap();

        LocalMcpClientConfig::new()
            .apply(McpClientTargetDto::Codex, &target_spec(), &path)
            .unwrap();

        let backup = directory.path().join("config.toml.haven-backup");
        assert!(backup.is_file(), "写入前必须留下可恢复备份");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), EXISTING_TOML);
        // 备份是"暂存 + 提升"出来的：提升之后不该再有暂存文件留在用户目录里。
        let leftovers: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "暂存文件必须被清理：{leftovers:?}");
    }

    /// 这次写入被放弃时，**上一份备份必须原样还在**。
    ///
    /// 回归点：备份曾经是"直接 `copy` 覆盖到备份名"。那样一来，覆盖发生在核对之前，
    /// 于是"客户端改了文件 → 我们放弃写入"这条路会顺手把上一份可恢复备份换成本次读到的
    /// 内容（甚至一份写了一半的备份）。暂存 + 提升之后，提升是原子的，且发生在核对之后。
    #[test]
    fn a_refused_write_leaves_the_previous_backup_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, EXISTING_TOML).unwrap();
        let backup = directory.path().join("config.toml.haven-backup");
        std::fs::write(&backup, "上一份备份\n").unwrap();

        let staged = stage_backup(&path, EXISTING_TOML.as_bytes(), None).expect("暂存备份不应失败");

        // 客户端在读取之后、替换之前自己改写了配置。
        std::fs::write(&path, format!("{EXISTING_TOML}\n# 客户端刚写的\n")).unwrap();
        let error = write_atomic(
            &path,
            b"new content\n",
            Some(EXISTING_TOML.as_bytes()),
            None,
            Some(&staged),
        )
        .expect_err("文件变化时必须放弃写入");
        assert_eq!(error.code().as_str(), "MCP_CLIENT_CONFIG_CHANGED");

        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            "上一份备份\n",
            "放弃写入不得动上一份备份"
        );
        let leftovers: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "暂存文件必须被清理：{leftovers:?}");
    }

    /// 读后写前的核对比的是**字节**，不是长度与修改时间。
    ///
    /// 回归点：核对曾经记下"读取时的长度 + 修改时间"再比对。修改时间在不少文件系统上
    /// 粒度粗到秒，于是"同长度、同秒内的改写"能整片骗过那一对数字——客户端自己重写过
    /// 的配置会被这次 rename 静默覆盖。下面两段分别钉住"内容变了"与"只有内容变了"。
    #[test]
    fn a_changed_file_between_read_and_write_is_never_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, EXISTING_TOML).unwrap();

        // 模拟"读完之后客户端自己改了这一份文件"。
        let read_back = EXISTING_TOML.as_bytes().to_vec();
        std::fs::write(
            &path,
            format!("{EXISTING_TOML}\n# 客户端刚刚自己写的一行\n"),
        )
        .unwrap();

        let error = write_atomic(
            &path,
            b"new content\n",
            Some(read_back.as_slice()),
            None,
            None,
        )
        .expect_err("文件在写入前变化时必须放弃");
        assert_eq!(error.code().as_str(), "MCP_CLIENT_CONFIG_CHANGED");
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("客户端刚刚自己写的一行")
        );
        // 临时文件必须被清理，不留垃圾在用户目录里。
        let leftovers: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "临时文件必须被清理：{leftovers:?}");
    }

    /// **等长**改写同样必须被发现。
    ///
    /// 这一条是上面那条的分辨力证据：长度一模一样，只有内容不同（模拟"另一个进程把这一行
    /// 改成了别的"）。只比长度 + 修改时间的实现会放过它，逐字节比较不会。
    #[test]
    fn an_equal_length_rewrite_between_read_and_write_is_never_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let original = "approval_policy = \"on-request\"\n";
        std::fs::write(&path, original).unwrap();
        let read_back = original.as_bytes().to_vec();

        let rewritten = "approval_policy = \"never-ever\"\n";
        assert_eq!(
            rewritten.len(),
            original.len(),
            "前提：这两份内容必须等长，否则这一条退化成上一条"
        );
        std::fs::write(&path, rewritten).unwrap();

        let error = write_atomic(
            &path,
            b"new content\n",
            Some(read_back.as_slice()),
            None,
            None,
        )
        .expect_err("等长改写同样必须被发现");
        assert_eq!(error.code().as_str(), "MCP_CLIENT_CONFIG_CHANGED");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), rewritten);
    }

    /// 反向证据：内容**逐字节没变**时必须放行——否则上面的拒绝可以靠"一律拒绝"通过。
    #[test]
    fn an_untouched_file_is_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, EXISTING_TOML).unwrap();

        write_atomic(
            &path,
            b"new content\n",
            Some(EXISTING_TOML.as_bytes()),
            None,
            None,
        )
        .expect("内容没变时必须允许替换");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new content\n");
    }

    /// 读取时文件不存在、写入前它被客户端创建出来：**不得覆盖**，更不能在没有备份的情况下
    /// 把它换掉。
    ///
    /// 回归点：核对曾经写成 `if let Some(expected) = stamp { … }`，于是"读取时没有这个文件"
    /// 这一支完全不核对——客户端在这中间新建的配置会被这次 rename 静默覆盖，而备份只在
    /// "读取时已经有文件"时才做，因此这一次覆盖连可回退的副本都没有。
    #[test]
    fn a_file_created_after_the_read_is_never_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");

        // 前提：读取时确实还没有这个文件，所以"据以计算的字节"是 None。
        assert!(!path.exists());

        // 客户端在读取之后、写入之前自己创建了它。
        std::fs::write(&path, "client wrote this first\n").unwrap();

        let error = write_atomic(&path, b"haven content\n", None, None, None)
            .expect_err("文件在读取之后被创建时必须放弃写入");
        assert_eq!(error.code().as_str(), "MCP_CLIENT_CONFIG_CHANGED");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "client wrote this first\n",
            "客户端新建的配置必须原样保留"
        );
        assert!(
            !directory.path().join("config.toml.haven-backup").exists(),
            "这一次写入没有备份，因此更不能发生"
        );
        let leftovers: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "临时文件必须被清理：{leftovers:?}");

        // 反向证据：文件确实还不存在时，同一条路径必须写得进去——否则上面的拒绝可以靠
        // "一律拒绝"通过。
        let fresh = directory.path().join("fresh.toml");
        assert!(write_atomic(&fresh, b"haven content\n", None, None, None).is_ok());
        assert_eq!(std::fs::read_to_string(&fresh).unwrap(), "haven content\n");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_config_file_is_never_followed() {
        let directory = tempfile::tempdir().unwrap();
        let real = directory.path().join("real.toml");
        std::fs::write(&real, EXISTING_TOML).unwrap();
        let link = directory.path().join("config.toml");
        if std::os::unix::fs::symlink(&real, &link).is_err() {
            return;
        }

        let inspected = LocalMcpClientConfig::new()
            .inspect(McpClientTargetDto::Codex, &target_spec(), &link)
            .unwrap();
        assert_eq!(inspected.entry, McpClientEntryState::Malformed);
        assert!(
            LocalMcpClientConfig::new()
                .apply(McpClientTargetDto::Codex, &target_spec(), &link)
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&real).unwrap(), EXISTING_TOML);
    }

    #[cfg(unix)]
    #[test]
    fn written_files_are_not_world_readable() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, EXISTING_TOML).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        LocalMcpClientConfig::new()
            .apply(McpClientTargetDto::Codex, &target_spec(), &path)
            .unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "客户端配置可能含访问令牌，写入后必须是 0600");
        let backup_mode = std::fs::metadata(directory.path().join("config.toml.haven-backup"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(backup_mode, 0o600, "备份与本体同级保密");
    }

    #[test]
    fn an_outdated_command_is_reported_as_outdated_not_current() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, EXISTING_TOML).unwrap();
        LocalMcpClientConfig::new()
            .apply(McpClientTargetDto::Codex, &target_spec(), &path)
            .unwrap();

        // 应用升级后资源路径变了。
        let moved = spec(r"C:\Haven2\resources\haven-mcp\runtime\node.exe");
        let inspected = LocalMcpClientConfig::new()
            .inspect(McpClientTargetDto::Codex, &moved, &path)
            .unwrap();
        assert_eq!(inspected.entry, McpClientEntryState::Outdated);
    }

    // ---------- Windows：替换不得放宽原有配置的访问控制，也不得改变它的继承状态 ----------
    //
    // 下面的用例只用 `tempfile` 里的临时路径与合成内容，不接触任何真实用户配置。

    /// 一份路径当前的访问控制状态。
    ///
    /// 走的是**生产代码的同一条路径**（`read_access_state` → `access_state_of`）：用例读回的
    /// 正是生产代码贴上去、并且自己也要读回核对的那份状态，而不是另写一套"看起来差不多"的
    /// 解析。少了这一点，用例可以通过而生产行为仍然是错的。
    #[cfg(windows)]
    fn acl_state_of(path: &Path) -> AccessState {
        read_access_state(path).expect("读取测试文件的访问控制不应失败")
    }

    /// ACE 集合（规范化后的比较键），用于比较。
    #[cfg(windows)]
    fn acl_entries(path: &Path) -> std::collections::BTreeSet<Vec<u8>> {
        acl_state_of(path)
            .entries
            .iter()
            .map(AclEntry::key)
            .collect()
    }

    /// 给一份临时文件换上一份**显式的** DACL，并指定它是否受父目录继承影响。
    ///
    /// 只用 AU / SY 两个身份：这份 DACL 必然不同于"同目录新建文件继承出来的那一份"
    /// （否则用例对"替换件是不是拿目录继承顶了包"毫无分辨力），而 AU 覆盖了几乎所有进程
    /// 令牌——测试进程（当前用户）仍然有权继续读写这份文件，断言不会退化成权限错误。
    #[cfg(windows)]
    fn set_acl(path: &Path, protected: bool) {
        use windows::Win32::Foundation::{HLOCAL, LocalFree};
        use windows::Win32::Security::Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1, SE_FILE_OBJECT,
            SetNamedSecurityInfoW,
        };
        use windows::Win32::Security::{
            ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl,
            PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
            UNPROTECTED_DACL_SECURITY_INFORMATION,
        };
        use windows::core::{BOOL, PCWSTR};

        // 完全控制，但只给这两个身份，且不带任何继承标记。
        const EXPLICIT: &str = "D:(A;;FA;;;AU)(A;;FA;;;SY)";
        // 这一位是 `SE_DACL_PROTECTED` 在 `SetNamedSecurityInfoW` 上的落点：protected 时
        // 父目录的可继承 ACE 不会进来，unprotected 时会被传播进来。
        let inheritance = if protected {
            PROTECTED_DACL_SECURITY_INFORMATION
        } else {
            UNPROTECTED_DACL_SECURITY_INFORMATION
        };

        let wide = wide_path(path).expect("测试路径必须能转成宽字符串");
        let mut sddl: Vec<u16> = EXPLICIT.encode_utf16().collect();
        sddl.push(0);

        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        let mut present = BOOL(0);
        let mut defaulted = BOOL(0);
        let mut acl: *mut ACL = std::ptr::null_mut();
        // SAFETY：`sddl` 是以 NUL 结尾的宽字符串；`descriptor` 由系统分配、用完后 `LocalFree`；
        // 两个 BOOL 出参与 ACL 指针都是本函数的局部变量。
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR::from_raw(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
            .expect("测试用的 SDDL 必须能解析");
            GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted)
                .expect("测试用的描述符必须带 DACL");
            assert!(present.as_bool() && !acl.is_null(), "测试描述符必须带 DACL");
            let status = SetNamedSecurityInfoW(
                PCWSTR::from_raw(wide.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | inheritance,
                None,
                None,
                Some(acl.cast_const()),
                None,
            );
            LocalFree(Some(HLOCAL(descriptor.0)));
            assert_eq!(status.0, 0, "测试无法给临时文件设置 DACL");
        }
    }

    /// 替换已有配置时，**原文件的访问控制必须被带过去**——替换件与备份都是。
    ///
    /// 回归点：`restrict_permissions` 在 Windows 上是空操作，而替换件是"同目录新建的临时
    /// 文件"，拿到的是目录继承；`rename` 之后原文件那份（可能被用户显式收窄过的）DACL 就
    /// 没了——一份含访问令牌的配置反而变得比原来更宽。
    #[cfg(windows)]
    #[test]
    fn windows_replacement_and_the_backup_keep_the_original_dacl() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, EXISTING_TOML).unwrap();
        set_acl(&path, false);

        // 同目录下"纯继承"的对照文件：先证明上面那份 DACL 确实与继承结果不同，
        // 否则这个用例证明不了任何东西。
        let control = directory.path().join("control.toml");
        std::fs::write(&control, "对照\n").unwrap();
        let private = acl_entries(&path);
        assert_ne!(
            private,
            acl_entries(&control),
            "测试前提：收窄过的 DACL 必须不同于目录继承出来的那一份"
        );
        assert!(
            !acl_state_of(&path).dacl_protected,
            "测试前提：这一份是 unprotected（会继承父目录），不是 protected"
        );

        LocalMcpClientConfig::new()
            .apply(McpClientTargetDto::Codex, &target_spec(), &path)
            .unwrap();

        // 先确认写入确实发生了：否则"权限没变"可以靠"什么都没写"通过。
        let written = std::fs::read_to_string(&path).unwrap();
        assert_toml_entry_written(&written);
        assert_eq!(
            acl_entries(&path),
            private,
            "替换后的配置必须与原文件同样保密"
        );
        assert!(
            !acl_state_of(&path).dacl_protected,
            "替换件不得被凭空贴上 protected：那会切断父目录的正常继承"
        );

        let backup = directory.path().join("config.toml.haven-backup");
        assert!(backup.is_file(), "写入前必须留下可恢复备份");
        assert_eq!(
            acl_entries(&backup),
            private,
            "备份含访问令牌，必须与本体同级保密"
        );
        assert!(!acl_state_of(&backup).dacl_protected);
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), EXISTING_TOML);
    }

    /// 一份**受继承保护**（`SE_DACL_PROTECTED`）的配置在替换后必须仍然受保护。
    ///
    /// 这才是"只贴 DACL"会漏掉的那一半：`SetFileSecurityW` 那条路径不接受 protected /
    /// unprotected 这两个位，系统于是按"不受保护"处理，父目录的可继承 ACE 会重新传播到
    /// 替换件上——用户显式收窄过的配置反而变得比原来更宽。用例同时比较**控制位**与
    /// **ACE 集合**，两者都取自生产代码要读回核对的那份状态。
    #[cfg(windows)]
    #[test]
    fn windows_a_protected_dacl_stays_protected_after_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, EXISTING_TOML).unwrap();
        set_acl(&path, true);

        let protected = acl_state_of(&path);
        assert!(
            protected.dacl_protected,
            "测试前提：用 protected 位设下去的 DACL 必须被读成受保护（否则这一位读不出来，\
             下面所有关于它的断言都没有分辨力）"
        );
        assert!(
            protected
                .entries
                .iter()
                .all(|entry| entry.ace_flags & ACE_FLAG_INHERITED == 0),
            "测试前提：受保护的 DACL 里不该有继承来的 ACE"
        );

        // 同目录下"纯继承"的对照文件：它的 ACE 集合就是"父目录会传播下来的那一份"。
        // 受保护的配置必须与它不同，否则这条用例分不清"保护住了"和"什么都没保住"。
        let control = directory.path().join("control.toml");
        std::fs::write(&control, "对照\n").unwrap();
        assert!(!acl_state_of(&control).dacl_protected);
        let protected_entries = acl_entries(&path);
        assert_ne!(
            protected_entries,
            acl_entries(&control),
            "测试前提：受保护的 DACL 必须不同于目录继承出来的那一份"
        );

        LocalMcpClientConfig::new()
            .apply(McpClientTargetDto::Codex, &target_spec(), &path)
            .unwrap();

        // 先确认写入确实发生了：否则"权限没变"可以靠"什么都没写"通过。
        assert_toml_entry_written(&std::fs::read_to_string(&path).unwrap());

        let after = acl_state_of(&path);
        assert!(
            after.dacl_protected,
            "替换件丢了 SE_DACL_PROTECTED：父目录的可继承 ACE 会重新进来，权限比原来更宽"
        );
        assert_eq!(
            after
                .entries
                .iter()
                .map(AclEntry::key)
                .collect::<std::collections::BTreeSet<_>>(),
            protected_entries,
            "替换件的访问控制项必须与原文件逐项相同"
        );

        let backup = directory.path().join("config.toml.haven-backup");
        assert!(backup.is_file(), "写入前必须留下可恢复备份");
        let backup_state = acl_state_of(&backup);
        assert!(
            backup_state.dacl_protected,
            "备份同样丢了 SE_DACL_PROTECTED"
        );
        assert_eq!(
            backup_state
                .entries
                .iter()
                .map(AclEntry::key)
                .collect::<std::collections::BTreeSet<_>>(),
            protected_entries,
            "备份含访问令牌，必须与本体同样保密"
        );
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), EXISTING_TOML);

        // 写入过程不得在用户目录里留下暂存文件。
        let leftovers: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "暂存文件必须被清理：{leftovers:?}");
    }

    /// 原本不存在的配置**不走**访问控制保留：新文件保持父目录的常规继承。
    ///
    /// 这一支需要独立证据：只测"保留"会放过一种把新建配置也贴上一份固定 DACL 的实现，
    /// 那等于把新文件的权限从"随目录继承"换成我们臆造的一份。
    #[cfg(windows)]
    #[test]
    fn windows_a_config_created_from_scratch_keeps_directory_inheritance() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(".codex").join("config.toml");

        LocalMcpClientConfig::new()
            .apply(McpClientTargetDto::Codex, &target_spec(), &path)
            .unwrap();

        // 同一个目录里再新建一个文件：它的 DACL 就是"这个目录的继承"。
        let control = directory.path().join(".codex").join("control.toml");
        std::fs::write(&control, "对照\n").unwrap();
        assert_eq!(
            acl_entries(&path),
            acl_entries(&control),
            "新建配置应当保持父目录继承，而不是被贴上一份我们构造的 DACL"
        );
        assert_eq!(
            acl_state_of(&path).dacl_protected,
            acl_state_of(&control).dacl_protected,
            "新建配置的继承状态也应当与同目录新建的普通文件一致"
        );
    }
}
