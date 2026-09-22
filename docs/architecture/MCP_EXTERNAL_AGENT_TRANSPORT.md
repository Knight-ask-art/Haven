# MCP 外部 Agent 传输设计（A5）

- 状态：**设计已冻结；A5 的核心已落地，但没有任何一项经过完整真实验收**。Rust Broker 内核、
  Windows 命名管道服务端、Tauri 命令 / `AppState` 接线、设置页「外部 Agent 接入」分组与
  Node live 适配器都已在仓库里并有测试；9 个工具的后端路径也已全部接通；真实客户端、
  真机 UI 与端到端链路尚未验收。
  逐项状态与证据见 §12。
- 上游边界：[`AI_SYSTEM.md`](./AI_SYSTEM.md)（§5 MCP 边界、§6 Skill 边界）
- 阶段拆分：[`docs/plans/2026-09-18-ai-provider-mcp-skill-plan.md`](../plans/2026-09-18-ai-provider-mcp-skill-plan.md) 的 A5 节

> 本版依据独立审查修订，相对首版有七处实质变更：取消发现文件改为显式端点配置（§4）、
> 请求集合冻结为 8 个 granted requests（`cancel` 仅为控制帧，见 §5.3）、会话与请求身份改由服务端生成（§6.2）、
> 配额写成确定契约（§8.3）、明确 `node:net` 的允许边界（§8.4）、
> Windows DACL 只授权当前用户（§4.4）、多实例改为 fail closed 且不声称支持（§7.4）。

## 目录

1. [目标与非目标](#1-目标与非目标)
2. [真实目标：同一工具契约，不同 stdio 配置](#2-真实目标同一工具契约不同-stdio-配置)
3. [拓扑与信任边界](#3-拓扑与信任边界)
4. [Broker 端点](#4-broker-端点)
5. [Broker 协议](#5-broker-协议)
6. [能力权威与身份](#6-能力权威与身份)
7. [生命周期](#7-生命周期)
8. [安全边界](#8-安全边界)
9. [外部 Agent 兼容矩阵](#9-外部-agent-兼容矩阵)
10. [威胁模型](#10-威胁模型)
11. [验收矩阵](#11-验收矩阵)
12. [实现状态与已知缺口](#12-实现状态与已知缺口)

---

## 1. 目标与非目标

### 目标

让**用户已经装好的**外部 Agent（Codex、Claude Code、DSH、Pi …）都能接入 Haven，
并且它们接入的是**同一套**工具契约、**同一个**安全内核：

- 一套 MCP 工具定义（A3 冻结的 9 个），不为任何客户端定制分支；
- 一条受控的本地 IPC 通道，把 MCP server 接到运行中的 Haven；
- 结论上只有两种能力：**读取脱敏上下文**、**创建 pending 提案**。Broker 的业务请求由
  6 个 Read 请求和 2 个 Propose 请求组成；第 9 个 MCP 工具 `get_system_capabilities`
  只在 Node MCP server 本地投影桥接与 Haven 能力状态，不进入 Broker `granted_requests`。

### 非目标（A5 明确不做）

- ❌ 不开放 TCP / Streamable HTTP / 任何网络监听。见 §8.5。
- ❌ 不做端点自动发现：不扫描文件系统、不枚举管道、不探测端口。见 §4.2。
- ❌ 不引入 Python sidecar、PydanticAI、完整 Agent runtime。
- ❌ 不让 Node MCP server 直接读 SQLite / 文件系统 / CredentialStore / invoke Tauri。
- ❌ 不提供 Apply / Approve / Reject / Delete / Filesystem / SQL / Secret 中的任何一项。
- ❌ 不为每个客户端写单独的适配器或"特权模式"。
- ❌ **不声称支持多实例同时提供 Broker**。见 §7.4。

---

## 2. 真实目标：同一工具契约，不同 stdio 配置

这是本设计最容易被说错的一点，先钉死：

> **各客户端之间的差异，只是"在配置里怎么写一条 stdio 命令"。**
> 工具名、输入 schema、输出 schema、错误码、安全语义**完全一致**。

Haven UI 提供的**同一份配置模板**（四个客户端共用；各客户端只是把这段放进自己的配置文件，
字段名可能不同，但 `command` / `args` / `env` 三项语义相同）：

```jsonc
{
  "command": "node",
  "args": ["<repo>/mcp/haven-mcp/dist/index.js"],
  "env": {
    // 由 Haven UI 显示并可一键复制；见 §4.2
    "HAVEN_MCP_BRIDGE": "live",
    "HAVEN_MCP_ENDPOINT": "\\\\.\\pipe\\haven-agent-v1-3f2a91c4"
  }
}
```

这就是当前 `buildAgentBrokerMcpTemplate` 逐字生成的形状（Windows 端点由
`JSON.stringify` 转义，手写拼接会产出无法解析的配置）。

因此 A5 的验收对象不是"四个客户端各自的适配器"，而是：

1. **同一份 MCP 工具契约**在四个客户端下都成立（§11 用 fixtures 断言，不是靠人工点）。
2. **同一个 Broker 协议**被四个客户端产生的 MCP 进程共用。

**不声称已经实测。** 四个客户端仍未跑通真实链路：`haven-mcp-server` 现在**有** live 适配器
（`HAVEN_MCP_BRIDGE=live` + 合法 `HAVEN_MCP_ENDPOINT`，见 §12），但**默认仍是
`UnavailableHavenAgentBridge`**，而且 Haven→Node→MCP 的真实端到端从未验证过——
现有 Node 连接用例用的是进程内假 transport，不是真的 Rust Broker。
§9 的矩阵标的是"设计目标"，不是"验证结果"。

---

## 3. 拓扑与信任边界

```text
┌─ 用户已有的 Agent 客户端 ────────────────────────────────┐
│  Codex / Claude Code / DSH / Pi                          │
│  （各自的 Agent runtime，各自的模型，各自不可信）           │
└───────────────────┬──────────────────────────────────────┘
                    │ ① MCP stdio（JSON-RPC，客户端拉起子进程）
                    ▼
┌─ haven-mcp-server（Node 进程，每客户端一个）──────────────┐
│  9 个冻结工具 · strict Zod · 脱敏 · 有界 · 同源响应        │
│  **不读** SQLite / 文件系统 / CredentialStore / Tauri      │
│  连接方式：node:net outbound client，目标来自               │
│  HAVEN_MCP_ENDPOINT（平台限定的本地端点，见 §4）            │
└───────────────────┬──────────────────────────────────────┘
                    │ ② Local IPC Broker（Named Pipe / Unix socket）
                    ▼
┌─ Haven 应用进程（Rust）──────────────────────────────────┐
│  Broker server（本设计新增）                              │
│    └─ 投影 8 个请求：6 Read + 2 Propose                  │
│       （MCP 的第 9 个工具 get_system_capabilities 在 Node 本地完成）│
│    └─ 服务端生成 AgentSessionId / AgentRequestId（§6.2）  │
│    └─ 配额前置校验（§8.3）                                 │
│  Typed Application service（AgentSettingsIpcService）     │
│    └─ 还持有 get_proposal / approve / reject / receipt     │
│       —— **不投影给 Broker**                               │
│  Domain：SettingProposal · canonical JSON · SHA-256 digest │
│  SQLite · CredentialStore · 文件系统                       │
└───────────────────┬──────────────────────────────────────┘
                    │ ③ 用户批准（唯一写入路径）
                    ▼
        Haven UI：Diff → 核对 digest → 批准 → CAS/Apply → Receipt
```

三条信任边界：

| # | 边界 | 谁不可信 | 控制手段 |
| --- | --- | --- | --- |
| ① | 客户端 ↔ MCP server | Agent 客户端、模型输出 | 工具清单本身（没有写方法）+ strict schema |
| ② | MCP server ↔ Haven | 本机其它进程、同用户恶意进程 | OS ACL + 闭合协议 + 握手 + 能力权威 + 配额 |
| ③ | 用户 ↔ 写入 | 一切自动化 | 只能由人在 Haven UI 批准 |

**关键点：边界②的请求集合比 Haven 的 Application service 面**窄**。**
Broker 现在投影 Agent Context 的 6 个 Read 用例和 Agent Settings 的 2 个 Propose 用例；
`get_proposal` / `approve` / `reject` / `receipt` 仍是 UI 的权限，Broker 里不存在对应帧——
不是"关着"，是**没有这个帧类型**，因此无法被构造出来。§11 有验收专门断言这个投影关系。

---

## 4. Broker 端点

### 4.1 端点形态

| 平台 | 端点 | 说明 |
| --- | --- | --- |
| Windows | Named Pipe `\\.\pipe\haven-agent-v1-<sid8>` | 不占端口、不进网络栈 |
| macOS / Linux | `$XDG_RUNTIME_DIR/haven/agent-v1.sock`；若 `XDG_RUNTIME_DIR` 未设置或不是绝对路径，则为 `$HOME/.haven/run/agent-v1.sock` | 目录 `0700`、套接字 `0600` |

两者承载**同一个**字节协议（§5），只是 OS 原语不同。不允许"Windows 一套协议、Unix 另一套"。

### 4.2 每用户稳定端点 + 显式配置（**没有发现文件**）

**首版曾设计过"发现文件"，现已删除。** 理由：让 Node 去读一个约定路径的文件来定位端点，
等于引入一条隐式的、可被本机任意进程抢夺的间接层——它既扩大了 Node 侧的职责
（碰文件系统），又让"连到哪个端点"变成不可审计的事。

替代方案：**端点由 Haven UI 明示，用户显式配置。**

- 端点名**每用户稳定**：不随 Haven 重启变化，因此配置文件写一次即可长期有效。
  - Windows：`<sid8>` = 当前用户 SID 的 SHA-256 前 8 位十六进制（稳定、每用户唯一）。
  - Unix：`<runtime-dir>` = `$XDG_RUNTIME_DIR`（仅当绝对路径）；未设置或不是绝对路径时回退
    `$HOME/.haven/run`（`0700`）。
    该目录本身就是每用户隔离的，因此套接字名固定即可。
- Haven UI（设置 → 智能功能）在用户开启外部接入后展示：
  1. 端点的**完整字符串**与「复制」按钮；
  2. 四个客户端通用的 MCP 配置模板（§2），其中已填好 `HAVEN_MCP_BRIDGE` 与
     `HAVEN_MCP_ENDPOINT`。
- **当前代码没有「测试端点」按钮，登记为后续待补。** 在它存在之前，不要把"端点已显示"
  读成"端点已验证可达"：UI 只保证端点来自真实监听状态，不保证对端可连。
- 端点与模板**只在真实监听**（`listening` 且端点形态合法）时给出。浏览器 Mock 返回的
  `mock://…` 是预览标识，既不可复制也不进模板——把 `mock://` 粘进客户端只会连不上。
- Node local bridge 只认 `HAVEN_MCP_ENDPOINT` 环境变量：
  - **未设置** → 不连接，返回 `HAVEN_BRIDGE_UNAVAILABLE`（不猜、不扫描、不回退）；
  - **格式非法** → 返回 `HAVEN_MCP_ENDPOINT_INVALID`，不尝试连接。

**端点名不是秘密。** 它是可复制的配置值，因此必然可被同机进程猜到。安全性来自
OS ACL（§4.4）与"这条通道只暴露两件事"（§8.1），**不**来自名字的隐蔽性。这一点必须
写清楚，避免有人把"名字看着随机"当成一层防护。

### 4.3 端点格式校验（平台限定，闭合）

`HAVEN_MCP_ENDPOINT` 必须精确匹配其一，否则拒绝：

| 平台 | 形态 | 校验 |
| --- | --- | --- |
| Windows | `\\.\pipe\haven-agent-v1-<8 位小写十六进制>` | 前缀精确匹配 `\\.\pipe\haven-agent-v1-`，其后恰好 8 位 `[0-9a-f]`；总长 ≤ 256 |
| Unix | `$XDG_RUNTIME_DIR/haven/agent-v1.sock`（仅当 `XDG_RUNTIME_DIR` 是绝对路径），或 `$HOME/.haven/run/agent-v1.sock`（回退路径） | 绝对路径；必须精确等于当前用户环境推导出的其中一条固定路径；字节长 ≤ 100（同时低于 Linux 107 与 macOS 103 的 `sun_path` 限制） |

明确拒绝：任何 scheme（`tcp://` / `http://` / `https://` / `ws://` / `file://`）、
相对路径、含 `..` 的路径、UNC 共享路径（`\\server\share\...`）、超出长度上限的值、
以及不匹配上述平台的任何字符串。

**不允许的行为**：扫描文件系统找端点、枚举管道/套接字、探测端口、读取任意路径。
端点只能来自这一个环境变量。

### 4.4 OS ACL

| 平台 | 要求 |
| --- | --- |
| Windows | Named Pipe 安全描述符：DACL **只授予创建者用户 SID**。**不**授予 `SYSTEM`、不授予 `Everyone` / `Authenticated Users` / 匿名 |
| Unix | 套接字文件 `0600`、活实例锁文件 `agent-v1.lock` 为 `0600`、父目录 `0700`，且**在 `bind` 之前**设好 umask，避免"先创建后 chmod"的窗口；Haven 管理的 `haven`（或回退路径中的 `.haven`/`run`）组件不得是符号链接，最终目录以 `O_DIRECTORY | O_NOFOLLOW` 打开并校验为当前用户拥有的目录 |

**关于 `SYSTEM`**：首版把"创建者 SID + `SYSTEM`"作为默认，本版收紧为**只授当前用户**。
理由：桌面版 Haven 以交互用户身份运行，没有任何组件需要 `SYSTEM` 访问这条通道；把
`SYSTEM` 写进 DACL 是为一个不存在的服务场景预留权限。若将来出现真正的服务模式
（Haven 以服务身份运行、需要跨会话访问），那是一条**独立的安全评审**，届时再放宽，
并且要连同"服务与桌面实例如何共存"一起设计。

ACL 是**主要**控制手段；协议层的握手、能力清单与配额是纵深防御，不是替代。

### 4.5 默认关闭，用户显式开启

**Broker 默认不监听。** 用户在 Haven 设置 → 智能功能里显式打开"允许外部 Agent 接入"
之前：不创建端点、不在 UI 显示端点与模板。关闭时：关闭监听、终止在途连接、
从 UI 撤下端点显示。

当前实现即如此：未开启时 `agent_broker_status` 返回 `disabled` 且**不带端点**，设置页只
显示「已关闭」，不给复制按钮也不给模板。

理由：这是一个**本机任意进程都可尝试连接**的入口。默认开启等于把"是否暴露"的决定
替用户做了。默认关闭让"暴露"成为用户知情的选择。

---

## 5. Broker 协议

### 5.1 Framing 与上限

- 每帧 = `4 字节大端无符号长度` + `UTF-8 JSON 载荷`。
- 长度前缀只描述载荷；载荷必须是**单个** JSON 对象，不允许尾随字节。
- 上限（闭合、写死，不协商）：

| 项 | 上限 | 超限行为 |
| --- | --- | --- |
| 帧载荷 | 256 KiB | 立即断连，不回复 |
| 单请求 | 64 KiB | 回 `HAVEN_BROKER_REQUEST_TOO_LARGE` |
| 单响应 | 256 KiB | 回 `HAVEN_BROKER_RESPONSE_TOO_LARGE` |
| 每连接在途请求 | 8 | 第 9 个回 `HAVEN_BROKER_TOO_MANY_INFLIGHT` |
| 并发连接 | 8 | 拒绝新连接（不排队） |
| 未知帧类型 | —— | 回 `HAVEN_BROKER_UNKNOWN_FRAME`，**不断连** |

响应上限 256 KiB **刻意高于** MCP 侧 24 000 字符的响应上限：MCP 层已把响应裁到
24 000 字符（CJK 下最坏约 72 KiB），Broker 留出余量，避免"两层各自截断、结果都不对"。

### 5.2 握手

连接建立后，**第一帧必须是 `hello`**，否则断连。

```jsonc
// client → server
{ "type": "hello",
  "protocol_version": 1,          // 必须与 server 支持的版本严格相等
  "client": {                      // **仅供参考**，绝不用于授权或领域身份（§6）
    "name": "claude-code",         // 自由字符串，不可信
    "version": "1.2.3",
    "instance_id": "..."           // 客户端自生成的随机标识，仅用于日志关联
  } }

// server → client
{ "type": "welcome",
  "protocol_version": 1,
  "session_id": "<hex>",           // **服务端生成**，见 §6.2
  "haven": {
    "app_version": "0.1.0-beta.1",
    "agent_api_version": 1,        // 与 AgentCapabilityManifest 同源
    "capabilities": {              // **权威**：服务端说的才算（§6.1）
      "settings_read": true,
      "settings_proposal": true,
      "setting_sources_read": true,
      "resource_preference_read": true,
      "resource_preference_proposal": true,
      "library_summary_read": true,
      "media_capabilities_read": true,
      "onboarding_read": true,
      "metadata_proposal": false,
      "rename_proposal": false,
      "secret_read": false,
      "filesystem_write": false
    }
  },
  "granted_requests": [
    "context", "setting_sources", "resource_preference", "library_summary",
    "media_capabilities", "onboarding", "create_proposal", "create_resource_proposal"
  ] }
```

握手超时 5 s；超时或版本不等 → `HAVEN_BROKER_PROTOCOL_MISMATCH` 并断连。
版本不等时**不做降级兼容**：宁可让客户端拿到明确错误，也不要让它以为协商成功。

### 5.3 请求闭合集合（**恰好 8 项；`cancel` 是控制帧**）

| `type` | 对应 Application 用例 | 语义 |
| --- | --- | --- |
| `context` | `AgentSettingsIpcService::context` | 读脱敏设置上下文（context_id / hash / revision / 能力清单） |
| `setting_sources` | `AgentContextQueryService::setting_sources` | 读设置来源与权威层信息 |
| `resource_preference` | `AgentContextQueryService::resource_preference_snapshot` | 读 edition / media item 作用域阅读偏好 |
| `library_summary` | `AgentContextQueryService::library_summary` | 读有界媒体库统计摘要 |
| `media_capabilities` | `AgentContextQueryService::media_capabilities` | 读媒体项可用能力摘要 |
| `onboarding` | `AgentContextQueryService::onboarding_state` | 读引导完成状态 |
| `create_proposal` | `AgentSettingsIpcService::create_proposal` | 创建全局 reading pending 提案 |
| `create_resource_proposal` | `AgentContextQueryService::create_resource_preference_proposal` | 创建资源级 reading pending 提案 |

```jsonc
// 通用请求外壳
{ "type": "<上表之一>", "id": 17, "payload": { /* 见下 */ } }

// context —— 无参数
{ "type": "context", "id": 1, "payload": {} }

// create_proposal —— 载荷与 MCP 侧 propose_settings_patch 同形
{ "type": "create_proposal", "id": 2, "payload": {
    "section": "reading",
    "context_id": "…", "context_hash": "…(64 位小写十六进制)", "base_revision": "…|null",
    "patch": { "font_size": "large" } } }
```

**为什么没有 `get_proposal`**（首版有，本版删除）：

1. A3 冻结的 `HavenAgentBridge` 端口**没有**这个方法——加上它就要改冻结端口；
2. A3 冻结的 9 个 MCP 工具里**没有**对应的外部工具——加上它就要改冻结工具清单；
3. 批准之后的状态与 Receipt **仍归 Haven UI**：把回读交给外部 Agent，会诱导
   "Agent 声称已生效"，而 Skill 明确禁止这类表述。

因此 v1 的 granted request 集恰好 8 项；`cancel` 只控制当前连接的在途任务，不是业务
能力，也不进入 `welcome.granted_requests`。将来若要让外部 Agent 回读提案状态，必须先同时修改
冻结端口与冻结工具清单，并单独评审——不在 A5 内。

**不存在的请求类型**（因此无法被构造）：`get_proposal` / `approve` / `reject` / `apply` /
`delete` / `write` / `rename` / `receipt` / `sql` / `fs` / `secret` / `exec` / `invoke`。

### 5.4 响应闭合集合（2 项）

```jsonc
// 成功
{ "type": "ok", "id": 17, "payload": { /* 与对应用例的 DTO 同形 */ } }

// 失败
{ "type": "error", "id": 17,
  "error": { "code": "HAVEN_CAPABILITY_UNAVAILABLE",
             "message": "…（可展示，已脱敏）",
             "retryable": false } }
```

失败响应**不含** `payload`，也不含堆栈、路径、SQL 或原始 Provider 报文。

### 5.5 取消

```jsonc
{ "type": "cancel", "id": 17 }        // 引用在途请求的 id
```

- 收到 `cancel` 后服务端尽力中止该请求；已进入 Proposal 内核的创建**不回滚**
  （提案要么已落库要么没有，不做半程撤销）。
- 对已完成/未知 id 的 `cancel` 幂等：回 `ok`，不改状态。
- 客户端断连时，服务端视为对该连接全部在途请求的取消。

### 5.6 稳定错误码

| 码 | 含义 | retryable |
| --- | --- | --- |
| `HAVEN_MCP_ENDPOINT_INVALID` | `HAVEN_MCP_ENDPOINT` 格式非法（客户端侧） | false |
| `HAVEN_BROKER_PROTOCOL_MISMATCH` | 协议版本不符 / 首帧不是 `hello` | false |
| `HAVEN_BROKER_UNKNOWN_FRAME` | 未知帧类型 | false |
| `HAVEN_BROKER_REQUEST_TOO_LARGE` | 单请求超限 | false |
| `HAVEN_BROKER_RESPONSE_TOO_LARGE` | 单响应超限 | false |
| `HAVEN_BROKER_TOO_MANY_INFLIGHT` | 每连接在途请求超过 8 | true |
| `HAVEN_BROKER_QUOTA_EXCEEDED` | 触碰 §8.3 的提案配额 | false |
| `HAVEN_BROKER_NOT_ENABLED` | 用户未开启外部 Agent 接入 | false |
| `HAVEN_BROKER_ENDPOINT_BUSY` | 端点已被另一个 Haven 实例占用 | false |
| `HAVEN_BROKER_TIMEOUT` | 请求超时 | true |
| `HAVEN_BROKER_BUSY` | Haven 正忙，暂时不接受 | true |
| `HAVEN_CAPABILITY_UNAVAILABLE` | 用例未实现 | false |
| `INVALID_ARGUMENT` | 载荷不合法 | false |
| `SETTING_PROPOSAL_*` | Haven 提案内核的稳定拒绝码，原样透传 | 见内核 |

错误码形状沿用 Haven 既有约定：`SCREAMING_SNAKE_CASE`，`message` 是可展示文案。
客户端以 `code` 判定，不解析 `message`。

---

## 6. 能力权威与身份

### 6.1 能力以服务端为唯一权威

- 客户端不得缓存跨会话的能力、不得自行推断、不得因为"上次能用"就当作这次也能用。
- 服务端在**每次请求**时按当前能力再次校验，而不是只信握手时那一份。
- `hello.client`（`clientInfo`）是**自报字符串**，只用于日志与诊断。它不参与授权、
  不参与能力判定、不改变任何行为、**也不成为领域身份**。一个自称 `claude-code` 的进程
  与一个自称 `unknown` 的进程拥有完全相同的权限。

当前 Haven 侧能力（`AgentCapabilityManifest::for_current_slice`）：

| 能力位 | 值 | 说明 |
| --- | --- | --- |
| `settings_read` | ✅ true | |
| `settings_proposal` | ✅ true | |
| `setting_sources_read` | ✅ true | |
| `resource_preference_read` | ✅ true | |
| `resource_preference_proposal` | ✅ true | |
| `library_summary_read` | ✅ true | |
| `media_capabilities_read` | ✅ true | |
| `onboarding_read` | ✅ true | |
| `metadata_proposal` | ❌ false | 本版本明确不做 |
| `rename_proposal` | ❌ false | 本版本明确不做 |
| `secret_read` | ❌ false | 永不提供 |
| `filesystem_write` | ❌ false | 永不提供 |

后四位不是"暂时关着"：`AgentCapabilityManifest::validate` 会**拒绝**任何声称拥有它们的清单。

### 6.2 会话与请求身份由服务端生成

**`session_id` 与 `request_id` 不由客户端传入。**

- 握手成功后，**Rust Broker 生成** `AgentSessionId`，绑定到该连接并**持久到连接生命周期**
  （同一连接上的多次 `create_proposal` 共享同一个 session）。
- 每个**通过校验的** frame id 被**映射为新的** `AgentRequestId`。客户端给的 frame `id`
  只是连接内的关联编号，不是领域标识，也不直接当作 `AgentRequestId` 使用。
- `hello.client` / `clientInfo` **只能做诊断**，绝不成为领域身份。

**必须注意的现有实现差异**：今天 Tauri UI 路径下，`session_id` / `request_id` 是
**由 wire 请求携带**的（`commands/agent.rs` 从 `request.session_id` / `request.request_id`
解析后传给 `create_proposal`）。那是 UI 作为可信本机调用方的形态。

**Broker 路径不能沿用这一点**：Broker 的 `create_proposal` 帧里**没有**这两个字段
（§5.3 的载荷就是全部），因此客户端无法提供它们；Rust 侧必须在握手时自造 session、
在每帧自造 request 后，才调用 `AgentSettingsIpcService::create_proposal`。

A5.4 的这条实现门禁**已经落地并有测试钉住**：`src-tauri/src/agent_broker/session.rs` 的
`create_proposal_generates_identity_and_rejects_client_supplied_ids`（帧里带
`session_id` / `request_id` 会被拒）与
`all_create_proposals_share_one_session_but_get_distinct_requests`（同连接共享 session、
每帧得到新的 request）。

---

## 7. 生命周期

### 7.1 超时

| 阶段 | 上限 | 超时行为 |
| --- | --- | --- |
| 连接后首帧 `hello` | 5 s | 断连 |
| 单次 Broker→client 写出 | 5 s | 断连（写不出去就是连接失败，不再追写一条错误回复） |
| 单请求处理 | 20 s | 回 `HAVEN_BROKER_TIMEOUT`，不断连 |
| 空闲（无在途请求） | 30 min | 服务端主动断连 |
| MCP 侧调用 Broker | 15 s（`BRIDGE_TIMEOUT_MS`） | 转成 MCP 工具的稳定错误 |

Broker 的 20 s **刻意大于** MCP 侧的 15 s：让 MCP 层先超时并给出它自己的稳定错误，
而不是两层同时超时、错误归因不明。

握手与写出这两项是**连接 permit 的守卫**。连接数上限（§5.1）是有限的，而一条"连上却不发
首帧"或"连上却不再读取"的连接，如果没有固定上限就会一直占着 permit，把上限逐个耗光，
Broker 侧还看不出异常。两者的超时都按连接失败收尾——**直接断连**，不把超时写成一个不可靠
的回复（写不出去正是问题本身）。窗口值由 `listener.rs` 的内部函数参数注入，测试用毫秒级值
覆盖，因此既不用真等 5 秒，也不依赖真实 Named Pipe / Unix socket。

### 7.2 断线与 Haven 退出

- **客户端断开**：服务端清理该连接的全部在途请求、取消登记与配额计数；不影响其它连接。
- **Haven 退出**：端点消失 → MCP 侧下一次调用连接失败 → 转成稳定错误
  （`HAVEN_BRIDGE_UNAVAILABLE`，`retryable: true`）。已创建的提案不受影响（已落库）。
- **Haven 崩溃**：同上。因为**没有发现文件**，不会留下需要清理的陈旧指针——
  端点名是稳定的，重启后仍是同一个名字。唯一例外是 Unix 端点在固定路径上留下的 socket
  文件（进程被 `SIGKILL` 时内核不会替它 `unlink`）：那不是"发现文件"，恢复也只针对这**一个**
  固定路径，见 §7.3。
- MCP server **不**因为 Haven 退出而自杀：它继续服务 `get_system_capabilities`，
  如实报告 `haven.available = false`。这是 A3 已确立的诚实路径。

### 7.3 端点不可用与陈旧端点恢复

**客户端侧（Node MCP server）**：没有发现文件，因此不存在"stale 指针"。取而代之的是
**连接层面的失败**：

1. 连接失败（端点不存在 / 被拒绝 / 握手版本不符）；
2. → 回稳定错误（`HAVEN_BRIDGE_UNAVAILABLE` 或 `HAVEN_BROKER_PROTOCOL_MISMATCH`）；
3. **不重试其它端点**、**不扫描**、**不猜测**、**不回退到任何默认值**。

"不扫描"是刻意的：自动发现会把"用户显式开启的通道"变成"任何监听同名管道的进程
都可能被自动选中"。

**Haven 侧（Unix 端点的崩溃恢复）**：Unix 的端点是**文件系统里的一个名字**，进程被
`SIGKILL` 时内核不会替它 `unlink`，所以固定路径上会留下一个没人听的 socket 文件。
下一次 `enable` 若不加区分，就会把这个**自己的残留**报成"另一个实例在跑"
（`HAVEN_BROKER_ENDPOINT_BUSY`）——于是崩溃一次之后，外部接入再也开不起来。

因此 Unix 侧在端点之外增加一层**每用户活实例锁**（Windows 侧不需要：命名管道随进程消失，
`FILE_FLAG_FIRST_PIPE_INSTANCE` 已经表达了同一件事）：

| # | 步骤 | 判据 | 结果 |
| --- | --- | --- | --- |
| 1 | 运行目录收紧到 `0700` | —— | 失败 → `HAVEN_BROKER_ENDPOINT_UNAVAILABLE` |
| 2 | 打开/创建 `<endpoint-parent>/agent-v1.lock`（`0600`、`O_NOFOLLOW`，只接受当前用户拥有的普通文件）并 `flock(LOCK_EX \| LOCK_NB)` | `EWOULDBLOCK` / `EAGAIN` = 锁被另一个**活**实例持有 | `HAVEN_BROKER_ENDPOINT_BUSY`，**一个文件都不动** |
| | | 其它 `flock` 失败 = 说不清楚 | `HAVEN_BROKER_ENDPOINT_UNAVAILABLE`（fail closed，但**不**谎称"另一个实例在跑"） |
| 3 | `bind` 端点 | 成功 | 端点置 `0600`，进入监听 |
| | | `EADDRINUSE` → 做一次 liveness probe | 见第 4 步 |
| 4 | probe：本地 `connect(端点)`，上限 1 s | 连上 / 超时 / 认不出的 errno = 有人在听或结果不确定 | 保持 `HAVEN_BROKER_ENDPOINT_BUSY`，**不删除** |
| | | `ECONNREFUSED` / `ENOENT` = 明确没有 listener | 删除**这一个**端点路径，重试 `bind` **一次**；若路径不是 socket（普通文件、目录或符号链接），保持原样并返回 `HAVEN_BROKER_ENDPOINT_UNAVAILABLE` |
| 5 | 重试 `bind` | 仍然 `EADDRINUSE` | `HAVEN_BROKER_ENDPOINT_BUSY`（有一个不受这把锁约束的监听者，不再删） |

四条硬约束：

- **判据是锁，不是锁文件的存在性。** `flock` 由内核随进程消亡（含 `SIGKILL`）释放，因此它
  回答的是"**此刻**有没有活实例"；"锁文件存不存在"只能回答"曾经有没有实例"。锁文件**永不删除**，
  删它既不释放锁也不表达任何东西。
- **删除只针对这一个固定端点路径，且只针对 socket 文件。** 删除前用 `lstat`
  （`symlink_metadata`）确认它确实是 socket：常规文件、目录、**符号链接**一律原样保留并
  fail closed——那说明路径被别的东西占了，不是我们的残留。不跟随符号链接去删它的目标，也不
  删除任何其它路径。非 socket 占用返回 `HAVEN_BROKER_ENDPOINT_UNAVAILABLE`，不把它冒充成
  另一个 Haven 实例。
- **正常情况下活 socket 永不被删。** probe 连得上就说明有人在 accept，那是别人的端点，不是残留。
  但 Unix 的 `connect → lstat → unlink` 无法对同用户、非 Haven 进程提供跨进程原子 inode
  身份检查；如果不可信同用户进程恰好在 probe 与 unlink 之间抢占同一路径，代码只能依赖窗口极短
  与既有“同用户进程不是可信方”的威胁模型。要把这条升级成竞态下的绝对保证，需要另行设计
  `rename`/目录 fd 级别的接管协议，不属于本期范围。
- **锁持有到 `serve` 收尾结束**（含删除 socket 文件）：收尾窗口里第二个实例仍然拿不到锁，
  不会把"正在收尾"误判成"端点可以接管"。

这把锁**不改变对外契约**：端点字符串（§4.3）、权限（§4.4）与协议（§5）都不变；
`agent-v1.lock` 不是端点，也不是发现文件，它只在同一目录里表达"有没有活实例"。

### 7.4 多实例：fail closed（**不声称支持**）

端点名为每用户稳定，因此同一用户下**同时只能有一个 Haven 实例提供 Broker**。

- 第一个实例创建端点成功 → 它提供 Broker。
- 第二个实例创建端点失败（Windows：首实例带 `FILE_FLAG_FIRST_PIPE_INSTANCE` 时
  `ERROR_ACCESS_DENIED` / `ERROR_PIPE_BUSY`；Unix：拿不到活实例锁）→ **fail closed**：
  - **不**接管、**不**删除、**不**重命名对方的端点；
  - **不**改用自己的另一个端点（那会让用户配置失效并制造两个可连目标）；
  - 在 UI 上如实告知："检测到另一个 Haven 实例正在提供外部 Agent 接入；
    本实例的该功能不可用。"对应错误码 `HAVEN_BROKER_ENDPOINT_BUSY`。
- **A5 不声称支持多实例同时提供 Broker**。这是第一版的明确边界，不是待修的缺陷。
  将来若需要，必须重新设计端点命名与实例选择，并单独评审。

两条平台各自有一个"先到者得"的原语，语义相同：

| 平台 | 原语 | 位置 |
| --- | --- | --- |
| Windows | `FILE_FLAG_FIRST_PIPE_INSTANCE`：同名 pipe 已存在时首实例的 `CreateNamedPipeW` 失败，归类为 `HAVEN_BROKER_ENDPOINT_BUSY` | `listener.rs` |
| Unix | 每用户活实例锁 `flock(LOCK_EX \| LOCK_NB)`（§7.3）：拿不到锁即 `HAVEN_BROKER_ENDPOINT_BUSY` | `listener.rs` |

Windows 侧的这条归类**只对首实例成立**。`ERROR_ACCESS_DENIED` 只有在
`FILE_FLAG_FIRST_PIPE_INSTANCE` 下才是"同名 pipe 已存在"的证据；后续等待实例（`first = false`）
不带该标志，同一个错误码来自安全描述符层，被归类为 `HAVEN_BROKER_ENDPOINT_UNAVAILABLE`。
把"本实例创不出端点"说成"另一个实例在跑"会让用户去找一个并不存在的第二实例，而真实的
失败原因（DACL 构造被拒等）被掩盖。

**`busy` 只通过 `enable` 的错误返回，`status` 不会主动投影它。** 判定"端点是否被占用"
唯一可靠的时刻是真正尝试 `bind` 的那一次；`agent_broker_status` 按设计不连接、不探测端点
（§4.5），因此它只可能报 `disabled` / `listening` / `unavailable`，**不会**凭一次非探测的
读取断言"另一个实例在跑"。这条刻意如此：把不确定的探测结果写成 `busy` 会制造虚假可达性——
UI 会告诉用户"端点被另一个实例占用"，而实际原因可能只是本实例的环境不可用。
`HAVEN_BROKER_ENDPOINT_BUSY` 因此是一条**由 `enable` 返回的失败**，不是一个
`agent_broker_status` 会主动发现的状态。

Unix 侧的判据刻意**不是**"socket 文件存不存在"：那个文件在进程被 `SIGKILL` 后会留下来，拿它
当判据等于把每一个崩溃残留都当成一个活实例（§7.3）。

能钉住这条的用例：`second_bind_on_same_endpoint_is_rejected`（该用例在首个 bind 拿不到端点时
会提前返回，因此它**不能**替代多实例真机验收）；Windows 侧另有
`access_denied_is_busy_only_for_the_first_pipe_instance` 钉住上述归类边界。Unix 侧有
`unix_endpoint::{instance_lock_alone_rejects_a_second_instance, live_socket_is_never_deleted}`
与 `stale_socket_from_a_killed_instance_is_recovered`——本机是 Windows，它们**本机仍未编译、
未运行**；CI 已新增 Linux all-targets 作业（§12），但该作业**尚未在 GitHub 上运行过**。

客户端侧同时开多个 Agent 会话是支持的：多个 MCP 进程 → 多个连接 → 各自独立
`AgentSessionId`（§6.2）。

---

## 8. 安全边界

### 8.1 允许 / 禁止（闭合）

```text
ALLOWED（两类、八个请求）
  context / setting_sources / resource_preference / library_summary
  media_capabilities / onboarding
                   读取脱敏、有限、无正文的上下文
  create_proposal / create_resource_proposal
                   创建 pending 提案（零写入设置）

CONTROL（不是业务能力）
  cancel           取消当前连接上的在途请求；不进入 granted_requests

FORBIDDEN（协议里**不存在**对应帧，因此无法构造）
  get_proposal · approve · reject · apply · write · delete · rename
  filesystem（读或写） · sql · secret_read · receipt
  exec / invoke / 任意命令 · 任意 prompt 直通
```

工具面（MCP 侧）与请求面（Broker 侧）同构：9 个 MCP 工具全部有对应的 Broker / Application
路径；其中 `get_system_capabilities` 负责本地能力状态投影，其余 8 个请求经过 Broker。
能力仍由服务端逐请求重校验，且默认 Broker 关闭，所以真实调用可能得到
`HAVEN_BRIDGE_UNAVAILABLE`，这与后端用例是否存在是两回事。

资源级 `create_resource_proposal` 只提交 Agent 请求的部分 patch，不把 authoritative 偏好
合并后回传或保存成 Proposal。真正批准时由 Haven 在同一 UoW 内重读、合并并 CAS；Agent
Receipt 若由内部路径产生，也只保存脱敏的 before/after 审计投影。原始设置事实仍留在 Haven
内部，Broker/MCP 不提供 Receipt 读取帧，也不把路径、endpoint 或凭据样式文本交给外部 Agent。

### 8.2 同用户恶意进程——诚实限制

**必须说清楚：OS ACL 挡不住同一用户下的恶意进程。**

Windows DACL 限制到创建者 SID、Unix `0600` 限制到属主，挡的是**其它用户**与低权限上下文。
但同一登录会话下、以同一用户身份运行的任意程序，只要知道端点名（而端点名按设计
就是可复制的、不是秘密），就能连上并发起 `context` / `create_proposal`。因此：

- **不把"同用户进程"当作可信方。** Broker 的授权模型建立在"这条通道只暴露只读 +
  创建 pending 提案"之上，而不是"只有我们的 MCP server 能连上"。
- 最坏后果被限制为：读到用户自己本来就能读的设置快照，以及**往提案表里塞垃圾行**。
  它**不能**改设置——写入需要人在 UI 批准。
- 因此提案配额是**必需**的补偿措施（§8.3），不是可选项。
- 未来若要把 Broker 提升为"可写"通道，必须先引入**能区分同用户进程**的机制
  （Windows 上校验客户端进程完整性级别/签名，Unix 上校验 peer credentials +
  可执行文件路径），那是**另一个**安全评审，不能靠现有 ACL 顺延。

### 8.3 配额（确定契约，不留"以后再定"）

| 配额 | 上限 | 作用域 |
| --- | --- | --- |
| 在途请求 | **8** | 每连接（= §5.1 的帧级上限） |
| 外部提案创建尝试 | **8** | 每连接 |
| 外部提案创建尝试 | **32** | 每个 Haven **进程窗口**（本次运行期间） |

- 超出任一项 → `HAVEN_BROKER_QUOTA_EXCEEDED`，**在进入 `AgentSettingsIpcService` 之前拒绝**，
  零写入。
- 这里统计的是创建尝试，不是假定 Application 成功后的 pending 数：调用失败时也不退款，
  因为失败可能发生在 Proposal 已落库但响应投影失败之后；这是一条更保守、可证明的
  fail-closed 噪音上限。
- 成功创建的 pending 提案仍沿用既有 `DEFAULT_SETTING_PROPOSAL_TTL_MS` = **24 小时**
  （`haven-application/src/services/setting_proposals.rs:50`），不新增第二套过期语义。
- 连接断开时，在途计数随之释放；尝试配额属于连接/进程窗口，不因断开或重连退款。

**数值理由**：

- **8**（在途）与 `§5.1` 的帧级并发上限一致——一个数值只定义一次，避免两处各写一个。
- **8**（每连接尝试）远小于"人类愿意逐条审阅"的规模；一个正常的 Agent 交互会
  提议一两次就等用户批准，8 已经宽松。
- **32**（进程窗口尝试）是一次运行期间外部创建工作的上界。它足够覆盖长时间使用，
  又能在被恶意进程或反复失败的 Agent 灌水时把总量挡在可人工清理的范围内。

**这是进程窗口配额，不是跨重启的持久计数。** 换句话说：重启 Haven 会把 32 清零。
残留风险与为何接受：

- 重启需要用户本人操作（或已经能控制该用户会话），此时攻击者能做的事远不止灌提案；
- 即便被清零，恶意进程能得到的**仍然只是"多发起一些创建尝试"**——成功时最多写入
  pending 提案，但它无法 Apply，
  无法绕过 UI 批准。配额的作用是抑制噪音，不是充当权限边界；
- 若做成持久计数，就要新增一张持久表并处理迁移/清理，收益（防一个本来就不构成提权的
  噪音）不抵复杂度。将来若真出现滥用证据，再升级为持久计数并单独立项。

### 8.4 `node:net` 的允许边界

Node local bridge 需要连接本地端点，因此在 A5.5 之后，`mcp/haven-mcp/src` 需要
**有限地**放开网络模块。边界如下：

```text
ALLOWED
  node:net —— 仅用于 net.connect() 到 §4.3 校验通过的本地端点

FORBIDDEN（src/ 内一律不得出现）
  node:http · node:https · node:dgram · node:tls · node:child_process
  net.createServer · server.listen（任何形式的监听）
  node:fs（读或写） · sqlite/数据库驱动 · keyring/凭据库 · @tauri-apps/api
```

**这是本地 IPC 客户端，不是网络访问。** 具体含义：

- 不发 DNS 查询、不连远程地址、不监听任何端口；
- 不做任何文件系统访问（端点来自环境变量，不来自文件）；
- 目标地址只能是 §4.3 的两个平台形态之一，由白名单前缀校验，拼不出任意 socket 路径。

**A3 的源码守卫已按此调整完毕**（A5.5 交付时完成，不再是"待办"）：
`mcp/haven-mcp/test/source-guards.test.ts` 把 `node:net` 从 `forbiddenModules` 移出，并新增
专门用例断言 `src/local-broker.ts` 只 import `node:net`、且不含 `createServer(` / `.listen(`；
其余模块（`node:http` / `node:https` / `node:dgram` / `node:tls` / `node:child_process`）
与文件系统 / SQLite / 凭据库 / Tauri 模块仍然整条禁止。

**`node:dgram` 与 `node:tls` 已由 `forbiddenModules` 明确禁止。** 这两项曾被登记为
"守卫列表未覆盖、靠无人引入"的残余缺口，现已补齐：`forbiddenModules` 里同时列有
`node:dgram` 与 `node:tls`，`src/` 下任何一条 import 命中即测试失败。这是**源码层**
的静态约束；它**不构成**真实客户端、真实 Rust Broker 或 WebView 界面的验收——
那几项仍未实测（§11.1、§12）。

### 8.5 不开放 TCP / Streamable HTTP

第一版**没有**任何网络监听：不绑定端口、不支持 `http://`/`https://` 端点、
不支持 MCP 的 Streamable HTTP 传输。

原因：TCP 监听意味着"局域网/本机任意网络上下文可达"，需要一整套认证（token 签发、
存储、轮换、来源校验、CORS/Origin、重放防护），与本地 IPC 完全不同的威胁模型。
把它塞进 A5 会让一个本可审计的本地通道变成半个网络服务。

后续若要做，必须**另行**设计并评审认证方案，且默认仍是关闭。本文件不为其背书。

---

## 9. 外部 Agent 兼容矩阵

**下表是设计目标，不是实测结果。** 截至 2026-09-20 的工作树，四个客户端仍全部未实测；
A5.6 已提供四份统一形状的配置夹具与契约测试（`test/client-fixtures.test.ts`），但每份夹具都明确
标记 `verified: false`，因此**没有任何真实客户端兼容性验证结论**。

| 客户端 | 接入方式 | 配置形态 | 状态 |
| --- | --- | --- | --- |
| Codex | MCP stdio | `command` + `args` + `env.HAVEN_MCP_ENDPOINT` | 设计目标；**未实测** |
| Claude Code | MCP stdio | 同上 | 设计目标；**未实测** |
| DSH | MCP stdio | 同上 | 设计目标；**未实测** |
| Pi | MCP stdio | 同上 | 设计目标；**未实测** |

四者共用 Haven UI 提供的**同一份配置模板**（§2）。仓库中的四份夹具只把客户端身份作为
测试数据，不能成为权限来源；共同不变量由共用模板和 `test/client-fixtures.test.ts` 覆盖：

1. 只暴露冻结的 9 个工具，不因客户端而增减；
2. 输入 schema 一律 strict，未知字段被拒绝；
3. 响应脱敏、有界、文本与 `structuredContent` 同源；
4. 提案只创建 pending，`digest` 原样返回；
5. 不存在任何 apply/approve/get_proposal 入口。

差异只应出现在"配置写在哪、字段叫什么"，且那属于各客户端的文档，不属于本仓库的契约。

---

## 10. 威胁模型

| # | 威胁 | 影响 | 缓解 | 残留风险 |
| --- | --- | --- | --- | --- |
| T1 | 其它用户连接 Broker | 读到设置快照 | OS ACL（DACL / `0600`） | 低 |
| T2 | **同用户恶意进程**连接 | 读快照 + 塞垃圾提案 | 只能做这两件事；配额（§8.3）+ 24h TTL | **中，已知且接受**（§8.2） |
| T3 | Agent 试图 Apply | 未授权改设置 | Apply 帧不存在；写入必须 UI 批准 | 低 |
| T4 | Agent 伪造 `clientInfo` 冒充可信客户端 | 提权 | `clientInfo` 不参与授权与身份（§6） | 无 |
| T5 | 恶意 client 发超大帧 / 海量请求 | 拖垮 Haven | 帧/请求/响应上限、在途上限、连接数上限、超时 | 低 |
| T6 | 协议版本错配导致行为错乱 | 误判能力 | 握手严格相等；不等即拒；不降级 | 低 |
| T7 | 端点不可达（Haven 未开/未运行） | 功能不可用 | 稳定错误 + UI 明示端点与模板 | 低 |
| T8 | 第二实例抢占端点 | 劫持通道 | fail closed，不接管；`HAVEN_BROKER_ENDPOINT_BUSY` | 低 |
| T9 | 提案洪水污染待审列表 | 用户疲劳/误批 | 三重配额 + 24h TTL + UI 显示来源 session | 中 |
| T10 | 客户端崩溃留下在途请求 | 资源泄漏 | 断连即清理；空闲超时 | 低 |
| T11 | 通过错误文案泄漏路径/凭据 | 信息泄漏 | `message` 复用既有脱敏与有界规则 | 低 |
| T12 | 未来误开 TCP | 网络暴露 | §8.5 明确禁止；需另行评审 | 无（当前） |
| T13 | 把端点名的随机性当成安全边界 | 误判防护强度 | §4.2 明确"端点名不是秘密" | 低 |
| T14 | 配额被重启清零后反复灌水 | 噪音 | 接受（不构成提权）；§8.3 记录理由 | 低，已知 |

---

## 11. 验收矩阵

A5 完成的定义是下列全部可执行、可重复：

| # | 验收项 | 手段 |
| --- | --- | --- |
| V1 | Broker `granted_requests` 集合**恰好 8 项**（6 Read + 2 Propose）；`cancel` 仅是控制帧 | 代码扫描 + 协议 fixtures 双向断言 |
| V2 | 请求集合是 Application service 面的**真子集**，且 `get_proposal`/approve/reject/receipt **不存在** | 对照 `AgentSettingsIpcService` / `AgentContextQueryService` 方法表 |
| V3 | Windows Named Pipe DACL **只**含创建者用户 SID（不含 SYSTEM） | 真实建管道后读回安全描述符并断言 |
| V4 | Unix socket `0600` + 目录 `0700`，且无"先建后 chmod"窗口 | `stat` 断言 + 时序断言 |
| V5 | 默认关闭：未开启用户开关时端点不存在、UI 不显示端点 | 进程级断言 |
| V6 | 握手：版本不等 → `HAVEN_BROKER_PROTOCOL_MISMATCH` 且断连；不降级 | 协议 fixtures |
| V7 | 五条帧级上限逐条触发（含恰好等于上限） | 边界 fixtures |
| V8 | 取消：未知 id 幂等；断连视为取消；已落库提案不回滚 | 协议 fixtures |
| V9 | 超时：Broker 20 s 先于 MCP 15 s 触发，错误码归因正确 | 注入延迟 |
| V10 | Haven 退出 → MCP 返回 `HAVEN_BRIDGE_UNAVAILABLE` 且 `retryable=true`；`get_system_capabilities` 仍可用 | 端到端 |
| V11 | 端点格式校验：非平台形态 / 带 scheme / 相对路径 / 超长 一律 `HAVEN_MCP_ENDPOINT_INVALID`，且**不尝试连接** | 单元 + 负向 fixtures |
| V12 | 多实例：第二实例 fail closed，不接管端点，报 `HAVEN_BROKER_ENDPOINT_BUSY` | 双进程 |
| V13 | 能力权威：`welcome` 报 false 的能力不可用；**每请求**重新校验 | 协议 fixtures |
| V14 | `clientInfo` 不影响任何行为，也不成为领域身份（改名/伪造后逐位相同） | 差分测试 |
| V15 | 四客户端 fixtures：同一份模板与工具清单在四个配置下产出同一结果 | A5.6 |
| V16 | 脱敏与有界在 Broker 层同样成立（错误 `message` 亦如此） | 泄漏 fixtures |
| V17 | 源码守卫按 §8.4 调整：`node:net` 仅允许 `connect`，无 `createServer`/`listen`；http/https/dgram/tls/child_process 与 fs 仍整条禁止 | 源码扫描 |
| V18 | 配额三项（8 / 8 / 32）逐项触发，且**在进入 Application 之前**拒绝、零写入 | A5.7 fixtures |
| V19 | 身份服务端生成：`create_proposal` 帧不含 `session_id`/`request_id`；传入 Application 的值由 Broker 生成；同连接多次调用共享同一 session | A5.4 fixtures |
| V20 | 端点每用户稳定：同一用户下 Haven 重启后端点字符串不变，旧配置仍可用 | 双次启动 |
| V21 | **无发现文件**：Node 侧不存在任何端点文件读取路径（源码扫描 + 行为断言） | A5.5 |

Unix stale socket 的附加验收约束：Haven 管理的运行目录组件出现符号链接、最终目录不是当前用户拥有的目录、
或端点路径被普通文件/目录/符号链接占用时，必须 fail closed，不 chmod 或删除无关对象；只有明确
`ECONNREFUSED` / `ENOENT` 且 `lstat` 确认是 socket 时，才允许删除固定端点并最多重绑一次。

### 11.1 当前核对（2026-09-20 工作树）

**"验收项存在"不等于"已验收"。** 逐条现状：

已由测试钉住的部分：

- **协议 / 会话**：8 项 granted requests 且禁帧到不了 Application（V1/V2）、握手版本不等即断连
  且不降级（V6）、帧/请求/响应上限与未知帧码（V7 的多数）、取消幂等与断连清理（V8）、
  `clientInfo` 不影响身份与能力（V14）——`agent_broker/{protocol,session}.rs` 单测。
- **身份服务端生成**（V19）：帧里带 `session_id` / `request_id` 被拒，同连接共享 session。
- **配额**（V18）：每连接 pending 与进程窗口两项都在**进入 Application 之前**拒绝，
  零写入；在途 8 一并由会话单测覆盖。
- **Windows 命名管道**（V3 / V12 的 Windows 侧）：真实管道往返 + DACL 读回断言只含当前用户、
  第二次 bind 报 `HAVEN_BROKER_ENDPOINT_BUSY`（该用例在首个 bind 拿不到端点时会提前返回，
  见 §7.4）。
- **源码守卫**（V17）：已按 §8.4 调整（`node:net` 仅 `connect`，无 `createServer`/`listen`）。
- **端点形态校验**（V11 的 Node 侧）：闭合形态单测，含"非法端点不尝试连接"（返回
  `HAVEN_MCP_ENDPOINT_INVALID`）。
- **握手、写出与空闲超时**（§7.1）：三个窗口都由内部函数参数注入毫秒级值，用例分别证明
  "连上不发首帧"、"发完首帧不再读取"（后者用小容量 duplex 把 welcome 写端堵住）与
  "握手完成后不再发请求"三种客户端都会让连接任务在短时间内自己收尾。空闲用例还带一段
  **对照**：同样的客户端行为在生产默认 idle 下不会在观测窗口内被关闭，因此关闭能归因到
  idle 定时器本身。三个用例只用内存 duplex，**不依赖 Named Pipe / Unix socket**。

**尚未验收**：

- V4（Unix `0600`/`0700`）：本机是 Windows，`#[cfg(unix)]` 路径**本机仍未编译、未运行**。
  Public CI 已新增 `tauri-unix` 的 Linux all-targets 作业来编译并运行这些用例，但**该作业
  尚未在 GitHub 上运行过**，因此不得据此声称 V4 已验收。
- V5（默认关闭的**进程级**断言）：默认关闭已实现并有单测，但没有进程级端到端断言。
- V9（Broker 20 s 先于 MCP 15 s）：两个常量与各自的超时路径都在，但**没有**验证两者先后
  关系的用例，也没有注入延迟的真实双进程测试。
- V10（Haven 退出 → MCP 归因）：需要真实双进程，未做。
- V11 的真实客户端路径：只在 Node 单测里成立，未经客户端复核。
- V15（四客户端夹具）：✅ 四份 `verified: false` 配置夹具与契约测试已落地；❌ 四个真实客户端仍未实测，
  因而不构成兼容性验收。
- V16（Broker 层脱敏/有界）：MCP 层有泄漏夹具，Broker 层未单独做泄漏 fixtures。
- V20（端点跨重启稳定）：需两次真实启动对比，未做。
- V21 的行为断言部分：源码扫描已由守卫覆盖，行为断言未做。
- Unix stale socket 的新增代码级断言（运行目录 symlink 拒绝、非 socket 归因、锁文件与 socket
  类型保护、最多一次重绑）已写入 `listener.rs`；由于本机是 Windows，这些 `#[cfg(unix)]`
  用例**本机仍未编译、未运行**。CI 已添加 Linux all-targets 作业来覆盖它们，但该作业尚未在
  GitHub 上运行过，因此**仍不能替代 Linux/macOS 实机门禁**。

**真机与真实客户端全部未做**：Windows WebView2 上的设置页界面、Codex / Claude Code / DSH /
Pi 四个真实客户端、以及 Haven→Node→MCP 的真实端到端（现有 Node 连接用例使用进程内假
transport）。**单元与集成测试通过不等于生产实测。**

---

## 12. 实现状态与已知缺口

**截至 2026-09-20 的工作树：A5 的 Rust 侧、Tauri 接线、设置页 UI、Node live 适配器与
9 个工具的后端路径都已落地并有测试；但真实客户端、真机 UI 与 Haven→Node→MCP 的真实
链路都还没有验收。**

仓库现状：

| 层 | 状态 |
| --- | --- |
| MCP 协议面（9 工具 / schema / 注解 / 错误语义） | ✅ 已实现（A3） |
| Typed Haven Agent Bridge **端口**（TypeScript 接口 + 不可用适配器） | ✅ 已实现（A3） |
| 行为协议 Skill | ✅ 已实现（A4） |
| **Broker 协议 / schema / 会话状态机** | ✅ 已实现（A5.1）：`src-tauri/src/agent_broker/{protocol,session,error,endpoint}.rs`；`agent_broker` 定向与全量 `cargo test` 均通过 |
| **Windows Named Pipe 服务端** | ✅ 已实现（A5.2）：`CreateNamedPipeW` + 只含当前用户 SID 的 DACL + `FILE_FLAG_FIRST_PIPE_INSTANCE`；真实管道往返与 DACL 读回断言已通过 |
| **Unix domain socket 服务端** | ⚠️ 已实现，但**本机仍未编译验证**（A5.3）：本机是 Windows，`#[cfg(unix)]` 代码路径在本机从未被编译或运行。已为它加上 Linux `--all-targets` 的 CI 作业（`tauri-unix`，见下表最后一行），但**该作业尚未在 GitHub 上运行过**，因此仍不得据此声称 Unix 侧可用。实现含每用户活实例锁（`flock`，锁文件 `0600`）与崩溃残留恢复（§7.3、§7.4）——对应用例已写进 `listener.rs` 的 `unix_endpoint` 一组，**同样从未运行** |
| **Rust Application bridge（Broker → Context / Settings services）** | ✅ 已实现（A5.4/A6）：`AgentSettingsBrokerApi`；8 个 granted requests 均有 dispatch、身份由服务端生成、配额前置；均有单测 |
| **Agent 资源 Patch / Receipt 安全语义** | ✅ 已实现：资源 Proposal 保存部分 `AgentResourcePreferencePatch`，批准时在同一 CAS UoW 内重读并合并；所有 Agent Receipt 写入路径使用统一脱敏审计投影，authoritative 值不被改写 |
| **Tauri 命令 / AppState 接线** | ✅ 已实现：`AppState` 持有 `AgentBrokerManager`，`agent_broker_status` / `agent_broker_enable` / `agent_broker_disable` 已注册，`command-manifest.rs` 与生成的权限 / capability 已接线 |
| **设置页 UI（开启 / 端点 / 模板）** | ✅ 已实现：设置页「外部 Agent 接入」分组，默认关闭、状态与启停、真实端点复制、四客户端共用模板；浏览器 Mock 只显示 `mock://` 预览（不可复制、不给模板）。**真实 Windows WebView2 上的该界面尚未验收**；设计里的「测试端点」按钮当前**不存在**，待补 |
| **Node local bridge（MCP server → Broker）** | ✅ 已实现（A5.5）：`src/local-broker.ts` 的 `LiveHavenAgentBridge`。`HAVEN_MCP_BRIDGE=live` **且** `HAVEN_MCP_ENDPOINT` 形态合法时连接 Rust Broker；默认仍是 `unavailable`；未配置或形态非法一律 fail closed；不读文件系统 / SQLite / secret。`mcp/haven-mcp` 本轮 `npm run build && npm test` = **88 passed / 1 skipped** |
| **四客户端 fixtures** | ✅ **已实现（A5.6）**：`fixtures/clients/{codex,claude-code,dsh,pi}.json` 共用同一 server 形状，契约测试校验身份、工具清单与 `verified: false`；❌ 真实客户端兼容性仍未验收 |
| **安全：默认关闭 / DACL / 配额** | ✅ 已实现（A5.7 核心）：默认关闭有单测；配额 8 / 8 / 32 在进入 Application 之前逐项拒绝；端点与模板 UI 已落地；「测试端点」按钮待补 |
| **生命周期：取消 / 断线 / 超时 / fail closed** | ✅ 已实现（A5.8 核心）：取消幂等、断连清理、空闲连接上限、请求 20 s 超时、握手 5 s 与单次写出 5 s 超时、停机、多实例 fail closed 均有单测；握手、写出与空闲三个窗口都作为内部函数参数注入，用例用毫秒级值证明连接任务会自己收尾。空闲超时（30 min）已有**毫秒级专门用例**（含"生产默认下不提前关闭"的对照），但它与其余用例一样只跑内存 duplex，**不构成真实双进程或平台端点上的验证** |
| **Public CI 的 Linux 与 MCP 作业** | ✅ 已添加（尚未运行）：`tauri-unix` 用 `--all-targets` 编译并运行 `#[cfg(unix)]` 用例，补齐了 CodeQL 对照所需的 `pkg-config` / `libwayland-dev`，并在 Rust format/clippy/test 之前用根 `.node-version` 安装 Node、在前端工作目录 `npm ci && npm run build` 以产出被嵌入的 `../前端/app/dist`（不安装任何本机 Linux Rust target）；`mcp/*` 单列为 `mcp` scope 与 `mcp` 作业（`npm ci --no-audit --no-fund` → `npm run build` → `npm test`），`full=true`（含 `.github/*`、依赖与构建策略改动）时同样触发，并已接入 `pr-gate` 的 `needs` 与 `require_if_selected`。**这两个作业都尚未在 GitHub 上运行过**，因此不构成任何验收证据 |

**默认仍是诚实不可用**：未设 `HAVEN_MCP_BRIDGE`（或设为 `unavailable`）时，
`haven-mcp-server` 连到 `UnavailableHavenAgentBridge`，`bridge.available = false`，
工具返回 `HAVEN_BRIDGE_UNAVAILABLE` / `HAVEN_CAPABILITY_UNAVAILABLE`。
只有用户显式开启 Broker、并在客户端配置 `HAVEN_MCP_BRIDGE=live` + 端点之后才会走真实通道；
**而这条通道从未验收过**。**在任何情况下都不得对外描述为"已接通 MCP"。**

落地过程中被实测推翻的两条设计假设（都已按实测结果修正）：

1. **"先建 pipe 再 `SetSecurityInfo` 收紧 DACL"不可行。**
   `PIPE_ACCESS_DUPLEX` 创建的句柄不含 `WRITE_DAC`，`SetSecurityInfo` 以
   `ERROR_ACCESS_DENIED(5)` 失败。改为 `CreateNamedPipeW` +
   `SECURITY_ATTRIBUTES`，顺带消除了"默认描述符窗口"。
2. **`PIPE_UNLIMITED_INSTANCES` 不足以让端点每用户唯一。**
   第二个进程可以用同一个名字再建实例，于是两个 Haven 会同时对外提供 Broker。
   必须加 `FILE_FLAG_FIRST_PIPE_INSTANCE`，已存在同名 pipe 时**首实例**创建失败并归类为
   `HAVEN_BROKER_ENDPOINT_BUSY`（后续实例不带该标志，其 `ERROR_ACCESS_DENIED` 归为
   `HAVEN_BROKER_ENDPOINT_UNAVAILABLE`，见 §7.4）。这条是测试先失败后才发现的设计缺口。

进一步，**Broker 是命名边界**：对外 snake_case（MCP 约定），对内要交给
`PreferenceReadingPatchDto`（camelCase + `deny_unknown_fields`）。映射在
`session.rs` 里显式列出，而不是写通用键名转换器——映射表可审计。

**`available = true` 不等于"已验证连通"。** live 适配器把 `available` 定义为"端点形态通过
§4.3 校验"，真实连接状态要到调用时才确认（`statusDetail` 本身就是这么写的）。因此
`get_system_capabilities` 报 `haven.available = true` 时，仍可能因为 Broker 没开、Haven 没
运行或 ACL 拒绝而在下一次调用上拿到 `HAVEN_BRIDGE_UNAVAILABLE`。

已知缺口（本设计不解决，登记在此）：

- 同用户恶意进程无法区分（§8.2）——需要独立安全评审才能推进为可写通道。
- 配额是进程窗口计数，重启清零（§8.3）——接受为残留风险，理由见该节。
- 9 个工具的后端路径已经实现（A6）；但真实 Haven→Node→MCP 链路仍未验收，
  因此"后端路径存在"不等于"整个 MCP 已完成"。
- 四客户端（Codex / Claude Code / DSH / Pi）全部未实测；A5.6 的四份配置夹具与契约测试已经落地，
  但 `verified: false` 只表示配置合同存在，不代表真实客户端兼容性通过（§9）。
- Unix 侧 `#[cfg(unix)]` 在**本机**未编译、未运行（本机 Windows）；CI 已添加 Linux
  all-targets 作业，但该作业尚未在 GitHub 上运行过，不得据此声称已编译验证。
- 真实 Windows WebView2 界面未验收；「测试端点」按钮未实现。
- Haven→Node→MCP 的真实端到端未验收：现有 Node 连接用例使用进程内假 transport，不是真的
  Rust Broker。
- 空闲超时（30 min）已有毫秒级专门用例（§7.1），但它只用内存 duplex：**真实双进程、
  Unix socket / Named Pipe 端点上的空闲断连仍未验证**。
- `get_proposal` 不在 v1；若将来要加，须同时改冻结端口与冻结工具清单并单独评审（§5.3）。
- 多实例同时提供 Broker **不支持**；第二实例 fail closed（§7.4）。
