---
doc_id: architecture.ai-system
type: canonical
status: active
owner: architecture
visibility: public
source_of_truth: accepted-design-and-runtime-contracts
last_reviewed: 2026-10-04
review_after: 2027-01-04
---

# Haven AI 系统架构

本文维护 Haven 产品的 Provider、Proposal、MCP、Skill 与凭据边界。
它不是开发工具操作手册，也不以设计目标代替发布版的实际能力清单。
传输协议见 [MCP 外部 Agent 传输](MCP_EXTERNAL_AGENT_TRANSPORT.md)。

## 1. Proposal、批准与回执

模型只能读取受控上下文、返回结构化建议。改变用户设置的唯一授权路径是：
**用户在 Haven 界面逐项检查并批准 Rust 生成的 canonical digest**。

```text
读取 authoritative 上下文
  → contextId / contextHash / revision / 脱敏快照 / capabilities
  → Provider 结构化建议或 MCP 提案请求
  → Application 重新校验上下文
  → pending Proposal + canonical JSON + SHA-256 digest
  → Haven UI 展示作用域、before/after、digest 与过期时间
  → 用户批准 expectedDigest
  → 单一 UoW：digest 校验 + CAS + 一次性批准 token + 写入 + Receipt
```

- digest 由 Rust 生成；UI 只显示并回传它，不自行生成领域摘要。
- 创建 pending 提案不应用设置；拒绝、过期和上下文冲突不写目标状态。
- 批准绑定 proposal、digest、目标 revision 与一次性 token，不能被模型代替。
- Receipt 是可回读的审计事实，必须与实际 changed、revision 和 changes 自洽。
- 原始凭据、文件路径、正文与 Provider 报文不进入 Agent 上下文或审计投影。

实现入口为 Application 的 `agent_settings_ipc`、`setting_proposals` 服务，
Domain 的 `setting_proposal` 内核和 Tauri 的 `commands/agent.rs`。

### 1.1 资源级部分 patch

Agent 资源提案只存请求修改的字段，不复制合并后的完整偏好。
资源级 Agent patch 的顶层和字段级 `None` 均表示不触碰该部分；
普通完整资源偏好的覆盖语义不因此改变。

批准时在同一个 UoW 内重读 authoritative 偏好、检查 base revision、
合并部分 patch 并 CAS。任何校验失败都不能留下目标写入或回执。
Agent Receipt 的 before/after 是脱敏审计投影，不能成为完整设置快照。

## 2. 分层与依赖方向

```text
React UI
  → Feature Hook / Action
  → Feature API / Gateway
  → Typed HavenClient
  → Tauri Command
  → Application Service
  → Domain / Port
  → Infrastructure
```

组件不得直接 invoke、读取 SQLite 或 fetch Provider。Application 是用例编排点；
Infrastructure 实现受控 HTTP、存储和 keyring。生成 Wire 只能由 Rust 定义和
规范生成流程更新。

不开放将任意 prompt、tool、URL、SQL、文件路径或命令直接转发到模型或系统的入口。
新增能力必须同时审查契约、权限、上下文投影和实际消费者。

## 3. Provider 契约

Provider 可以返回有界解释和闭合的设置 patch，不能批准、Apply、
访问 Repository/UoW、创建 Receipt 或自行决定权限。

设置建议路径：

```text
启用的 profile + CredentialStore + selectedModelId
  → 模型目录中的同一 id，且显式 chat=Supported
  → authoritative 设置上下文
  → AiSettingsRecommendationPort
  → strict json_schema structured output
  → 验证 ReadingPatch 与有界 explanation
  → Application 创建 pending Proposal
```

调用前后必须绑定同一份 context id/hash/base revision。Provider 端口不能自己
创建提案；领域 digest 仍由 Haven 生成。Structured Output 要求闭合键集合、
合法枚举、长度和控制字符约束；缺失或未知键都拒绝，不从自由文本中猜测 JSON。

没有可用 profile、凭据、选中模型或显式 chat 能力时，不发模型请求，也不拿模板
冒充模型响应。错误只返回稳定、脱敏的 code/message/retryable。

## 4. 凭据与隐私

AI 凭据使用 Domain 的 scoped `CredentialRef`，真实 secret 只属于
CredentialStore。普通 settings、SQLite profile、TS 响应、localStorage、
日志、Proposal、Receipt 和 MCP 响应都不得保存或回显 API key。

Secret 在受控适配器调用栈内短暂暴露；Debug/Display 必须脱敏，释放时清零。
Provider profile 删除先验证 expected revision，再受控清理凭据，最后 CAS 删除
profile；凭据清理失败必须中止，不能留下不可管理的孤儿 secret。
跨凭据库与数据库的并发窗口不能冒充单条 SQL 事务：若凭据已删但 CAS 冲突，
profile 必须呈现可恢复的“未配置凭据”状态。

出站请求使用共享 URL、DNS、连接和响应策略，不允许 userinfo、非法 scheme、
DNS 重绑定或无界响应。每次请求校验目的地址并固定已验证解析结果；
重定向不能绕过地址策略。AI endpoint 采用自己的策略类别，不能继承其它来源的
测试例外或局域网授权。

## 5. MCP 边界

MCP 只提供两类业务能力：读取脱敏、有界上下文；创建 pending 提案。
它不是写入、批准或通用系统工具通道。

### 5.1 闭合工具清单

唯一工具名声明在 `mcp/haven-mcp/src/constants.ts`：

```text
get_system_capabilities
get_settings_snapshot
get_setting_sources
get_resource_preference_snapshot
get_library_summary
get_media_capabilities
get_onboarding_state
propose_settings_patch
propose_resource_preference_patch
```

工具不得包含 Apply、Approve、Reject、Delete、文件系统、SQL、Shell、
Secret、直接 IPC、metadata patch 或文件重命名。输入 schema 必须 strict；
文本与 structuredContent 同源，输出受单字段、列表和整体上限约束。

### 5.2 同一安全内核

```text
外部 MCP 客户端
  → Haven MCP strict tool
  → Typed Haven Agent Bridge
  → 本地 Rust Broker
  → Typed Application service
  → pending Proposal
  → 用户在 Haven UI 批准 → CAS/Apply → Receipt
```

MCP server 不读取数据库、文件或凭据库，也不 invoke Tauri。
`get_system_capabilities` 在 Node 本地投影桥接状态；其余八个业务请求经过 Broker。
能力由 Haven 逐请求校验，客户端身份、模型输出和 Skill 都不是权限来源。

外部接入默认关闭。未配置 live bridge 或端点不可用时明确报告
`HAVEN_BRIDGE_UNAVAILABLE`；服务端没有开放能力时报告
`HAVEN_CAPABILITY_UNAVAILABLE`。两者不得混淆。
非法端点不尝试连接；生产入口不注入 fixture bridge，也不返回测试数据。

## 6. Skill 边界

Skill 是说明性行为协议，不是权限。它不能改变能力位、ACL、凭据可达性、
MCP 工具集合或批准路径。

### 6.1 同一份产品内容

`skills/haven-agent-proposal/SKILL.md` 是内置运行时与外部分发的同一份内容源。
产品 Skill 引导先读取能力和上下文，再提出 pending Proposal；
不能声称提案已应用，也不能通过 MCP 回读本来没有开放的 Receipt。

### 6.2 原生加载与状态

内置文档经编译期嵌入、严格解析后进入 BuiltinAgentSkillRegistry。
Application 的 AgentSkillService 管理 list/set_enabled；
SQLite 的 agent_skill_states 是启用状态的事实源，不使用页面状态替代。

只有启用且 instructions_hash 与当前原生投影一致的技能生效。
摘要变化时状态为 stale，需要用户重新确认；禁用是幂等状态更新，不是删除。
IPC 只引用 skill id，不接受任意路径或远程正文。

### 6.3 外部分发

技能 zip 与源码内容逐字节一致，manifest 标明版本、文件摘要及面向的工具清单。
技能包安装由用户管理；Haven 不写客户端 Skill 目录。
用户主动触发的 MCP 连接配置是另一项独立能力，其窄化边界见传输文档。

### 6.4 预览与生产

浏览器 Mock 和开发预览必须明确标识，不得替代真实持久化、模型请求或能力。
生产技能启用经 typed IPC 写入权威状态；建议生成消费同一份启用记录。

### 6.5 受众投影

```text
<!-- haven:audience=native --> … <!-- /haven:audience -->
<!-- haven:audience=external --> … <!-- /haven:audience -->
```

标记外正文共用。frontmatter 是元数据，不进入正文投影或 instructions_hash。
未知受众、嵌套、未闭合、多余闭合或非规范标记均在加载时拒绝。
原生投影不含外部 MCP 工具说明，外部分发保留整份文件。

原生模型没有工具，只返回 Provider 的 camelCase structured output；
外部 MCP patch 使用自己的 snake_case 契约。投影只转换说明文字，不转换字段协议。
摘要绑定原生投影：改共用/原生正文使启用失效，仅改元数据或外部正文不使其失效。

### 6.6 请求消费者

AiProviderProfileService 解析当前启用技能，将原生投影交给设置建议适配器的
system 消息。前端只提交 profileId、用户目标和上下文锚点，不推断模型或拼接
权威设置。无选中配置时失败；生产路径不能静默退回 Mock 模板。

## 7. 模型与能力

模型目录只来自 Provider 的兼容 models 响应，不能内置猜测候选名。
能力为 supported/unsupported/unknown；缺字段即 unknown，不从模型名猜
chat、vision 或 embedding。空目录、未配置和禁用各有明确状态；
网络或非法响应返回稳定错误，不伪造目录。

Settings Registry、Wire、Binding、持久化和运行时消费者必须指向同一份能力。
页面视觉完整与编译成功都不能单独建立产品兼容性承诺。

## 8. 事件与安全回归

AgentTracePort 使用闭合事件、服务端 sequence、上下文关联和有界 collector。
事件不是 Provider 原文、聊天历史或权限通道；不得附带 secret、路径、SQL 或正文。

相关测试分别覆盖 canonical digest、批准/CAS/Receipt、资源部分 patch、
凭据脱敏、模型能力、命令/ACL/Wire 一致性、MCP strict schema 与取消、
Skill 解析/投影/摘要及其真实请求消费者。
自动化、交互运行和发行产物验证的范围分别记录，不能互相替代。
