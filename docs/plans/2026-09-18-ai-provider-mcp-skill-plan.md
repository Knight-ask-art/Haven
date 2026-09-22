# AI Provider 基础切片 / MCP / Skill 阶段计划

- 日期：2026-09-18
- 分支：`codex/ai-provider-mcp-skill`
- 架构边界：[`docs/architecture/AI_SYSTEM.md`](../architecture/AI_SYSTEM.md)
- 前置阶段：`docs/plans/2026-09-17-agent-approval-token-plan.md`（A1 设置智能体闭环，已落地）

## 目标

为后续 MCP / Skill 提供**唯一的 Typed Application API**，先把 AI Provider 的地基铺好：

1. Provider Profile 的非敏感持久化（闭合集合、严格校验、CAS）。
2. 凭据统一走 `haven:ai:<profile-id>`，secret 不进任何可读存储。
3. 兼容 `/models` 的模型发现，能力字段三态、不猜。
4. 全链路 Typed IPC（Rust DTO → 生成 wire.ts → HavenClient → Tauri/Mock）。
5. 设置页 AI 占位接到真实 profile / 凭据状态 / 模型目录。
6. 补齐 9 个 MCP 工具的 Haven Application / Broker / Node live 后端路径。
7. 以 Structured Outputs 生成 typed 设置建议，并接入既有 pending Proposal 链路。
8. 提供不携带自由文本或敏感材料的 Agent Event / Trace typed 基础。

## 不做什么（明确的非目标）

- 不做 Python sidecar、PydanticAI、完整 Agent runtime。
- **不做 MCP 的写入/审批工具**（`apply` / `approve` / `write` / `delete` 等一律不存在）；
  MCP 只做只读上下文与创建 pending 提案。
- 不重新设计 MCP 运行时传输协议；本轮实现并补测既有冻结设计的本地 Broker / live 适配器，
  但不把单元测试通过写成真实客户端或端到端验收。
- 不做 `metadata_patch` / `file_renames`。
- 不做任何自动 Apply；Provider 依然只能产出结构化建议。
- 不在本轮继续接 AI 前端对话框、设置页聊天框或轨迹图；前端消费 typed 契约另行排期。
- 不引入自由 `agent_invoke` / `ai_chat` 之类的直通命令。
- 不引入第二套数据库连接或第二套 HTTP 栈。

## 不变量

1. **Provider 不能 Apply**。写入唯一入口仍是「用户批准的一份 Rust canonical digest」。
2. **API key 只能通过 CredentialStore**。`haven:ai:<profile-id>`，不进 settings JSON /
   SQLite profile 行 / TS wire / localStorage / 日志 / provenance / receipt / MCP 返回。
3. **profile 模型不含 secret**。`AiProviderProfile` 只有 id、显示名、kind、endpoint、
   enabled、selectedModelId 这些非敏感字段。
4. **endpoint 只能来自用户显式配置**，且必须通过共享 HTTP URL 策略；不打开任意 scheme，
   不静默发现局域网目标。
5. **能力不猜**。模型名不构成 vision / embedding / chat 的证据；缺字段即 `unknown`。
6. **无可用模型是诚实状态**。没有 profile、被禁用、没有密钥、目录为空 → 空目录 +
   明确 state；UI 显示「无可用模型」；永不写入或推断 `gpt-4o`。
7. **凭据清理有确定语义**。删除 profile 先删凭据、后 CAS 删行；凭据删除失败即中止。
8. **命令清单单一事实源**。新命令只加在 `src-tauri/command-manifest.rs`，ACL 与
   capability 由同一份声明派生。
9. **wire.ts 只能生成**。`cargo run -p haven-application --example gen_wire_bindings`。

## 实现范围

### Domain（`haven-domain`）

- `AiProviderKind`：闭合枚举，第一版只有 `openai_compatible`。
- `AiProviderProfile`：`profile_id` / `display_name` / `kind` / `endpoint` / `enabled` /
  `selected_model_id` / `revision` / `created_at` / `updated_at`。
- 严格输入校验：控制字符、本地路径、绝对路径、凭据形状（`user:pass@`、`scheme://`）、
  query / fragment 一律拒绝。
- `AiModelDescriptor` + `AiModelCapability`（`supported` / `unsupported` / `unknown`）。
- 契约 `AiProviderProfileRepository`：`list` / `get` / `cas_upsert` / `cas_delete`。

### Common（`haven-common`）

- `HttpUrlPolicy::AiProviderEndpoint`：与 `SourceEndpoint` 同语义，上下文显式化。

### Application（`haven-application`）

- `AiProviderProfileService`：`list` / `get` / `upsert` / `delete` / `models_catalog` / typed
  `generate_settings_recommendation`。
- `AiModelCatalogPort`：出站模型发现的端口；实现方负责 URL / 重定向 / 响应校验。
- `AiSettingsRecommendationPort`：只返回 `AiSettingsRecommendation`，不创建 Proposal。
- `AgentContextQueryService`：设置来源、资源偏好、媒体库摘要、媒体能力、引导状态和资源提案。
- Agent 资源提案使用 `AgentResourcePreferencePatch` 保存部分 `PreferenceData`；批准时在
  同一 UoW 内重读 authoritative 偏好、合并 patch、校验 `base_revision` 并 CAS。Agent Receipt
  的 before/after 是统一脱敏的审计投影，不能把已有路径/endpoint/凭据样式文本复制给外部 Agent。
- `AgentTracePort`：闭合事件、上下文绑定、服务端序号与会话上限。
- Wire DTO（`wire/dto.rs`）并在 `wire/generate.rs` 注册（生成物唯一来源）。

### Infrastructure（`haven-infrastructure`）

- 迁移 `045_ai_provider_profiles.sql`：幂等、`CREATE TABLE IF NOT EXISTS`、不动既有表。
- `SqliteAiProviderProfileRepository`：CAS 写与 CAS 删。
- `OpenAiCompatibleModelCatalog`：兼容 `/models`，DNS 固定、关闭重定向、有界响应体。

### Tauri

- 命令：`ai_provider_profile_list` / `_get` / `_upsert` / `_delete`、`ai_provider_models_list`。
- 命令只调用 Application service；网络只由 Infrastructure 适配器发起。

### 前端（已有接线；本轮不扩展 AI UI）

- `settings/ipc/ai-provider-gateway.ts`：运行时形状守卫 + 错误归一。
- `settings/lib/useAiProviderSettings.ts`：Hook（真实 profile、凭据 configured、模型目录）。
- 设置页 AI 区块接线：保留栖阅视觉与对话/轨迹布局，不做视觉重设计。
- API key 单向提交，回显「已配置 / 未配置」，不回填原文。

## 验收门禁

见 `AI_SYSTEM.md` §9 的 G1–G19。补充本切片的具体命令：

```text
cargo fmt --manifest-path 后端/Cargo.toml --all -- --check
cargo test  --manifest-path 后端/Cargo.toml -p haven-domain
cargo test  --manifest-path 后端/Cargo.toml -p haven-application
cargo test  --manifest-path 后端/Cargo.toml -p haven-infrastructure
cargo test  --manifest-path src-tauri/Cargo.toml
cargo run   --manifest-path 后端/Cargo.toml -p haven-application --example gen_wire_bindings
cd 前端/app && npm run test
cd 前端/app && npm run fixtures:check
cd mcp/haven-mcp && npm run typecheck
cd mcp/haven-mcp && npm run build && npm test
python tools/skills/skill-contract-check.py
python tools/skills/skill-contract-check.test.py
git diff --check
```

`mcp/haven-mcp` 的门禁顺序是 **build 先于 test**：stdio 端到端用例需要一个已构建的
`dist/`，缺失时会跳过（并在测试名里说明），因此只有这个顺序才是完整门禁。

Public CI（`.github/workflows/ci.yml`）现在把 `mcp/*` 单列成一个 scope 与 `mcp` 作业：
`npm ci --no-audit --no-fund` → `npm run build` → `npm test`（同样 build 先于 test），
`full=true`（含 `.github/*`、依赖与构建策略改动）时一并触发，并已接入 `pr-gate` 的
`needs` 与 `require_if_selected`。同一次改动还修正了 `tauri-unix` 作业：补齐
`pkg-config` / `libwayland-dev`（与 CodeQL 对照）与 Tauri Linux 依赖，并在 Rust
format/clippy/test 之前安装 Node 与构建前端 `dist`。**这两个作业都尚未在 GitHub 上运行过**，
本地也**没有**为了它们下载任何 Linux target。

生成物一致性由 `haven-application/tests/wire_bindings_consistency.rs` 与
`src-tauri/tests/capability_consistency.rs` 在测试中强制，不依赖人工检查。
MCP 的枚举漂移由 `mcp/haven-mcp/test/wire-drift.test.ts` 对着生成的 `wire.ts` 强制。

## 测试矩阵（最小覆盖）

| 主题 | 位置 |
| --- | --- |
| profile 校验（控制字符/路径/凭据/端点） | `haven-domain/src/ai_provider.rs` 单测 |
| CAS upsert / CAS delete 冲突 | domain 单测 + `haven-infrastructure` 单测 |
| `haven:ai:<profile-id>` 命名空间 | `haven-domain/src/credential.rs` 单测 |
| secret 不出 wire / 普通存储 / 日志 | application + tauri + 前端 gateway 测试 |
| `CredentialProviderDto::Ai` | `haven-application` + fixture 一致性 |
| 无配置时模型目录为空 | application 单测 + Mock fixture + 前端 hook 测试 |
| `/models` 成功 / 非 2xx / 非法 JSON / 缺能力字段 | `haven-infrastructure/src/ai_provider.rs` 单测 |
| Agent 资源 partial patch 合并、None 不覆盖与敏感自由文本保留 | `haven-domain/src/settings.rs` 单测 + application/infrastructure setting proposal 单测 |
| Agent Proposal / Receipt 脱敏且 authoritative 设置不变 | `haven-domain/src/agent.rs` + `haven-application/src/services/agent_settings_ipc.rs` + SQLite setting proposal 单测 |
| 命令 manifest 与生成 wire 一致 | `capability_consistency.rs` + `wire_bindings_consistency.rs` |

## 实现结果（2026-09-18）

### 已落地

- **Domain**：`haven-domain/src/ai_provider.rs` —— 闭合 `AiProviderKind`、非敏感
  `AiProviderProfile`（严格 id / 显示名 / endpoint / 模型 id 校验，拒绝控制字符、
  本地路径、凭据形状、query、fragment）、三态 `AiModelCapability`、
  `AiModelDescriptor`（可选文本清洗 + 按字符边界截断）、`models_url_for`
  （不重复 `/v1`、不重复 `/models`）。
- **Common**：新增 `HttpUrlPolicy::AiProviderEndpoint`，语法与端口规则与
  `SourceEndpoint` 一致，只是把上下文显式化。
- **Application**：`AiProviderProfileService`（`list` / `get` / `upsert` / `delete` /
  `models_catalog`）+ `AiModelCatalogPort` + `UnavailableModelCatalog`。
  凭据统一 `haven:ai:<profile-id>`；DTO 只投影 `credentialConfigured` 布尔事实。
- **Infrastructure**：迁移 `045_ai_provider_profiles`（幂等、`CREATE TABLE IF NOT
  EXISTS`、表级 CHECK 与 Rust 校验同形）、`SqliteAiProviderProfileRepository`
  （CAS 写 + CAS 删 + 写后同锁回读）、`OpenAiCompatibleModelCatalog`
  （DNS 固定、关闭重定向、有界超时、按块限流读取）。
- **Tauri**：`ai_provider_profile_list/get/upsert/delete`、`ai_provider_models_list`；
  命令只调用 Application service。
- **前端**：`ai-provider-gateway.ts`（`Reflect.ownKeys` 精确形状守卫 + secret 形状
  拒绝）、`useAiProviderSettings.ts`、设置页 AI 分组接线。API Key 单向提交，
  回显仅「已配置 / 未配置」；无可用模型时显示「无可用模型」。
- **契约**：`contracts/ipc/v1/fixtures/ai-provider/*` 六个 fixture +
  `fixtures_consistency.rs` 双端消费基线。

### 收尾复审修复（真实缺陷）

1. **被拒绝的删除曾会销毁 API Key**。删除路径原先"先删凭据、再 CAS 删行"，因此
   携带过期 `expectedRevision` 的删除会先清掉密钥、再返回 `REVISION_CONFLICT`：
   操作报告失败，却已经执行了不可逆的破坏性副作用。现在 `expected_revision` 在
   触碰凭据之前先与刚读到的版本比对，可预见的过期请求零副作用地失败。
   仅保留"读之后、CAS 之前被并发写者推进版本"这一无法用单条 SQL 消除的窗口，
   该窗口留下的中间态是"profile 存在但未配置密钥"——可见、可重设、可再删除。
2. **`upsert` 的响应与权威状态不一致**。更新时数据库保留原 `created_at`，而服务
   原先用本地候选值（`created_at = now`）拼响应，于是"写成功"的响应里的
   `createdAt` 与随后 `get` 读到的值不同。`cas_upsert` 现在返回**写后回读的行**
   （与写入共用同一次 `Db::lock()`），并移除了会产生这种不一致的
   `AiProviderProfile::with_persisted_state`。
3. **响应体上限在 chunked 响应下形同虚设**。原先先 `bytes()` 缓冲整份响应再检查
   长度，声明长度为空的 chunked 响应会先被完整读进内存。现在按块累积并逐块校验，
   越界立即拒绝。
4. **Mock 的删除忽略 `expectedRevision`**。后端会因 CAS 冲突拒绝并保留数据，而
   Mock 无条件删除，于是 UI 的冲突分支在开发环境永远不会被走到。Mock 现在执行
   同样的 CAS 前置比对，并在通过后才清理凭据。

### 已记录的边界

- 私网 / 回环 / 单标签主机端点**仍然被拒绝**（`AI_SYSTEM.md` §7）。因此第一版
  不支持本机运行时端点（如 `http://127.0.0.1:11434/v1`）。放宽需要独立 ADR，
  不在本切片内。
- Mock 的端点校验只做形状检查，主机策略由 Rust 侧唯一实现：在 TypeScript 里
  再写一份安全策略只会制造两套会漂移的真相。
- `/models` 的响应解释逻辑（状态码 / JSON / 能力三态）被拆成纯函数并由单测完整
  覆盖；真实网络路径（DNS 固定、重定向拒绝）由 `http_security` 的既有策略测试与
  端点拒绝测试覆盖，没有起真实 HTTP 服务器。

---

## A3：MCP 协议面 + Typed Bridge 端口（已落地）

位置：`mcp/haven-mcp/`（TypeScript，MCP TypeScript SDK + Zod）。用一个独立子包而不是
塞进 `前端/app`：MCP server 是被外部客户端当子进程启动的东西，与浏览器/WebView 的
构建目标、依赖面、运行时假设都不同。

### 已实现

- **冻结的 9 个工具**（`src/constants.ts` 的 `TOOL_NAMES`）。清单是闭合集合，
  `test/tools.test.ts` 断言真实注册结果与它完全相等，并对工具名跑禁用词根扫描
  （`apply` / `approve` / `reject` / `write` / `delete` / `filesystem` / `shell` /
  `sql` / `secret` / `metadata_patch` / `file_renames` …）。
- **Strict Zod 输入**。用 `.strictObject` 而不是 `.object`：后者会**丢弃**未知字段，
  于是"模型传了 `apply: true` 而服务器静默忽略"——调用方以为意图被接受了，那是最坏
  的一种失败。跨字段规则（`target_scope` 与 `media_item_id` 一致、patch 至少一个键）
  用 `.refine` 表达，因为 Zod v4 的 refine 保留对象类型，所以 SDK 仍能正确生成
  `additionalProperties: false` 的 JSON Schema。
- **`outputSchema` + `structuredContent`**。SDK 会对成功结果强制校验，因此
  "结构化输出与声明一致"由框架保证，而不是靠自觉。
- **文本与结构化同源**：文本永远由 `structuredContent` 渲染而来（`json` 格式下
  `JSON.parse(text)` 深度相等；`markdown` 格式由同一个通用渲染器派生，结构化值里的
  每个标量都出现在文本里）。
- **脱敏**（`src/redact.ts`）：内容层打码凭据 target / 绝对路径 / Bearer / API key
  形状；结构层对 `secret` / `credential_ref` / `sql` / `db_row` / `raw_provider_response`
  / `stack_trace` 等键**整键丢弃**。刻意**不**做通用十六进制打码——提案 digest 是
  64 位小写十六进制，必须原样返回给用户核对。
- **有界**：单字段 512 字符、列表 50 条、整份响应 24 000 字符；超限按工具声明的规则
  收缩并**重新渲染**（省略会写进结构化值），收缩不收敛时抛稳定错误而不是返回半截响应。
- **稳定错误**：`SCREAMING_SNAKE_CASE` 码 + 可展示消息 + `retryable`；非 `HavenMcpError`
  的异常一律归一为 `INTERNAL_ERROR`，不回显原始消息或堆栈。
- **Typed Haven Agent Bridge 端口**（`src/bridge.ts`）：9 个方法，**没有任何**
  apply / approve / reject / delete / write 方法。不读 SQLite、不碰文件系统、
  不访问 CredentialStore、不 invoke Tauri。
- **显式不可用适配器**：生产默认，每个操作抛稳定的 `HAVEN_BRIDGE_UNAVAILABLE`，
  **不返回任何看起来像数据的空值**。
- **fail-closed 的桥接选择**（`src/bridge-selection.ts`）：默认 `unavailable`；
  `fixture` 需要 `HAVEN_MCP_ALLOW_TEST_BRIDGE=1` **且**注入 `fixtureFactory`，而生产入口
  不注入 —— 因此测试替身不可能成为生产数据源。**`live` 在 A5.5 之后由
  `src/local-broker.ts` 实现**：缺端点 → 显式不可用桥接；端点形态非法 →
  `HAVEN_MCP_ENDPOINT_INVALID` 且不尝试连接。
- **stdout 洁净**：日志只走 stderr（`src/log.ts` 是唯一出口），并由源码守卫测试持续盯住；
  真实子进程端到端用例断言 stdout 每一行都是 JSON-RPC。

### 接通状态（必须明确区分）

| 层 | 状态 |
| --- | --- |
| MCP 协议面（9 个工具、schema、注解、错误语义） | 已实现并冻结 |
| Typed Haven Agent Bridge 端口 | 已实现 |
| 显式不可用适配器（生产默认） | 已实现 |
| **本地运行时传输适配器** | **已实现（A5.5）**，但默认不启用且未经真实验收——见下文 A5 节 |
| 9 个工具对应的 Haven 后端用例 | **9 个均已实现**；真实数据库数据与端到端仍未验收 |

- **`live` 传输已实现（A5.5），但默认不启用、也从未验收。** MCP 客户端把本 server 当子进程
  启动，而它要访问的是另一个进程（运行中的 Haven）：`HAVEN_MCP_BRIDGE=live` **且**
  `HAVEN_MCP_ENDPOINT` 形态合法时走 `LiveHavenAgentBridge`，只做 outbound `node:net` 连接；
  缺端点时保持显式不可用，端点非法时 `HAVEN_MCP_ENDPOINT_INVALID` 且不尝试连接。
  现有连接用例用的是**进程内假 transport**，因此 Haven→Node→MCP 的真实端到端仍未验证。
  请求一个"没有实现"的传输时我们**不静默降级**——静默降级会让调用方以为"桥接通了，
  只是没数据"。
- **9 个工具的后端路径均已实现**，并由 Rust Broker 的 8 个 granted requests
  （6 个读取请求 + 2 个创建 pending Proposal 请求）接通到现有
  `AgentContextQueryService` / `AgentSettingsIpcService`。`get_system_capabilities`
  会如实报告每一项能力；这不改变默认 Broker 关闭、端点未配置时的
  `HAVEN_BRIDGE_UNAVAILABLE`。
- **默认配置下工具仍不可调用**，原因是外部 Broker 默认关闭，而不是后端用例缺失。
  live 通道显式开启且端点合法后，9 个工具才有机会进入 Rust；真实 Haven→Node→MCP
  链路和四个客户端仍未验收，**不能把单元测试通过写成已接通生产 MCP**。

### 测试（A3 阶段 63 → 78 项；含 A5.5/A5.6 后当前为 88 passed / 1 skipped）

| 主题 | 位置 |
| --- | --- |
| 恰好 9 个工具；禁用工具不存在；注解；`additionalProperties:false`；snake_case 字段 | `test/tools.test.ts` |
| strict 输入：未知字段 / 非法枚举 / 锚点格式 / 跨字段一致性 | `test/tools.test.ts` |
| 只读零写入（工具层 + 端口层各一轮） | `test/tools.test.ts` |
| 提案只产生 `pending`；结构里没有 token/secret/路径的位置 | `test/tools.test.ts` |
| Haven 侧拒绝（如上下文过期）被如实透传且零提案 | `test/tools.test.ts` |
| 脱敏：凭据 target / 路径 / 密钥 / SQL / DB 行 / Provider 原始响应 / 堆栈 | `test/tools.test.ts`、`test/redact.test.ts` |
| 有界与 shrink 循环；不收敛时的硬上限 | `test/respond.test.ts` |
| 文本与结构化同源（json 严格相等 / markdown 覆盖标量） | `test/tools.test.ts`、`test/respond.test.ts` |
| 桥接选择 fail-closed | `test/bridge-selection.test.ts` |
| live 本地传输：帧编解码 / 拆包重组 / 越界与错配响应拒绝 | `test/local-broker.test.ts` |
| Codex / Claude Code / DSH / Pi 配置夹具形状与未验证标记 | `test/client-fixtures.test.ts`、`fixtures/clients/*.json` |
| Rust Broker 8 类请求的真实 dispatch、能力 gate、配额前置与 resource patch 映射 | `src-tauri/src/agent_broker/{protocol,session}.rs` |
| Provider 前置条件、显式 Supported chat、Structured Output → pending Proposal、零写入 | `haven-application/src/services/{ai_provider,agent_settings_ipc}.rs` |
| Agent Trace 闭合事件、上下文绑定、服务端序号、上限与敏感字段拒绝 | `haven-application/src/services/agent_trace.rs` |
| 生产入口行为 + stdout 洁净（真实子进程） | `test/stdio-e2e.test.ts` |
| stdout 洁净 / 无 fs-SQL-Tauri 依赖 / 端口无写方法 | `test/source-guards.test.ts` |
| 枚举与 patch 字段对生成 `wire.ts` 无漂移 | `test/wire-drift.test.ts` |

命令：`cd mcp/haven-mcp && npm run typecheck && npm run build && npm test`

### 如实记录的一处不完美

输入 schema 校验失败的**错误文本由 MCP SDK 生成**，它会回显调用方自己传入的值
（例如 `Invalid enum value ... received 'xxx'`）。那不是跨方泄漏——同一方的输入回到
同一方，且该值本来就在调用方的上下文里——所以没有为此拦截 SDK。我们真正保证的是另一半：
错误文本里不会出现任何 **Haven 侧**材料（凭据 target、路径、DB 列、堆栈）。
`test/tools.test.ts` 对这条断言。

---

## A4：正式 Skill —— `skills/haven-agent-proposal`（已落地）

按 skill-creator 规范创建的仓库内 Skill。它**是行为协议，不是权限**：只规定"拿到 MCP 的
9 个工具后应该怎么用"，不新增任何工具、不改变任何 ACL、不提升任何能力。

### 文件

| 文件 | 作用 |
| --- | --- |
| `skills/haven-agent-proposal/SKILL.md` | 182 行的可执行协议（frontmatter + 工作流 + 输出结构 + 禁止事项） |
| `skills/haven-agent-proposal/references/mcp-tool-map.md` | 9 个工具的映射、完整 patch 字段清单、错误码含义、当前接通状态 |
| `skills/haven-agent-proposal/evals/evals.json` | 5 条现实提示 + `expected_output` + 可验证 expectations |
| `tools/skills/skill-contract-check.py` | 确定性契约校验（可从任意 Skill 复用） |
| `tools/skills/skill-contract-check.test.py` | 21 项自测，逐条证明校验规则真的会失败 |

> 命名说明：skill-creator 明确要求技能包内不放 `README.md` 这类"关于技能本身的说明文档"。
> 参考材料因此命名为 `references/mcp-tool-map.md` —— 语义清晰，且符合"只放被引用的材料"。

### 协议要点

1. 永远先调 `get_system_capabilities`；桥接不可用 / 能力不具备 / 工具未实现时**如实说明**，
   绝不编造数据、模型或能力。
2. 能力允许时先读脱敏上下文；全局阅读设置用 `get_settings_snapshot`，并**逐字保留**
   `context_id` / `context_hash` / `base_revision`。
3. 全局改动只调 `propose_settings_patch`；资源级改动只有在能力清单允许且读写工具都可用时
   才允许，否则明确告知暂不可用——不用全局提案近似资源级请求。
4. 提案结果**只表示 pending**：把 digest、逐项 changes、作用域、过期时间告诉用户，并明确
   "尚未应用"，要求用户在 Haven UI 查看 Diff 后批准。Skill 不等待、不伪造 approval token，
   也不调用 Apply/Approve。用户说"直接应用"时**仍然拒绝绕过 UI**。
5. 永远不读取/回显 API key、`credentialRef`、绝对路径、文件内容、SQL、Provider 原始响应；
   不做 `metadata_patch` / `file_renames`；不从模型名猜能力；MCP 断开时给安全的手动路径。
6. 不声称"已应用/已保存"——本 Skill 没有读取回执的能力。

### 确定性校验（可重复运行）

```text
# 仓库自己的检查（locale 无关：所有文件读取都显式指定 utf-8）
python tools/skills/skill-contract-check.py
python tools/skills/skill-contract-check.test.py

# skill-creator 现成脚本（需从 skill-creator 根目录运行，并显式设置 UTF-8 / 模块路径）
cd <skill-creator-root>
PYTHONUTF8=1 PYTHONPATH=. python scripts/quick_validate.py <repo>/skills/haven-agent-proposal
PYTHONUTF8=1 PYTHONPATH=. python scripts/package_skill.py <repo>/skills/haven-agent-proposal <temp-dir>
```

`PYTHONUTF8=1` 不是可选的美化：`quick_validate.py` 用 `read_text()` 而**没有指定编码**，
在本机 GBK 默认编码下会直接 `UnicodeDecodeError`；`package_skill.py` 还通过
`scripts.quick_validate` 导入模块，所以需要从 skill-creator 根目录运行或设置 `PYTHONPATH`。
这些都是上游脚本的 locale / module-path 依赖，不是技能内容的问题——我们自己的检查脚本全部
显式指定 `encoding="utf-8"`，因此不受影响。

### 接通边界（与 A3 相同，Skill 不改变它）

- **live 运行时传输已实现但默认关闭** → 默认环境下 `bridge.available = false`；并且它从未在
  真实客户端上验收（见 A5 节）。
- **9 个工具的后端路径已经实现**，但默认桥接关闭、端点缺失或真实链路不可达时，仍会
  返回 `HAVEN_BRIDGE_UNAVAILABLE`；能力被服务端关闭时才返回
  `HAVEN_CAPABILITY_UNAVAILABLE`。
- 因此 Skill 的现实价值是：无论桥接是否可用，它都让模型**如实说明并给出手动路径**，
  而不是编造数据或绕过审批。evals 覆盖桥接不可用与能力读取路径。

---

## A5：跨客户端 MCP 运行时传输（设计已冻结；核心已落地，**未验收**）

设计全文：[`docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md`](../architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md)

**目标**：让用户已装好的外部 Agent（Codex / Claude Code / DSH / Pi …）都以 **MCP stdio**
接入，并且接入的是**同一套** 9 工具契约与**同一个** Proposal/Approval/Receipt 安全内核。

**真实目标的一句话**：各客户端的差异只是「配置里怎么写一条 stdio 命令」；
工具名、schema、错误码、安全语义完全一致。**不是**为每个客户端写适配器。

**链路**：

```text
外部 Agent 客户端
  → MCP stdio（客户端拉起 haven-mcp-server 子进程）
  → 9 个冻结工具（strict schema / 脱敏 / 有界 / 同源）
  → node:net outbound client（目标 = HAVEN_MCP_ENDPOINT，见下）
  → Local IPC Broker（Named Pipe / Unix socket）
  → Rust Application bridge（8 个 Read / Propose 请求 + 服务端生成身份 + 配额前置）
  → AgentSettingsIpcService → Domain Proposal 内核
  → 用户 UI 批准 → CAS/Apply → Receipt（唯一写入路径，A5 不碰）
```

**端点形态（修订）**：不使用发现文件。Haven UI 显示并可一键复制**每用户稳定**的端点
与一份四客户端通用的 MCP 配置模板（含 `HAVEN_MCP_BRIDGE=live` 与 `HAVEN_MCP_ENDPOINT`）；
Node 侧只认这一个环境变量，只做 `node:net` 出站连接，不扫描文件系统、不枚举管道、
不探测端口。

> 已落地情况：设置页「外部 Agent 接入」分组在**真实监听**状态下给出可复制端点与四客户端
> 共用模板（`buildAgentBrokerMcpTemplate`，四个客户端的 JSON 完全相同）；浏览器 Mock 只
> 显示 `mock://` 预览，既不可复制也不给模板。设计里的「测试端点」按钮当前**不存在**，
> 登记为后续待补——在它出现之前，"端点已显示"不等于"端点已验证可达"。

### 拆分与门禁

| 子项 | 范围 | 交付物 | 门禁 | 状态（2026-09-20 工作树） |
| --- | --- | --- | --- | --- |
| **A5.1** Broker protocol / schema | 帧格式、握手、**闭合 granted request 集 8 项**（6 个 Read + 2 个 Propose）、响应集（`ok`/`error`）、取消、五条帧级上限、稳定错误码 | 协议 schema + fixtures | 8 项请求集；`cancel` 仅为控制帧；`get_proposal`/approve/reject/apply/receipt **不存在**；五条上限逐条触发 | ✅ 已实现：`agent_broker/{protocol,session,error}.rs`；定向 Broker 测试通过 |
| **A5.2** Windows Named Pipe | 服务端建管道；DACL **只授创建者用户 SID**（不含 `SYSTEM`）；端点名每用户稳定；创建失败不接管他人端点 | Rust 实现 + 测试 | 真实建管道后读回安全描述符断言；第二实例 fail closed | ✅ 已实现：真实管道往返 + DACL 只含当前用户 + 第二实例报 `HAVEN_BROKER_ENDPOINT_BUSY` 均有测试（后者在首个 bind 失败时会提前返回，不能替代真机验收） |
| **A5.3** Unix socket | `$XDG_RUNTIME_DIR/haven/agent-v1.sock`（回退 `$HOME/.haven/run`）、`bind` 前设 umask、`0600`/`0700`、同字节协议；运行目录 symlink/非目录 fail closed；stale socket 只删固定 socket 且最多重绑一次 | Rust 实现 + 测试 | `stat` 断言 + 无「先建后 chmod」窗口 + 目录/端点 symlink 与非 socket 保护 | ⚠️ 代码已写并补齐目录/类型/归因保护；**已添加 Linux all-targets CI 作业（`tauri-unix`），但该 job 尚未在 GitHub 运行；本机仍未编译**。因此门禁**未过** |
| **A5.4** Rust Application bridge | Broker 帧 → `AgentContextQueryService` / `AgentSettingsIpcService` 映射；投影 8 个 Read / Propose 请求；握手生成 `AgentSessionId`、每帧生成 `AgentRequestId`；能力逐请求重校验；配额前置校验 | Rust 实现 + 测试 | 请求集是 Application 方法表的**真子集**；帧载荷**不含** `session_id`/`request_id` 且传入值由 Broker 生成；读取不消耗 Proposal quota；配额超限时**未进入** Application | ✅ 已实现：`AgentSettingsBrokerApi`；8 类 dispatch、身份服务端生成、配额前置、禁帧到不了 Application 均有单测 |
| **A5.5** Node local bridge | `HavenAgentBridge` 的 `live` 适配器：读 `HAVEN_MCP_ENDPOINT` → 平台形态校验 → `node:net` 连接 → 握手校验 → 超时/取消/断线语义 | TS 实现 + 测试 | 未设/格式非法端点给稳定错误且**不尝试连接**；不重试、不扫描、不回退；`selectBridge({HAVEN_MCP_BRIDGE:"live"})` 绝不静默降级成 unavailable | ✅ 已实现：`src/local-broker.ts`；本轮 `npm run build && npm test` = 88 passed / 1 skipped（1 skipped 是 dist 已构建时跳过的占位提示用例）。**连接用例用进程内假 transport，非真实 Broker** |
| **A5.6** four-client fixtures | 四个客户端共用同一份配置模板与工具清单，产出同一结果 | 四份配置夹具 + 契约测试 | 夹具差异只允许出现在命令路径与配置字段名；**未实测就标注未实测** | ✅ **夹具已实现**：`fixtures/clients/{codex,claude-code,dsh,pi}.json` + `test/client-fixtures.test.ts`；每份明确 `verified: false`，真实客户端兼容性仍未验收 |
| **A5.7** security | 默认关闭 + 用户显式开启、端点与模板 UI、DACL/umask、**配额 8 / 8 / 32**、沿用 24h TTL、错误文案脱敏 | 实现 + 测试 | 默认关闭时端点不存在且 UI 不显示；三项配额逐项触发、`HAVEN_BROKER_QUOTA_EXCEEDED` 且零写入 | ✅ 核心已实现并有单测：默认关闭、DACL、配额在进入 Application 之前拒绝；端点与模板 UI 已落地。「测试端点」按钮待补；**真机 WebView2 未验收** |
| **A5.8** lifecycle / cancellation | 握手超时、请求超时（20 s）、空闲超时、断连清理、Haven 退出、多实例 fail closed | 实现 + 测试 | Broker 20 s 先于 MCP 15 s 触发；Haven 退出后 `get_system_capabilities` 仍可用且如实报告；第二实例不接管端点 | ✅ 核心已实现并有单测：握手/请求超时、断连清理、停机、多实例 fail closed；Unix stale socket 的锁/探活/类型保护也有 `#[cfg(unix)]` 用例。空闲超时（30 min）已有**毫秒级专门用例**（握手完成后不再发请求 → 连接自己收尾，另带"生产默认下不提前关闭"的对照）。**20 s 与 15 s 的先后关系、Haven 退出后的错误归因、Unix 代码都没有本机真实双进程/Unix 验证**；空闲超时用例同样只跑内存 duplex |

### 关键契约（本阶段定稿，不留「以后再定」）

- **请求集**：8 项 granted requests：`context`、`setting_sources`、`resource_preference`、
  `library_summary`、`media_capabilities`、`onboarding`、`create_proposal`、
  `create_resource_proposal`。`cancel` 是控制帧，不计入 granted requests。
  `get_proposal` 从 v1 删除——批准后的状态与 Receipt 仍归 Haven UI。
- **身份**：`session_id` / `request_id` **不由客户端传入**。Rust Broker 在握手后生成
  `AgentSessionId` 并持久到连接生命周期；每个已验证 frame id 映射为新的 `AgentRequestId`。
  `hello.clientInfo` 只能做诊断，绝不成为领域身份。
- **配额**：每连接在途请求 ≤ **8**；每连接外部提案创建尝试 ≤ **8**；
  每个 Haven **进程窗口**外部提案创建尝试 ≤ **32**；成功创建的 pending Proposal TTL
  沿用既有 **24 小时**。失败尝试不退款，避免在结果不确定时放大噪音预算。
  超限返回 `HAVEN_BROKER_QUOTA_EXCEEDED`，**在进入 Application 之前拒绝、零写入**。
  这是**进程窗口计数，不是跨重启的持久计数**——重启清零属已知残留风险（不构成提权，
  详见设计 §8.3 的理由）。
- **多实例**：**不声称支持**同时提供 Broker。第二实例创建端点失败即 fail closed，
  不接管、不改用自己的另一个端点，报 `HAVEN_BROKER_ENDPOINT_BUSY` 并告知用户。

### 本阶段**不**引入（硬约束）

- ❌ Python sidecar、PydanticAI、完整 Agent runtime。
- ❌ Apply / Approve / Reject / Delete / Filesystem / SQL / Secret 中的任何一项。
  Broker 里**不存在**这些帧类型——不是关着，是无法构造。
- ❌ TCP / Streamable HTTP / 任何网络监听；不做端点自动发现（不扫描文件系统、
  不枚举管道、不探测端口）。
- ❌ Node MCP server 读 SQLite / 文件系统 / CredentialStore / invoke Tauri。
  **`node:net` 的边界**：仅允许 `net.connect()` 到校验通过的本地端点；
  禁止 `net.createServer` / `listen`，禁止 `node:http` / `node:https` / `node:dgram` /
  `node:tls` / `node:child_process`。这是**本地 IPC 客户端**，不是网络访问。
  落地时需把 `mcp/haven-mcp/test/source-guards.test.ts` 里对 `node:net` 的整条禁用
  改为「允许导入但断言无 `createServer`/`listen`」。**已完成**：`node:net` 已移出
  `forbiddenModules` 并有专门用例；`node:http` / `node:https` / `node:dgram` /
  `node:tls` / `node:child_process` 与 fs / SQLite / 凭据库 / Tauri 模块仍留在
  `forbiddenModules` 里整条禁止（`node:dgram` 与 `node:tls` 曾登记为守卫未覆盖的
  残余缺口，现已补进列表）。
- ❌ 不为任何客户端加「特权模式」或客户端专属工具分支。

### 验收项：多 Agent 兼容

多客户端兼容是**验收项**，不是宣传语：

1. 四个客户端使用**同一份** MCP 工具清单、schema 与配置模板（由共用模板与四份夹具契约测试
   覆盖；夹具的 `verified` 仍为 false）；
2. 四份配置夹具的差异**只允许**出现在命令路径与配置字段名上（契约测试已验证形状，真实客户端
   兼容性仍未实测）；
3. `clientInfo` 在任何情况下不影响权限与身份——伪造 `client.name` 后行为逐位相同
   （已由 `agent_broker/session.rs` 的 `client_info_cannot_influence_identity_or_capabilities`
   覆盖）；
4. 未实测的客户端必须在矩阵里标为「设计目标；未实测」，不得写成「支持」。

### 当前状态

**A5 的核心已落地，但没有任何一项经过完整真实验收。** 证据与边界：

- **Rust Broker 与接线已实现**：`AppState` 持有 `AgentBrokerManager`，
  `agent_broker_status` / `agent_broker_enable` / `agent_broker_disable` 已注册，
  `command-manifest.rs` 与生成的权限 / capability 已接线。`agent_broker` 定向与全量
  `cargo test` 通过；Windows 侧的真实命名管道往返与 DACL 读回断言在测试里通过。
- **设置页「外部 Agent 接入」分组已实现**：默认关闭、状态与启停、真实端点复制、四客户端
  共用模板；浏览器 Mock 只显示预览、不给可复制地址或模板。**真实 Windows WebView2 上的
  该界面尚未验收。**
- **Node live 适配器（A5.5）已实现**：`HAVEN_MCP_BRIDGE=live` 且 `HAVEN_MCP_ENDPOINT`
  形态合法时连接 Rust Broker；默认仍是 `unavailable`；未配置 / 非法端点 fail closed；
  不读文件系统、SQLite 或 secret。`mcp/haven-mcp` 本轮 `npm run build && npm test`
  = 88 passed / 1 skipped。
- **本轮 Unix 代码审阅与修复**：Claude Code 新会话只读检查确认没有 P0/P1；已修正运行目录
  symlink/非目录保护、非 socket 占用的错误归因、第二个 umask 窗口的块作用域、`EAGAIN`
  兼容匹配，并统一了文档中的回退端点字面路径。Windows 本机 `cargo fmt --check`、Broker
  定向测试 72/72、全量 Rust 测试 185 个库测试及全部 IPC/契约测试通过；`rustup target list`
  只有 `x86_64-pc-windows-msvc`，Linux target 检查因未安装 target 失败，Unix 路径本机仍未
  编译或运行。Public CI 已为此新增 `tauri-unix` 的 Linux all-targets 作业（补齐 CodeQL 对照
  所需的 `pkg-config`/`libwayland-dev`，并在 Rust format/clippy/test 之前构建前端 `dist`），
  但**该作业尚未在 GitHub 上运行过**——本机也**没有**下载或安装任何 Linux Rust target，
  因此"已添加 CI"不等于"已编译验证"。
- **本轮复审的 P2 修复**（最小、可证明）：Windows `create_instance` 的
  `ERROR_ACCESS_DENIED` 现在只在**首实例**（`FILE_FLAG_FIRST_PIPE_INSTANCE`）才映射成
  `HAVEN_BROKER_ENDPOINT_BUSY`，后续实例归类为 `HAVEN_BROKER_ENDPOINT_UNAVAILABLE`
  （新增单测 `access_denied_is_busy_only_for_the_first_pipe_instance`）；`BrokerStatus::EndpointBusy`
  保持内部变体，并在文档与代码注释里明确 busy 只经 `enable` 的失败返回、`status` 不主动
  探测因而**不会**投影 busy。
- **未完成**：Unix 侧编译与运行（本机 Windows）；真实 Windows WebView2
  验收；Codex / Claude Code / DSH / Pi 四个真实客户端；Haven→Node→MCP 的真实端到端
  （现有连接用例用的是进程内假 transport，不是真的 Broker）；「测试端点」按钮。
- 默认行为仍是 A3 的诚实不可用：未设 `HAVEN_MCP_BRIDGE` 时 `bridge.available = false`，
  工具返回 `HAVEN_BRIDGE_UNAVAILABLE`；服务端能力明确关闭时才返回
  `HAVEN_CAPABILITY_UNAVAILABLE`。9 个工具的后端路径已经存在，但**不能把"链路上线"
  写成"整个 MCP 已完成"，也不得把单测写成真实客户端已接通。

已知缺口（登记，不在 A5 解决）：同用户恶意进程无法与合法客户端区分（因此通道只暴露
只读 + 创建 pending 提案，并靠三重配额与 24h TTL 抑制噪音）；配额为进程窗口计数，
重启清零；四客户端全部未实测（夹具存在但 `verified: false`）；
Unix 侧在本机未编译验证（本机 Windows，且未下载任何 Unix target）；CI 已添加 Linux
all-targets 作业，但该 job **尚未在 GitHub 运行**，故不构成编译验证；真实 WebView2 UI 与端到端未验收；「测试端点」按钮待补；
空闲超时已有毫秒级专门用例，但只用内存 duplex，真实双进程/平台端点上的空闲断连仍未验证；`get_proposal` 不在 v1；
多实例同时提供 Broker 不支持。

---

## A6：9 个 MCP 工具的 Haven 后端路径（已落地，未做真实端到端验收）

A6 已把原先只接通设置上下文与全局设置提案的 Broker 扩展为完整的 9 工具后端面：

- `AgentContextQueryService` 提供 `setting_sources`、资源级阅读偏好快照、library summary、
  media capabilities、onboarding state；资源偏好 context id 由 authoritative payload hash
  派生，`ContentCategory::All` 在投影层保持 `null` 语义。
- `AgentSettingsIpcService` 负责全局与资源级 proposal，二者都进入既有
  Proposal → Approval → CAS → Apply → Receipt 内核；资源级 Agent proposal 保存部分
  `AgentResourcePreferencePatch`，批准时在同一 UoW 内重读并合并 authoritative 偏好，
  不把完整 merged `PreferenceData` 放进 Agent Proposal。
- Agent Receipt 的 before/after 是脱敏审计投影：authoritative 设置继续保存真实值，但 Agent
  上下文、Proposal 展示、Receipt 展示与错误边界不回显路径、endpoint 或凭据样式自由文本。
  Broker 只允许创建 pending proposal，不存在 approve/apply/receipt 帧。
- Rust Broker 的 `GRANTED_REQUESTS` 固定为 8 项，所有请求逐项做能力 gate；读取请求不消耗
  Proposal quota，全局和资源 proposal 共用进程窗口额度。

已验证：Rust protocol/session 的严格字段、枚举、UUID、limit、scope、patch 映射、quota 与
真实 dispatch 单测；尚未验证：真实运行中的 Haven→Node→MCP、四个外部客户端、Windows
WebView2 与 Linux Unix socket。

## A7：Provider Structured Outputs → pending Proposal（已落地）

第一版只实现 OpenAI-compatible `/chat/completions` 的 typed recommender：

1. profile 存在且启用；
2. CredentialStore 有凭据；
3. 显式选择模型；
4. `/models` 目录存在该模型且 `chat == Supported`；
5. 读取 authoritative settings context；
6. 请求 `response_format.type=json_schema` 且 `strict=true`；
7. 验证结构化 `ReadingPatch` 后调用 `create_proposal`。

任一前置条件不满足时不发模型请求，也不猜 `gpt-4o`；Provider 不持有 Apply、审批、
Receipt 或文件系统权限。测试覆盖 ready/disabled/no credential/no model/Unknown/Unsupported/
stale context、结构化输出和零写入。

## A8：Agent Trace typed 基础（已落地，暂不接 UI）

`AgentTracePort` 与有界 collector 已落地。事件闭合集合包含 request/context/provider/
structured-output/proposal/approval/CAS/apply/receipt/cancel/retry/failure；每条事件绑定
session、request、context、hash、服务端 sequence、时间和可选 duration。每会话最多保留 256
条；不接受调用方序号或自由文本，不记录 secret、路径、SQL、正文或 raw provider response。

本轮明确不实现 AI 对话框、Diff 审批 UI、轨迹图 UI；未来 UI 只能消费这套 typed 事件，不能
建立第二套前端事实源。

## 本轮后端收尾（2026-09-20）

本轮按“前端 UI 暂不接入”的边界补齐了后端安全闭环：

- `PreferenceData::apply_patch` 覆盖 reading/comic 两个资源偏好分区的字段级合并；空 patch
  幂等，顶层 `None` 不会清除 authoritative 分区。
- Agent 资源 Proposal 改为只保存请求的部分 patch；批准时重新读取当前值并检查
  `base_revision`，在同一事务里合并后 CAS 写入。
- `SettingChangeReceipt` 的 Agent before/after 在所有应用入口统一走脱敏审计投影；真实设置
  事实源仍保留用户值，Agent 侧不看到路径、endpoint、Bearer 或凭据样式文本。
- 补齐了领域、Application、Infrastructure、Broker、MCP 与 Skill 合约验证；重新生成并
  校验 `wire.ts`，但没有接入 AI 对话框、设置页聊天框或轨迹图。

这不代表 AI 前端已经完成，也不代表四个真实外部 Agent 或 Haven→Node→MCP 端到端已经验收；
它只表示 Proposal/Approval/Receipt/MCP/Skill 可以作为后续 UI 的稳定后端契约。
