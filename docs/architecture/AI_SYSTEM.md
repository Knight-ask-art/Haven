# Haven AI 系统架构（AI Provider / MCP / Skill 边界）

本文件是 AI 能力在栖阅（Haven）中的**安全边界与分层契约**。它描述的是"什么是允许的"，
不是"未来可能做什么"：任何与本文件冲突的实现都属于缺陷，必须先改本文件再改代码。

相关文档：

- 阶段计划：[`docs/plans/2026-09-18-ai-provider-mcp-skill-plan.md`](../plans/2026-09-18-ai-provider-mcp-skill-plan.md)
- 外部 Agent 传输设计（A5，**设计已冻结；核心已落地，真实客户端与端到端未验收**）：[`MCP_EXTERNAL_AGENT_TRANSPORT.md`](./MCP_EXTERNAL_AGENT_TRANSPORT.md)
- 既有 Proposal 内核：`haven-domain/src/setting_proposal.rs`、`haven-application/src/services/setting_proposals.rs`
- 既有 Typed IPC 切片：`haven-application/src/services/agent_settings_ipc.rs`、`src-tauri/src/commands/agent.rs`
- 凭据契约：`haven-domain/src/credential.rs`（ADR-001，Windows Credential Manager）
- 出站 HTTP 约束：`haven-common/src/network.rs`、`haven-infrastructure/src/http_security.rs`

---

## 0. 一句话结论

> **AI Provider 只能生成结构化建议与上下文；唯一能改变本机状态的路径是
> 「User 在 UI 上批准一份 Rust 生成的 canonical digest」。**

Provider 不是执行者。模型输出不是权限。MCP 不是写入通道。Skill 不是能力提升。
任何"让模型直接改设置 / 直接调 IPC / 直接写 SQLite"的设计都违反本文件。

---

## 1. 已落地的链路（A1 设置智能体，现状）

当前仓库中已经存在并在运行的一条闭环，AI Provider 切片必须复用它、不得另起一条：

```text
① 读取上下文
   React UI (AiAssistantDialog)
     → feature hook / AiAssistant 客户端
     → HavenClient.agentSettingsContextGet()
     → Tauri command `agent_settings_context_get`
     → AgentSettingsIpcService
     → SettingsService（authoritative 读取）
   ← AgentSettingsContextDto { contextId, contextHash, revision, 脱敏快照, capabilities }

② 生成提案（Provider 的唯一产出）
   AgentSettingsProposalCreateRequest { sessionId, requestId, contextId, contextHash, baseRevision, patch }
     → Tauri command `agent_settings_proposal_create`
     → AgentSettingsIpcService（重新读取 authoritative 设置、重建上下文、逐项比对）
     → SettingProposal::new(...)
     → canonical JSON（对象键按 UTF-8 字节序升序、紧凑输出、UTF-8 直出）
     → SHA-256 → digest（64 位小写十六进制）
     → SQLite setting_proposals（payload_json + digest + status=pending）

③ UI Diff
   AgentSettingsProposalDto { proposalId, digest, status, subject, targetLabel, baseRevision,
                               createdAt, expiresAt, changes[] }
     → UI 逐项渲染 key / before / after，并**逐字显示** digest
       （本地预览摘要必须标注"本地预览"，不得当作领域 digest）

④ 用户批准
   AgentSettingsProposalApproveRequest { proposalId, expectedDigest }
     → Tauri command `agent_settings_proposal_approve`
     → 单一 UoW（BEGIN IMMEDIATE）：
         digest 比对 + 目标 CAS + 一次性 Approval Token 签发/消费 + 设置写入 + 回执写入
         + Proposal/Binding 状态迁移

⑤ Receipt
   AgentSettingChangeReceiptDto { receiptId, proposalId, proposalDigest, status,
                                  appliedRevision, changed, changes[], appliedAt }
     → 可独立重放、可回读（`agent_setting_change_receipt_get`）
```

关键不变量（已实现，AI 切片必须继承）：

1. **digest 只由 Rust 生成**。UI 不计算领域 digest，只回传它显示的那一份。
2. **只有批准会写设置**。创建/拒绝/过期都是零写入。
3. **旧快照 fail-closed**。`contextId` / `contextHash` / `baseRevision` 任一不匹配即拒绝。
4. **回执是审计事实**。删除目标不会删除提案与回执；回执必须自洽（`changed` 与 before/after 一致）。
5. **token / secret / 路径 / 正文不进 wire**。DSO 只暴露 code / message / retryable。

---

## 2. 分层与依赖方向（强制）

```text
React UI
  → Feature Hook / Action        （features/<domain>/lib/use*.ts）
  → Feature Gateway              （features/<domain>/ipc/*-gateway.ts，运行时形状守卫）
  → Typed HavenClient            （lib/ipc/client.ts 接口）
      ├─ TauriClient             （lib/ipc/tauri-client.ts → invoke）
      └─ MockClient              （lib/ipc/mock-client.ts → contracts/ipc/v1/fixtures）
  → Tauri command                （src-tauri/src/commands/*.rs，只做解析 + 调用 + 错误映射）
  → Application service          （haven-application/src/services/*.rs，唯一用例编排点）
  → Domain / Port                （haven-domain 契约，无框架依赖）
  → Infrastructure               （haven-infrastructure：SQLite / HTTP / keyring）
```

**禁止**：

- 组件直接 `invoke(...)`、直接读 SQLite、直接 `fetch` 任何 Provider。
- Provider 适配器从 React 或 IPC 层被绕过 Application 调用。
- 引入"自由调用"入口（例如 `agent_invoke`、`ai_complete`、`ai_chat` 这类把任意
  提示词/任意工具直接转发给模型的命令）。
- 自由 JSON 穿透 IPC（`serde_json::Value` 作为请求或响应载荷）。

**唯一例外**：还没有 Application 服务的**纯读**能力可以在基础设施层直接实现，
但必须经由 Application service 暴露，命令层不得直接持有基础设施类型。

---

## 3. Provider 契约

### 3.1 Provider 能做什么

| 允许 | 说明 |
| --- | --- |
| 生成**结构化建议** | 例如一份阅读排版 patch，字段落在闭合 DTO 上 |
| 生成**上下文解释** | 对已脱敏快照的自然语言说明，不参与写入 |
| 读取**已脱敏上下文** | 只能看到服务端裁剪后的快照与能力清单 |

### 3.2 Provider 绝对不能做什么

| 禁止 | 理由 |
| --- | --- |
| Apply / 写设置 | 写入只认用户批准的 digest |
| 读取 secret / API key | 凭据只存在于 CredentialStore |
| 读绝对路径 / 目录树 | 扫描与路径事实不出 Rust |
| 直接 HTTP 绕过 Application | 出站请求必须由 Infrastructure 适配器发起 |
| 决定自己的能力 | 能力清单是服务端固定投影，不是调用方参数 |
| 伪造模型或能力 | 模型目录必须来自 Provider `/models`；缺字段即 `unknown` |

### 3.3 Allowed / Forbidden 速查

```text
ALLOWED
  read: 脱敏设置快照、revision、能力清单、模型目录（id/显示名/created/ownedBy/显式能力）
  write: 创建 Proposal（一条 pending 行）
  never: 直接写入任何 authoritative 状态

FORBIDDEN
  read:  API key 原文、credentialRef / target 名、绝对路径、正文、Cookie、请求头
  write: 设置、文件、数据库行、provenance、receipt、MCP 返回
  call:  Tauri IPC、SQLite、文件系统、任意 URL（只能走 profile.endpoint 的兼容 /models）
```

---

## 4. 凭据边界（唯一事实源）

- 统一使用 `CredentialRef::new_scoped("ai", profile_id)`，形成 `haven:ai:<profile-id>`。
  该 target 的字符约束由 `haven-domain/src/credential.rs` 强制
  （拒绝控制字符、冒号、反斜杠、空白；单段 ≤ 60，总长 ≤ 128）。
- 凭据 Provider 枚举 `CredentialProviderDto` 扩展 `ai` 变体，**只**用于受控的
  credential status / set / delete；任何响应都不得回显 secret。
- **API key 绝不进入**：普通 settings JSON、SQLite profile 行、TS wire、
  localStorage、日志、provenance、receipt、MCP 返回、错误消息。
- 读取路径：`CredentialStore::get()` → `SecretString`（`Debug`/`Display` 均为
  `[REDACTED]`，`Drop` 清零，不实现 `Clone`）→ 只在 Provider 适配器调用栈内
  `expose()`，随即随栈释放。
- **profile 删除的凭据清理语义**：调用方携带的 `expectedRevision` **先**与刚读到的版本
  比对（零副作用地拒绝过期的删除请求），通过之后才对凭据执行受控 `delete`，最后对
  profile 行做 CAS 删除。顺序不可颠倒：
  - 若不先做版本比对，一次"UI 拿着旧列表点删除"就会先销毁 API Key、再返回冲突——
    操作报告失败，却已经做完了不可逆的破坏。
  - 若先删行再删凭据，凭据删除失败会留下**没有任何行引用的孤儿 secret**——用户再也
    无法从 UI 管理它。
  - 剩下唯一无法用单条 SQL 消除的窗口是"读之后、CAS 之前被并发写者推进版本"。该窗口
    内凭据已删而 CAS 失败，留下的是一个"存在但未配置密钥"的 profile：状态可见、
    可重新配置、可再次删除。这是可恢复的那一侧。
  - 凭据删除失败（keyring 不可用等）必须**中止整次删除**并把错误返回 UI，不得静默跳过。

---

## 5. MCP 边界

MCP 只允许两类工具：

| 允许 | 语义 |
| --- | --- |
| `Read` | 读取已脱敏、已投影的上下文（与 `agent_settings_context_get` 同一份 DTO） |
| `Propose` | 创建一份 pending Proposal（与 `agent_settings_proposal_create` 同一条路径） |

**禁止**：`Apply`、`Approve`、`Reject`、`Write`、`Delete`、`metadata_patch`、`file_renames`、
任意文件系统工具、任意 shell、任意 SQL、任意 secret 读取、任意"直接把 prompt / 命令转发
给模型或系统"的工具。MCP 客户端拿不到比 WebView 更多的权限——它只是同一 API 的另一个
调用方。控制点不在 ACL（那是 WebView 的边界），而在**工具清单本身**：不允许的动词压根
没有工具。

### 5.1 冻结的工具清单（第一版，恰好 9 个）

```text
get_system_capabilities            propose_settings_patch
get_settings_snapshot              propose_resource_preference_patch
get_setting_sources
get_resource_preference_snapshot
get_library_summary
get_media_capabilities
get_onboarding_state
```

清单是闭合集合：`mcp/haven-mcp/src/constants.ts` 的 `TOOL_NAMES` 是唯一声明处，
`test/tools.test.ts` 断言真实注册结果与它完全相等，并对工具名跑禁用词根扫描。

### 5.2 链路（不可绕过）

```text
MCP client
  → MCP tool（Strict Zod 输入）
  → Typed Haven Agent Bridge 端口          ← MCP 侧唯一的出口
  → Haven Application service（Typed）
  → SettingProposal（canonical JSON + SHA-256 digest）
  → UI Approval（用户逐项核对 digest）
  → Rust CAS / Apply
  → Receipt
```

MCP server **不读** SQLite、**不碰**文件系统、**不访问** CredentialStore、**不**
invoke Tauri。它只调用桥接端口，端口只承载已经投影、已经脱敏的载荷。

### 5.3 传输与接通状态（截至 2026-09-18）

| 层 | 状态 |
| --- | --- |
| MCP 协议面（工具名 / schema / outputSchema / 注解 / 错误语义） | **已实现并冻结** |
| Typed Haven Agent Bridge 端口 | **已实现**（`mcp/haven-mcp/src/bridge.ts`） |
| 显式不可用适配器 | **已实现**，且是生产默认 |
| 本地传输适配器（端口 → 运行中的 Haven 进程） | **已实现**（`mcp/haven-mcp/src/local-broker.ts` 的 `LiveHavenAgentBridge`；Rust 侧 Broker、Tauri 命令与设置页「外部 Agent 接入」分组均已落地），但**默认关闭**，真实链路**未验收** |
| 9 个工具对应的 Haven 后端用例 | 3 个已实现，6 个未实现 |

运行时桥接的设计已冻结（[A5 传输设计](./MCP_EXTERNAL_AGENT_TRANSPORT.md)），A5.1–A5.5 与
A5.7/A5.8 的核心也已落地：Rust Broker、Tauri 命令、设置页分组与 Node live 适配器都在仓库里
并有测试。**但"有实现"不等于"已接通"**：默认关闭、默认不可用，四个真实客户端、真实 Windows
WebView2 界面与 Haven→Node→MCP 的端到端都**未验收**，A5.6 的四客户端夹具也未实现。
逐项证据与边界见该文件 §11.1 与 §12。

目标接入方是**用户已经装好的**外部 Agent（Codex、Claude Code、DSH、Pi …），
它们共用**同一套** 9 工具契约；客户端之间的差异只是 stdio 配置写法，
不存在任何客户端专属工具或特权模式。设置页已提供四客户端共用的**同一份**配置模板，但
四个客户端**均未实测**（A5.6 的四份客户端夹具与兼容验证都未做），见
[`MCP_EXTERNAL_AGENT_TRANSPORT.md`](./MCP_EXTERNAL_AGENT_TRANSPORT.md) §9 的兼容矩阵。

因此默认行为是**显式不可用**，而不是伪造接通：

- 默认（未设 `HAVEN_MCP_BRIDGE`）→ `UnavailableHavenAgentBridge`；
- `HAVEN_MCP_BRIDGE=live` **但未配置 `HAVEN_MCP_ENDPOINT`** → 仍是显式不可用桥接
  （说明缺哪个变量，不猜、不扫描、不回退）；
- `HAVEN_MCP_BRIDGE=live` 且端点形态合法 → 连接本地 Broker。**`available = true` 只表示
  端点形态通过校验，连接状态要到调用时才确认**（Broker 未开启或 Haven 未运行时，调用仍拿到
  `HAVEN_BRIDGE_UNAVAILABLE`）；
- `HAVEN_MCP_BRIDGE=live` 且端点形态非法 → `HAVEN_MCP_ENDPOINT_INVALID`，**不尝试连接**；
- `HAVEN_MCP_BRIDGE=fixture` → **拒绝启动**：测试替身需要环境变量**与**注入的工厂
  同时存在，而生产入口不注入工厂，因此假桥接不可能成为生产数据源。

两种不可用的含义必须区分开，`get_system_capabilities` 会如实报告：

- `HAVEN_BRIDGE_UNAVAILABLE`：桥接没接通，**换一个 Haven 版本可能就行**；
- `HAVEN_CAPABILITY_UNAVAILABLE`：Haven 后端根本没有这个用例，**重试无用**。

**未实现的 6 个工具在协议面上已冻结，但运行时不可用。** 这是刻意的：
协议冻结让客户端与后续实现有稳定的目标，而"返回编造的数据"会把一个尚未实现的能力
伪装成已完成。**live 通道接通后它们依然如此**——那 6 个用例属于 A6，与传输无关。

**外部 Agent 接入默认关闭。** 只有用户在设置页显式开启后才会创建本地端点；未开启时
`agent_broker_status` 返回 `disabled` 且不带端点，界面不给端点、不给模板。

---

## 6. Skill 边界

**Skill 是行为协议，不是权限。**

- Skill 描述"这件事按什么顺序做、产出什么形状"，它可以被模型、被模板、被人执行。
- Skill 不携带授权，不提升能力，不改变 ACL，不改变 CredentialStore 的可达性。
- 一个 Skill 无论怎么写，都不能让模型获得 `Apply`；它只能产出 Proposal。
- 因此 Skill 的评审标准是**正确性与可读性**，而权限评审标准在 Provider / MCP 边界上。

### 6.1 第一个正式 Skill：`skills/haven-agent-proposal`

它是上面这条边界的具体化：教模型**怎么用** MCP 的 9 个工具，而不是给模型更多工具。

协议要点（全文见 `skills/haven-agent-proposal/SKILL.md`）：

1. **永远先调 `get_system_capabilities`**，据此判断桥接状态、能力位与各工具是否 implemented。
   读不到就如实说读不到，不编造数据 / 模型 / 能力。
2. **先读后提议**。改全局阅读设置前必须 `get_settings_snapshot`，并把
   `context_id` / `context_hash` / `base_revision` **逐字**回传给提案工具。
3. **改全局只调 `propose_settings_patch`**；资源级改动只有在能力与工具都可用时才允许，
   否则明确告知不可用——不拿全局提案去近似资源级请求（那会改错范围）。
4. **提案只是 pending**。必须告诉用户 digest、逐项 changes、作用域、过期时间，并说明
   "尚未应用"；审批只能在 Haven UI 里由本人完成。用户说"直接应用"时**仍然拒绝绕过 UI**。
5. **不读不回显** API key / `credentialRef` / 绝对路径 / 文件内容 / SQL / Provider 原始响应；
   不做 `metadata_patch` / `file_renames`；不从模型名猜能力；MCP 断开时给安全的手动路径。
6. **不声称"已应用"**。本 Skill 没有读取回执的能力，因此它连"刚才那次批准成功了"都
   无法确认；在存在独立的 Receipt 读取事实之前，只描述到 `pending` 为止。

这条边界是**可执行的**，不是口号：`tools/skills/skill-contract-check.py` 会校验

- frontmatter 形状、`SKILL.md` 行数上限；
- `evals/evals.json` 可解析、`skill_name` 与 frontmatter 一致、eval 数量与字段完整；
- 技能包里出现的每个"像工具名"的标识符都必须在 MCP 冻结的 9 项里（清单**从
  `mcp/haven-mcp/src/constants.ts` 现读**，不另抄一份）；
- 白名单之外的工具名只允许出现在**禁止语境**里（该行含否定标记），
  因此"❌ 做 `metadata_patch`"合法，而"调用 `apply_settings`"会被抓住。

---

## 7. 出站 HTTP 安全约束（Provider 模型发现）

Provider 的模型发现是当前唯一由用户配置端点的出站请求，必须复用项目既有约束：

- **URL 语法**：只接受 `http` / `https`；拒绝 userinfo（`user:pass@`）、fragment、
  显式空端口、单标签主机（`localhost`、`media`、`*.local`、`*.internal`）、
  非公网字面 IP（回环、私网、链路本地、CGNAT、文档网段等）。
- **端口**：仅 `80 / 443 / 8080 / 8443`（外加 `film-tv-fixture` feature 下
  字面 `127.0.0.1` 的临时端口，仅编译期开启，仅测试用）。
- **DNS**：每次请求重新解析，任一解析结果非公网即整体拒绝（fail closed，不做"取第一条
  公网地址"的降级）；解析结果通过 `resolve_to_addrs` 固定到连接上。
- **重定向**：关闭。手动跟随的调用方必须对每一跳重新走完整策略。
- **不静默发现**：不做 mDNS / SSDP / 局域网扫描来"找" Provider；端点只能来自用户显式配置。

**已记录的决策（与需求的冲突点）**：

需求希望"沿用项目的 HTTP 安全约束，同时不能静默发现局域网目标"。当前共享策略
（`HttpUrlPolicy`）**同时拒绝**显式配置的回环/私网端点。这意味着第一版**不支持**
`http://127.0.0.1:11434/v1` 这类本机运行时端点。我们选择**保留现有安全边界**而不是
为本切片放宽它：

- 放宽需要新的 ADR（明确"用户显式配置的私网端点"这一类别、其提示 UI、以及它与 SSRF
  防护的关系），而不是在 AI 切片里顺手打开。
- 现有代码里唯一的回环例外是编译期 feature（`film-tv-fixture`），它是测试夹具，不是
  产品能力；复用它会让"测试路径"变成"生产后门"。
- 因此本切片新增 `HttpUrlPolicy::AiProviderEndpoint` 变体，语义与 `SourceEndpoint`
  一致，只是把上下文显式化，防止未来某个上下文被放宽时被另一个悄悄继承。

---

## 8. 模型与能力（诚实原则）

- 模型目录**只能**来自 Provider 的兼容 `/models` 响应。任何"内置候选模型名"都是伪造。
- **不通过模型名猜测能力**。`gpt-4o` / `*-vision` / `text-embedding-*` 这类名字
  **不**构成 vision / embedding / chat 的证据。
- 能力字段是三态闭合集合：`supported` / `unsupported` / `unknown`。第三方响应缺字段时
  一律 `unknown`，UI 显示"未声明"，不得显示为"不支持"或"支持"。
- endpoint 已带 `/v1` 时不重复拼接；已以 `/models` 结尾时不再追加。
- 未配置密钥 / profile 被禁用 / 目录为空 → 返回**空目录 + 明确 state**，不是错误，
  也不是假模型。
- 网络失败 / 非 2xx / 响应非法 → 返回**稳定可行动 ErrorDto**，不吞错、不伪造模型。

---

## 9. 验收门禁

任何 AI 相关改动合并前必须全部为真：

| # | 门禁 | 证据 |
| --- | --- | --- |
| G1 | Provider 不写入 | 代码搜索：Provider 适配器不持有 repository / UoW / command 类型 |
| G2 | 无自由调用入口 | `command-manifest.rs` 中不存在把任意 prompt/tool 直通的命令 |
| G3 | secret 不出 wire | wire 生成物与 `Reflect.ownKeys` 兜底守卫测试；fixture 序列化断言 |
| G4 | secret 只走 CredentialStore | `haven:ai:<profile-id>` 命名空间单测 + 普通存储/日志断言 |
| G5 | 命令清单单一事实源 | `capability_consistency.rs`：`TARGET_PERMISSIONS` == 生成 ACL == `capabilities/main.json` |
| G6 | wire 生成物无漂移 | `wire_bindings_consistency.rs` + `cargo run -p haven-application --example gen_wire_bindings` |
| G7 | 模型能力不猜 | 单测：只有名字、没有能力字段的模型 → `unknown` |
| G8 | 无可用模型诚实 | 单测 + Mock fixture：未配置/禁用/空目录 → 空目录 + `无可用模型` |
| G9 | profile 删除清理凭据 | 单测：删除后 `credential_status` 为未配置；凭据删除失败时中止 |
| G10 | 出站 URL 策略 | 单测：私网 / 单标签 / 非 http(s) / userinfo / 超范围端口全部拒绝 |
| G11 | CAS 有效 | 单测：过期 revision 更新返回冲突且零写入 |
| G12 | 文档同步 | 本文件与阶段计划描述的状态与代码一致 |
| G13 | MCP 无写入工具 | `mcp/haven-mcp/test/tools.test.ts`：注册结果 == 冻结 9 项；工具名禁用词根扫描 |
| G14 | MCP 桥接 fail-closed | `bridge-selection.test.ts` + `stdio-e2e.test.ts` + `local-broker.test.ts`：`fixture` 与未知取值拒绝启动；`live` 缺端点时显式不可用、端点非法时 `HAVEN_MCP_ENDPOINT_INVALID` 且**不尝试连接**；绝不静默降级成"已接通" |
| G15 | MCP 只读零写入 | 工具层与端口层各一轮：调用全部只读工具后写入计数为 0 |
| G16 | MCP 输出脱敏且有界 | 泄漏夹具 + 逐字段断言；单字段 512 / 列表 50 / 整份 24 000 字符上限 |
| G17 | MCP 文本与结构化同源 | json 格式 `JSON.parse(text)` 深度相等；markdown 覆盖全部标量 |
| G18 | MCP stdout 洁净 | 真实子进程端到端：stdout 每行都是 JSON-RPC，日志只在 stderr |
| G19 | Skill 不越权 | `tools/skills/skill-contract-check.py`：工具名只来自 MCP 冻结 9 项；白名单外的工具名仅允许出现在禁止语境；evals 与 frontmatter 一致；`SKILL.md` < 500 行 |

---

## 10. 阶段划分

| 阶段 | 内容 | 状态 |
| --- | --- | --- |
| **A1** | 设置智能体闭环：Proposal → canonical JSON → SHA-256 → Diff → 批准 → CAS/Apply → Receipt | **已落地** |
| **A2** | AI Provider Profile 基础切片：profile 持久化 + 凭据引用 + 模型发现 + Typed IPC + 设置页接线 | **已落地** |
| **A3** | MCP 协议面 + Typed Bridge 端口：9 个冻结工具、strict schema、脱敏/有界/同源响应、显式不可用桥接 | **已落地** |
| **A4** | 正式 Skill：`skills/haven-agent-proposal` 行为协议 + 契约校验与 evals | **已落地** |
| **A5** | MCP 运行时传输适配器：把端口接到运行中的 Haven（认证/来源、生命周期、能力协商）——设计已冻结，见 [`MCP_EXTERNAL_AGENT_TRANSPORT.md`](./MCP_EXTERNAL_AGENT_TRANSPORT.md)，拆分见计划 A5.1–A5.8 | **核心已落地，未验收**：Rust Broker（A5.1–A5.4）、Windows 命名管道（A5.2）、Tauri 接线与设置页「外部 Agent 接入」分组、Node live 适配器（A5.5）均已实现并有测试；A5.6 四客户端夹具未实现，Unix 侧未编译验证，四客户端 / 真机 UI / 端到端均未验收 |
| **A6** | 补齐 6 个未实现工具的 Haven 后端用例（library summary / media capabilities / onboarding / setting sources / 资源偏好读与提案） | 后续 |
| **A7** | Provider 生成 Proposal：把模型输出接进 A1 的提案路径（仍无 Apply 权限） | 后续 |

仍**不含**：Python sidecar、PydanticAI、完整 Agent runtime、MCP 的任何写入/审批工具、
`metadata_patch` / `file_renames`、任何自动 Apply，以及任何给 Skill 的额外权限。

**A3 + A4 + A5 合起来的边界必须说清楚**：MCP 的**协议面**、**桥接端口**与**行为协议 Skill**
都已完成并冻结；A5 的运行时传输适配器（Rust Broker + Tauri 接线 + 设置页分组 + Node live
适配器）也已实现，但它**默认关闭**，且**从未在真实客户端、真机或端到端链路上验收过**。
同时 **6 个工具的后端用例仍未实现**（A6），所以即使 live 通道接通，它们依然返回
`HAVEN_CAPABILITY_UNAVAILABLE`。结论：默认配置下 9 个工具里只有 `get_system_capabilities`
可用（它如实报告其余不可用）；**"链路上线"不等于"已接通 MCP"，任何文档都不得这样描述**。
Skill 的价值在于：在这些不可用的前提下，模型也会**如实说明**并给出安全的手动路径，
而不是编造数据或绕过审批。
