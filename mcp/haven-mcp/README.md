# haven-mcp-server

栖阅（Haven）的 MCP server。它让外部 MCP 客户端（Claude Desktop 等）**读取脱敏上下文**
或**创建待批准提案**——仅此而已。

> **一句话边界**：本 server 没有任何工具可以批准、应用、删除或写入。
> 真正的写入只能由用户在 Haven 应用界面里逐项核对 digest 后完成。

架构与安全边界以 [`docs/architecture/AI_SYSTEM.md`](../../docs/architecture/AI_SYSTEM.md) §5 为准；
本文件只讲怎么跑、怎么测、以及**当前接通到了哪一步**。

---

## 当前接通状态（重要）

**live 适配器已实现，但默认关闭，且从未在真实客户端上验收。**

| 层 | 状态 |
| --- | --- |
| MCP 协议面（9 个工具的 name / schema / outputSchema / 注解 / 错误语义） | **已实现并冻结** |
| Typed Haven Agent Bridge 端口（`src/bridge.ts`） | **已实现**（类型化的读 + 提案端口） |
| 显式不可用适配器（`UnavailableHavenAgentBridge`） | **已实现**，且是生产默认 |
| 本地传输适配器（`src/local-broker.ts` 的 `LiveHavenAgentBridge`） | **已实现**：`HAVEN_MCP_BRIDGE=live` + 合法 `HAVEN_MCP_ENDPOINT` 时连接 Rust Broker |
| Haven 后端的 9 个只读/提案用例 | **均已实现**；真实 Haven→Node→MCP 尚未验收 |

因此现在的行为分三种：

1. **默认**（未设 `HAVEN_MCP_BRIDGE`）：`haven.available = false`，其余工具返回
   `HAVEN_BRIDGE_UNAVAILABLE` 或 `HAVEN_CAPABILITY_UNAVAILABLE`；
2. **`live` 且端点形态合法**：连接本机 Haven Broker（需要用户在栖阅设置页显式开启
   「外部 Agent 接入」）。注意 `haven.available = true` **只表示端点形态通过校验**，真实连接
   状态要到调用时才确认——Broker 没开、Haven 没运行或 ACL 拒绝时，调用仍会拿到
   `HAVEN_BRIDGE_UNAVAILABLE`；
3. **`live` 但端点缺失 / 非法**：缺失时保持显式不可用，非法时返回
   `HAVEN_MCP_ENDPOINT_INVALID` 且**不尝试连接**（不猜、不扫描、不回退）。

**它不会返回任何编造的数据。** 9 个工具现在都有 Haven 后端路径；但默认桥接关闭、端点
缺失或真实链路不可达时，工具仍会如实返回 `HAVEN_BRIDGE_UNAVAILABLE`。能力被服务端明确
关闭时才会返回 `HAVEN_CAPABILITY_UNAVAILABLE`。

**未验收边界**：Codex / Claude Code / DSH / Pi 四个真实客户端、Windows WebView2 上的栖阅
设置界面、以及 Haven→Node→MCP 的真实端到端都**没有实测过**——本包的连接用例用的是进程内
假 transport，不是真的 Rust Broker。**单元与集成测试通过不等于生产实测。**

### 9 个工具与本版本实现状态

| 工具 | Haven 侧实现 |
| --- | --- |
| `get_system_capabilities` | ✅ 已实现（本地 + 桥接状态） |
| `get_settings_snapshot` | ✅ 已实现（`settings_read`） |
| `propose_settings_patch` | ✅ 已实现（`settings_proposal`） |
| `get_setting_sources` | ✅ 已接入 `setting_sources_read` |
| `get_resource_preference_snapshot` | ✅ 已接入 `resource_preference_read` |
| `get_library_summary` | ✅ 已接入 `library_summary_read` |
| `get_media_capabilities` | ✅ 已接入 `media_capabilities_read` |
| `get_onboarding_state` | ✅ 已接入 `onboarding_read` |
| `propose_resource_preference_patch` | ✅ 已接入 `resource_preference_proposal` |

工具清单是**冻结集合**（`src/constants.ts` 的 `TOOL_NAMES`）。新增工具必须先改架构文档；
`test/tools.test.ts` 断言注册结果与它完全相等，因此"顺手多加一个 write 工具"会直接失败。

资源级 `propose_resource_preference_patch` 只携带 Agent 请求修改的部分 patch，不会把当前
authoritative 偏好合并后再复制进外部响应。用户在 Haven UI 批准时，Rust Application service
会在同一 CAS 事务内重读并合并；Agent Receipt（若由 UI/内部服务读取）也是脱敏审计投影，
不暴露已有的字体路径、endpoint、Bearer 或凭据样式文本。Haven 内部的 authoritative 设置
仍保存真实值。MCP server 本身没有 Receipt 读取或 Apply 工具。

---

## `live` 桥接：已实现，默认关闭

MCP 客户端会把本 server 当子进程启动，而它要访问的是**另一个进程**（运行中的 Haven）。
这条本地通道由 `src/local-broker.ts` 实现，边界如下：

- 只做 **outbound `node:net`** 连接，目标是 Windows Named Pipe 或 Unix socket：不监听任何
  端口、不发 DNS、不做任意网络访问。`test/source-guards.test.ts` 断言源码里只 import
  `node:net`，且不含 `createServer(` / `.listen(`；其余网络与进程模块
  （`node:http` / `node:https` / `node:dgram` / `node:tls` / `node:child_process`）
  连同 fs / SQLite / 凭据库 / Tauri 都在 `forbiddenModules` 里整条禁止；
- **不读**文件系统、**不读** SQLite、**不读** secret、**不** invoke Tauri；
- 每次工具调用建立一条短连接：connect → hello/welcome → 一个请求 → close；
- 端点缺失 → 显式不可用；端点形态非法 → `HAVEN_MCP_ENDPOINT_INVALID` 且不尝试连接。

| `HAVEN_MCP_BRIDGE` | 行为 |
| --- | --- |
| 未设 / `unavailable` / `none` | 显式不可用桥接（**默认**） |
| `live` | 有合法端点 → 连接 Broker；无端点 → 显式不可用；端点非法 → `HAVEN_MCP_ENDPOINT_INVALID` |
| `fixture` | **拒绝启动**（见下） |
| 其它取值 | **拒绝启动** |

### 测试替身不是生产降级路径

`fixture` 模式需要**同时**满足两个条件：`HAVEN_MCP_ALLOW_TEST_BRIDGE=1` **且**调用方
注入了 `fixtureFactory`。生产入口 `src/index.ts` 不传 `fixtureFactory`，因此环境变量
本身永远不足以在生产进程里打开假桥接——那段代码根本不在生产的模块图里
（`test/source-guards.test.ts` 会扫描确认）。

宁可显式不可用，也不要一个"看起来很真"的假数据源。

---

## 使用

先分清两条入口：它们的 Node 前置条件**完全不同**，别把其中一条的要求安到另一条头上。

| 入口 | 配置里的命令 | Node 从哪来 |
| --- | --- | --- |
| **开发 / 自建**（本节的例子、下面手敲的客户端 CLI） | 你自己 clone 出来的 `mcp/haven-mcp/dist/index.js` | **本机自装**：`package.json` 与 `package-lock.json` 的 `engines` 都是 `node >= 22.12.0` |
| **装好的栖阅**（设置页「一键配置」实际写进客户端的那一条） | 安装目录里的 `<resource_dir>/haven-mcp/runtime/node.exe` + `dist/index.js` | **随安装包附带**，不要求用户装 Node、也不需要 clone；只有 Windows 安装包形态（见下文「可分发运行时」） |

下面这段是**开发 / 自建**路径。已经用安装包装好栖阅的用户不需要执行它——那是构建机 / CI 的事。

```bash
npm install                 # engines: node >= 22.12.0
npm run build
node dist/index.js          # stdio transport
```

自建产物的 MCP 客户端配置（不填 `env` 也能启动，但那样所有工具都会如实报告桥接不可用）：

```json
{
  "mcpServers": {
    "haven": {
      "command": "node",
      "args": ["<repo>/mcp/haven-mcp/dist/index.js"],
      "env": {
        "HAVEN_MCP_BRIDGE": "live",
        "HAVEN_MCP_ENDPOINT": "\\\\.\\pipe\\haven-agent-v1-<8 位小写十六进制>"
      }
    }
  }
}
```

端点字符串请**从栖阅设置页「外部 Agent 接入」分组复制**（只有真实监听时才给出可复制地址，
且需要你先在那里显式开启）；不要手写、不要猜。上面这份 JSON 就是栖阅界面为四个客户端
（Codex / Claude Code / DSH / Pi）生成的同一份模板，四者内容完全相同；模板里同一个占位符
写作 `<path-to-haven>`，`<repo>` 与它指的是同一件事——**你自己 clone 本仓库的路径**，
只在自建路径下才需要替换。注意它与设置页「一键配置」写进客户端的那条命令**不是同一条**
（那边是随包分发的 `runtime/node.exe`），这是有意的。

### 配套的行为协议技能包

光有连接还不够：模型还需要知道**怎么用**这 9 个工具。这项行为协议是
`skills/haven-agent-proposal`（[`AI_SYSTEM.md`](../../docs/architecture/AI_SYSTEM.md) §6.1）。
把它交给外部客户端的内容由仓库里的打包器产出：

```bash
python tools/skills/pack-skill.py --out <产物目录>
# → haven-agent-proposal-<版本>.zip
# → haven-agent-proposal-<版本>.manifest.json
```

包内文件与 `skills/haven-agent-proposal/` **逐字节相同**；manifest 里的工具清单从
`src/constants.ts` 现读，等于冻结的 9 项。栖阅应用自己用的是同一份内容（编译期嵌入二进制），
见 AI_SYSTEM.md §6.2/§6.3。

### 接到 Codex / Claude Code 上（**未实测**，按客户端文档核对）

这一段是**自建路径**：用你自己构建的产物、自己执行客户端 CLI。两个客户端**都没有实测过**
——下面是按它们各自文档整理的最小步骤，权威说明以客户端自己的文档为准（版本差异都发生在
那一层）。**Haven 不会替你执行这些命令**，也不会在你不知情时改写任何客户端配置；唯一会写
客户端配置的能力是设置页里的「一键配置」，由你主动点击触发，写的是随包分发的运行时
（见下文「一键配置 Codex / Claude Code」）。已经用安装包装好栖阅的用户可以直接走那条路，
不需要做本节任何一步。

连接。两个客户端都有自己的 CLI，可在添加 stdio MCP server 时一并传入桥接环境变量：

```powershell
# 自建路径：先在仓库的 mcp/haven-mcp 目录运行 npm install 与 npm run build
# （本机需要 Node ≥ 22.12.0；随安装包分发的那份运行时不需要）。
# 把 <复制的端点> 替换成 Haven 设置页「外部 Agent 接入」提供的实际端点，<repo> 替换成仓库路径。
claude mcp add haven -s user -e HAVEN_MCP_BRIDGE=live -e "HAVEN_MCP_ENDPOINT=<复制的端点>" -- node "<repo>\mcp\haven-mcp\dist\index.js"
codex mcp add haven --env HAVEN_MCP_BRIDGE=live --env "HAVEN_MCP_ENDPOINT=<复制的端点>" -- node "<repo>\mcp\haven-mcp\dist\index.js"
```

这两条命令会分别写入你所选客户端的 MCP 配置；Haven 本身不会执行它们。环境变量只有
`HAVEN_MCP_BRIDGE=live` 与 `HAVEN_MCP_ENDPOINT`。端点必须从 Haven 设置页复制；不要手写或猜测。
未启用 Haven Broker 或 Haven 未运行时，工具会如实报告桥接不可用。`<repo>` 是你自己克隆本仓库
的路径，且需要先在 `mcp/haven-mcp` 目录运行 `npm install` 与 `npm run build` 生成 `dist/`。

不想用 CLI 的话，上面那段 `mcpServers` JSON 就是等价写法：Claude Code 放在项目级
`.mcp.json`，Codex 放在它自己的配置文件里。各客户端字段名可能不同，`command` / `args` /
`env` 三项语义相同——差异属于各客户端的文档，不属于本仓库的契约。

技能包。这一步同样是**客户端自己的机制**——栖阅不会把技能包装进任何客户端，也不写任何
Skill 目录。它与上文那条由你主动点击触发的「一键配置」是**两件互不相干的事**：那条只写
授权范围内的那一条 MCP 连接配置，不涉及技能包。下面分别解压到各自支持的 Skill 目录：

- **Claude Code**：项目级目录为 `.claude/skills/`，个人级目录为 `~/.claude/skills/`。
- **Codex**：项目级目录为 `.agents/skills/`，个人级目录为 `~/.agents/skills/`。

压缩包内所有文件都以 `haven-agent-proposal/` 为路径前缀（不写入空目录条目）；把 zip 解压到对应的 `skills/` 父目录即可，最终应有
`<skills目录>/haven-agent-proposal/SKILL.md`。Codex 项目级 `.agents/` 可被 Git 忽略；安装 Skill
不需要把正文复制进 `AGENTS.md`。

`SKILL.md` 里用 `<!-- haven:audience=native -->` / `<!-- haven:audience=external -->` 标出了
两条运行时各自的段落。外部客户端拿到的是**整份文件**：属于栖阅自己那条路径的段落
（"你没有工具，只能输出 JSON"）说的是原生运行时，不约束你。

**仍未验收**：上面两条路径都没有在真实客户端上跑过，"把技能包放进技能目录之后模型确实遵守
它"也没有实测记录。

### 距「开箱即用」还差什么（**当前不是开箱即用**）

逐条列清楚哪些只是**自建路径**的要求、哪些对装好的栖阅同样成立，避免把它读成
"装好栖阅就能连上"：

1. **自建产物要自己构建。** `dist/` 不随版本库分发（根 `.gitignore:3` 的 `**/dist/`），
   自建路径必须先在 `mcp/haven-mcp` 里 `npm install && npm run build`，而这条路径要求本机
   Node ≥ 22.12.0（`package.json` / `package-lock.json` 的 `engines`；开发门禁里的
   `vitest` 还要更挑版本）。**装好的栖阅不走这条路径**：安装包自带已编译产物与一份 Node
   运行时（见「可分发运行时」），用户既不需要 clone 也不需要装 Node。
2. **包没有发布到 registry。** `package.json` 是 `"private": true`，不在任何 npm registry 上，
   因此**自建路径**下客户端配置里的入口只能是你克隆下来的仓库路径。构建产物本身是一个标准
   可执行入口（`bin` 里声明了 `haven-mcp-server`，`src/index.ts:1` 有 shebang），所以你也可以
   用本地 link 把它变成一条命令；但**这条本地安装路径本仓库没有实测过**，不作为推荐步骤。
   **自动配置也不走这条路径**：它写的是安装目录里随包分发的 `runtime/node.exe` + `dist/index.js`
   （见「一键配置」），所以"入口必须是仓库路径"只对自建路径成立。
3. **端点不能猜。** `HAVEN_MCP_ENDPOINT` 没有默认值，也不存在任何发现文件；必须由用户在
   栖阅设置页显式开启 Broker 后**复制**出来（§4.2 / §4.5）。
4. **没有真实客户端实测。** Codex / Claude Code / DSH / Pi 四者都是 `verified: false`，
   §9 的兼容矩阵是设计目标，不是实测结果。

剩下要解决的只有第 4 条：它需要真实客户端环境，本仓库无法自行提供。第 2 条里的**分发决策
已经做完**——产物随 Windows 安装包分发，一键配置直接写那条路径，不再依赖 registry 发布。
但"随包分发 + 一键配置"这条链路**没有在真实安装包与真实客户端上跑过一次往返**，因此本文件与
[`MCP_EXTERNAL_AGENT_TRANSPORT.md`](../../docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md)
继续是 partial 状态，不得对外描述为开箱即用、也不得描述为"已接通 MCP"。

### 可分发运行时（打包器已实现，并已随安装包分发）

上面第 1 条的"产物要自己构建（构建机还得自己装 Node）"已经有了一条**不需要这两件事**的产物：
仓库里的打包器把 server 产物、生产依赖与一份 Node 运行时组装成一个自包含目录。

```text
pwsh tools/mcp/assemble-runtime.ps1 -Out src-tauri/resources
  -> src-tauri/resources/haven-mcp/        （也就是安装后的 <resource_dir>/haven-mcp）
       runtime/node.exe           随包分发的 Node（不再要求用户自己装）
       dist/**.js                 已编译的 MCP server
       node_modules/**            生产依赖（npm prune --omit=dev 之后）
       package.json
       haven-mcp-launcher.cmd     人工排查 / CI 用的启动器（**不是**自动配置写的命令）
  -> src-tauri/resources/haven-mcp-runtime-<版本>.manifest.json
```

这个目录是 `src-tauri/tauri.conf.json` 的 `bundle.resources` 映射源，因此 **MSI 与 NSIS
都会带上它**；`tauri build` 之前必须跑过组装（脚本内部会 `npm ci && npm run build &&
npm prune --omit=dev`，再调用 `tools/mcp/package-runtime.py`）。那条 `npm ci` 正是
`node >= 22.12.0` 的落点：这个要求属于**构建机 / CI**，不属于用户机——用户机上跑的是产物里
那份自带的 `node.exe`，他不需要知道 Node 是什么。

产物的形状由 CI 的 `mcp-launcher` 作业（Windows）真实构建并逐条断言：清单声明的每份文件
都存在、`bundle.resources` 的映射名与产物目录同名、客户端命令是 `runtime/node.exe` 加
`dist/index.js`（不是 `.cmd` 启动器、不是裸 `node`、不含模板占位符残留）、环境变量恰好是
`HAVEN_MCP_BRIDGE` 与 `HAVEN_MCP_ENDPOINT`、随包的 `node.exe --version` 等于
`.node-version`；最后把产物复制到一段**含空格的安装路径**（模拟 `C:\Program Files\…`）上，
用清单里那条**确切的**命令与参数跑一次 JSON-RPC `initialize` 往返。

启动器只用 `%~dp0`（自身所在目录）定位运行时与入口，因此整个 `haven-mcp/` 目录可以搬到任何
位置；`tools/mcp/package-runtime.test.py` 对这条路径形状、产物内容与安装包映射逐条断言。

**尚未完成的部分（不得读成"已经能用"）：**

- **只有 Windows 形态。** 运行时是 `node.exe`。本仓库发布的安装包目标就是 `msi` / `nsis`，
  因此这不构成"跨平台方案"的缺口声明——其它平台**没有实现，也没有声称**（那边自动配置
  一律显示"当前不可写入"）。
- **没有在真实安装包上跑过一次端到端往返**（写入客户端配置 → 重启客户端 → 连上 Haven）。
  CI 断言的是"产物与命令形状正确、命令能起来并应答"，不等于客户端真的连上了。
- 打包器产出的清单里 `verified.runtimeExecuted` 恒为 `false`：它只描述输入内容与版本一致性；
  "真的跑起来了"由 CI 那一步单独断言，不由清单自称。

### 「一键配置 Codex / Claude Code」：**已实现（Windows 安装包形态），尚未真机验收**

授权范围被收窄到：只写两个平台固定路径（Codex `~/.codex/config.toml` 的
`[mcp_servers.haven]`、Claude Code `~/.claude.json` 顶层 `mcpServers.haven`）、只由用户在设置页
主动点击触发、结构化解析并只替换 `haven` 这一项、写前备份 + 同目录原子替换、**替换不得把原有
权限放宽**（Unix 设 0600；Windows 没有等价的模式位，改为原样保留原配置的 DACL，读不到或贴不上
就放弃写入）、不得写入任何凭据。完整边界见
[`MCP_EXTERNAL_AGENT_TRANSPORT.md`](../../docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md) §9.1.1。

写进客户端的 `command` / `args` 是**随包分发**的那两个文件
（`<resource_dir>/haven-mcp/runtime/node.exe` + `dist/index.js`），所以走这条路的用户既不需要
clone 仓库、也不需要装 Node。

实现在三处：判定在 `后端/crates/haven-application/src/services/mcp_client_config.rs`，
字节在 `后端/crates/haven-infrastructure/src/mcp_client_config.rs`（TOML 用 `toml_edit`，
JSON 用 `serde_json`），命令与控件在 `src-tauri/src/commands/mcp_client.rs` 与设置页
「智能功能 → 外部 Agent 接入」。`CODEX_HOME` / `CLAUDE_CONFIG_DIR` 一旦把客户端指向别处，
状态就是"当前不可写入"，栖阅不写任何东西并说明原因。

**仍然要说清楚的：** 上面那两节的手动步骤继续有效，也是**唯一经过验证**的路径；
自动配置没有在真实安装包与真实客户端上跑过一次往返，因此登记表里
`ai.clientAutoConfig` 的 `verifiedLive` 是 `false`。

两件事现在都做完了：

1. **运行时接进安装包**——`bundle.resources` + `tools/mcp/assemble-runtime.ps1`，
   CI 与发布流程都在 `tauri build` 之前调用它。
2. **写入通道**——结构化 TOML / JSON 合并、写前备份（同目录暂存 → 贴原访问控制 → 落字节
   → flush → 提升）、同目录原子替换、读后写前的**逐字节**核对、链接与竞态拒绝、环境变量
   重定向 fail closed，全部有单测（用例一律在临时目录里跑，不碰真实用户配置）。
   Windows 一侧的访问控制保留另有 `#[cfg(windows)]` 用例：原配置的 DACL **连同
   `SE_DACL_PROTECTED` 这一位**一起带走，替换件与备份都读回核对（同样只用临时目录，比较
   写入前后的控制位与 ACE 集合）。这些用例**本次仍未运行**——改动所在环境无法执行
   `cargo test`。

**唯一还缺的是验收**：没有在真实安装包与真实客户端上跑过一次「写入 → 重启客户端 →
连上 Haven」的往返。在那之前，本文件继续把手动路径写成唯一**经过验证**的路径。

日志全部走 **stderr**（stdout 是 JSON-RPC 通道，一句 `console.log` 就会破坏协议流；
有源码守卫测试盯着这条）。

---

## 工具契约要点

- **输入 schema 是 strict 的**：未知字段被**拒绝**，不是被忽略。丢掉 `apply: true`
  会让调用方以为自己的意图被接受了——那是最坏的一种失败。
- **命名**：工具名与输入字段一律 `snake_case`（对外协议）；Haven 内部 Wire 是 camelCase，
  两层映射由 `test/wire-drift.test.ts` 对着生成的 `wire.ts` 逐项钉死。
- **输出**：每个工具都有 `outputSchema`，返回 `structuredContent`。
  - `response_format: "json"`（默认）时，`JSON.parse(text)` 与 `structuredContent` **深度相等**；
  - `response_format: "markdown"` 时，文本由**同一个**对象渲染，结构化值里的每个标量都出现在文本里。
- **有界**：单字段 ≤ 512 字符，列表 ≤ 50 条，整份响应 ≤ 24 000 字符；
  超限时按工具声明的规则收缩（并把省略写进结构化值），仍超限则返回 `HAVEN_RESPONSE_TOO_LARGE`。
- **脱敏**：所有响应先过 `src/redact.ts`。凭据 target（`haven:<provider>:<id>`）、
  绝对路径、API key / Bearer 形状一律打码；`secret` / `credentialRef` / `sql` /
  `db_row` / `raw_provider_response` / `stack_trace` 等键**整键丢弃**。
- **错误**：稳定错误码（`SCREAMING_SNAKE_CASE`）+ 可展示消息 + `retryable`。
  非 `HavenMcpError` 的异常一律归一为 `INTERNAL_ERROR`，**不**回显原始消息或堆栈。
  > 注意：输入 schema 校验失败由 MCP SDK 生成消息，它会回显**调用方自己传入的值**。
  > 那不是跨方泄漏（同一方的输入回到同一方），但错误文本里不会出现任何 Haven 侧材料。

---

## 测试

```bash
npm run typecheck   # tsc --noEmit
npm run build       # tsc -> dist/
npm test            # vitest（含真实 stdio 端到端）
```

`npm test` 里的 stdio 端到端用例需要一个已构建的 `dist/`；缺失时会跳过并在测试名里说明。
完整门禁因此是 **`npm run build && npm test`**。

这三条都是**开发机**上的门禁，前置条件是 Node ≥ 22.12.0（`engines`）与一个已构建的 `dist/`；
随安装包分发的那份运行时与它们无关，用户机上不需要跑 `npm`。

**这里不给出通过计数。** 改动前的记录里留过两个互相矛盾的数（88 与 95 passed），无法判定
哪个对应本树；本轮修复也**没有重新运行** `npm test`（改动所在环境不能执行构建与测试）。
可核对的只有门禁本身：CI 的 `mcp` 作业在 `npm ci && npm run build` 之后执行 `npm test`，
结果以那次运行为准。`dist/` 已构建时会出现一个 skipped 的占位提示用例
（`dist 缺失时请先运行 npm run build`），属于预期行为。

覆盖范围：

| 主题 | 位置 |
| --- | --- |
| 恰好 9 个工具；禁用词根与显式禁名；注解；`additionalProperties:false`；snake_case | `test/tools.test.ts` |
| strict 输入：未知字段 / 非法枚举 / 锚点格式 / 跨字段一致性 | `test/tools.test.ts` |
| 只读零写入（工具层 + 端口层） | `test/tools.test.ts` |
| 提案只产生 `pending`，且结构里没有 token/secret/路径的位置 | `test/tools.test.ts` |
| 脱敏（含 SQL / DB 行 / Provider 原始响应 / 堆栈） | `test/tools.test.ts`、`test/redact.test.ts` |
| 响应有界与 shrink 循环、不收敛时的硬上限 | `test/respond.test.ts` |
| `structuredContent` 与文本同源（json 严格相等 / markdown 覆盖标量） | `test/tools.test.ts`、`test/respond.test.ts` |
| 桥接选择 fail-closed（fixture / 未知取值拒绝启动；live 缺端点或非法端点不静默降级） | `test/bridge-selection.test.ts` |
| live 本地传输：端点形态校验、帧编解码、长度前缀拆包重组、越界与错配响应拒绝 | `test/local-broker.test.ts`（**进程内假 transport，不是真的 Broker**） |
| Codex / Claude Code / DSH / Pi 配置夹具形状与未验证标记 | `test/client-fixtures.test.ts`、`fixtures/clients/*.json` |
| 生产入口行为 + stdout 洁净（真实子进程） | `test/stdio-e2e.test.ts` |
| stdout 洁净、无 fs/SQL/Tauri 依赖、端口无写方法、`node:net` 仅 connect | `test/source-guards.test.ts` |
| 枚举与 patch 字段对生成 `wire.ts` 无漂移 | `test/wire-drift.test.ts` |

**这些是单元 / 集成测试，不是生产实测。** 四份配置夹具已经存在，但仅用于验证统一模板、
客户端标识和 `verified: false` 的事实标记；真实客户端（Codex / Claude Code / DSH / Pi）、
真实 Windows WebView2 界面与 Haven→Node→MCP 端到端链路都没有验过。
