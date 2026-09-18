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
| Haven 后端的其余 6 个只读/提案用例 | **未实现**（见下表） |

因此现在的行为分三种：

1. **默认**（未设 `HAVEN_MCP_BRIDGE`）：`haven.available = false`，其余工具返回
   `HAVEN_BRIDGE_UNAVAILABLE` 或 `HAVEN_CAPABILITY_UNAVAILABLE`；
2. **`live` 且端点形态合法**：连接本机 Haven Broker（需要用户在栖阅设置页显式开启
   「外部 Agent 接入」）。注意 `haven.available = true` **只表示端点形态通过校验**，真实连接
   状态要到调用时才确认——Broker 没开、Haven 没运行或 ACL 拒绝时，调用仍会拿到
   `HAVEN_BRIDGE_UNAVAILABLE`；
3. **`live` 但端点缺失 / 非法**：缺失时保持显式不可用，非法时返回
   `HAVEN_MCP_ENDPOINT_INVALID` 且**不尝试连接**（不猜、不扫描、不回退）。

**它不会返回任何编造的数据。** 另外：9 个工具里只有 3 个在 Haven 后端有用例，其余 6 个
**即使 live 通道接通也仍然**返回 `HAVEN_CAPABILITY_UNAVAILABLE`（那属于 A6，与传输无关）。

**未验收边界**：Codex / Claude Code / DSH / Pi 四个真实客户端、Windows WebView2 上的栖阅
设置界面、以及 Haven→Node→MCP 的真实端到端都**没有实测过**——本包的连接用例用的是进程内
假 transport，不是真的 Rust Broker。**单元与集成测试通过不等于生产实测。**

### 9 个工具与本版本实现状态

| 工具 | Haven 侧实现 |
| --- | --- |
| `get_system_capabilities` | ✅ 已实现（本地 + 桥接状态） |
| `get_settings_snapshot` | ✅ 已实现（`settings_read`） |
| `propose_settings_patch` | ✅ 已实现（`settings_proposal`） |
| `get_setting_sources` | ❌ 后端无用例 → `HAVEN_CAPABILITY_UNAVAILABLE` |
| `get_resource_preference_snapshot` | ❌ 后端无用例 |
| `get_library_summary` | ❌ `library_summary_read` 能力未实现 |
| `get_media_capabilities` | ❌ 后端无用例 |
| `get_onboarding_state` | ❌ 后端无用例 |
| `propose_resource_preference_patch` | ❌ 缺少 Agent 作用域入口 |

工具清单是**冻结集合**（`src/constants.ts` 的 `TOOL_NAMES`）。新增工具必须先改架构文档；
`test/tools.test.ts` 断言注册结果与它完全相等，因此"顺手多加一个 write 工具"会直接失败。

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

```bash
npm install
npm run build
node dist/index.js          # stdio transport
```

MCP 客户端配置（不填 `env` 也能启动，但那样所有工具都会如实报告桥接不可用）：

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
且需要你先在那里显式开启）；不要手写、不要猜。这就是栖阅界面为四个客户端
（Codex / Claude Code / DSH / Pi）生成的同一份模板，四者内容完全相同。

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

本轮（2026-09-18 工作树）结果：**86 passed / 1 skipped**。那 1 个 skipped 是 `dist/` **已**
构建时才跳过的占位提示用例（`dist 缺失时请先运行 npm run build`），属于预期行为。

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
| 生产入口行为 + stdout 洁净（真实子进程） | `test/stdio-e2e.test.ts` |
| stdout 洁净、无 fs/SQL/Tauri 依赖、端口无写方法、`node:net` 仅 connect | `test/source-guards.test.ts` |
| 枚举与 patch 字段对生成 `wire.ts` 无漂移 | `test/wire-drift.test.ts` |

**这些是单元 / 集成测试，不是生产实测。** 真实客户端（Codex / Claude Code / DSH / Pi）、
真实 Windows WebView2 界面与 Haven→Node→MCP 端到端链路都没有验过。
