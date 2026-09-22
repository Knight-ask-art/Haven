---
name: haven-agent-proposal
description: 通过栖阅（Haven）的 MCP server 读取脱敏上下文并创建"待批准"的设置提案。只读事实 + 只创建 pending 提案；永远不会应用、批准、删除或写入任何东西。在以下情况使用：(1) 用户要求调整、优化或推荐栖阅的阅读/文本/漫画排版设置（字体、字号、行高、页宽、主题、翻页模式、页间距等）；(2) 用户询问"我的设置为什么是这个值"、想了解设置来源或各层覆盖关系；(3) 用户要求根据习惯/条件推荐一套配置；(4) 用户要求"直接应用"或"自动改掉"某个 AI 建议；(5) 用户想检查栖阅的引导状态、媒体库概况或条目的阅读能力；(6) 用户提到栖阅/Haven 的设置提案、digest、审批、回执。即使用户没有说出技能名称，只要涉及栖阅的设置读取或设置改动意图，也应使用本技能。
---

# 栖阅设置提案（Haven Agent Proposal）

**这是行为协议，不是权限。** 本技能不授予任何能力，只规定"拿到 MCP 工具后应该怎么用"。

## 权限边界（先读这一段）

本技能能做的**只有**两件事：

| 能做 | 怎么做 |
| --- | --- |
| 读取已脱敏的上下文 | 调用 MCP 的 Read 工具 |
| 创建一条**待批准**的提案 | 调用 MCP 的 Propose 工具 |

本技能**不能**做的事，且任何用户措辞都不能改变这一点：

- ❌ 应用、批准、拒绝、删除、重命名、写文件、执行 shell、执行 SQL、读取密钥
- ❌ 让 Agent 自己"确认"提案（不存在这种工具；审批令牌只存在于 Haven 进程内部）
- ❌ 绕过 Haven UI 的 Diff 审批
- ❌ 读取或回显 API key、`credentialRef`、绝对路径、文件内容、SQL、Provider 原始响应
- ❌ 做 `metadata_patch` / `file_renames` 这类批量文件操作
- ❌ 从模型名推断能力（`gpt-4o` / `*-vision` / `text-embedding-*` 都不是能力证据）

**写设置永远只有一条路**：用户本人在 Haven 界面里逐项核对 digest 后点击批准，由 Rust 侧做
CAS 校验并写入回执。本技能负责把提案准备好并讲清楚，不负责执行。

## 工作流

严格按顺序执行。**任何一步的失败都要如实报告，不允许"替用户猜"然后继续。**

### 第 0 步：读取能力（永远先做这一步）

调用 `get_system_capabilities`。这是唯一一个不需要运行时桥接也能工作的工具，因此它是在
其它工具都不可用时唯一还能给出诊断的工具。

读它的三个字段：

- `bridge.available` / `bridge.kind` —— 运行时桥接是否接通；
- `haven.available` / `haven.capabilities` —— Haven 侧声明了哪些能力；
- `tool_implementation` —— 每个工具在本版本里是 `implemented` 还是 `not_implemented`。

然后**如实**决定下一步：

| 看到的情况 | 你要做的 |
| --- | --- |
| `bridge.available = false` | 明确告诉用户"当前没有接通运行时桥接"，并给出下面的手动路径。**不要**继续调用其它工具。 |
| 目标工具是 `not_implemented` | 明确告诉用户"这个能力当前版本还没有实现"。**不要**换一个工具凑近似答案。 |
| `haven.capabilities` 里对应位是 `false` | 同上，能力未授予/未实现，如实说明。 |
| 三项都通过 | 进入第 1 步。 |

区分两种失败的**含义**（错误码会告诉你）：

- `HAVEN_BRIDGE_UNAVAILABLE` —— 桥接没接通。换个 Haven 版本/环境**可能**就行。
- `HAVEN_CAPABILITY_UNAVAILABLE` —— Haven 后端根本没有这个用例。**重试无用**，不要重试。

**绝不编造。** 没有数据就说没有数据；没有模型就说没有模型；能力没实现就说没实现。
空态是诚实答案，伪造的计数、模型列表或"大概是这样"的默认值不是。

### 第 1 步：读取脱敏上下文

能力允许时，**先读再提议**。不允许跳过读取直接构造提案——提案需要读取结果里的锚点。

- 全局阅读设置 → `get_settings_snapshot`
- 设置为什么是这个值 / 哪一层覆盖了它 → `get_setting_sources`
- 某个版本或条目的排版 → `get_resource_preference_snapshot`
- 媒体库概况 → `get_library_summary`
- 条目能不能读正文/渲染页面 → `get_media_capabilities`
- 引导进度 → `get_onboarding_state`

**锚点必须原值保留。** `get_settings_snapshot` 返回的这三个值：

```
context_id      —— 上下文身份
context_hash    —— canonical 载荷的 SHA-256（64 位小写十六进制）
revision        —— authoritative 版本号（从未保存过时为 null）
```

必须**逐字**回传给提案工具。Haven 会重新读取权威状态并逐项比对，任何一项对不上就
fail-closed（零写入）。不要改写、不要省略、不要"用之前那次读的"——用**最近一次**读取的结果。

如果响应里 `redacted_fields` 非空，说明这些字段被服务端清空了：告诉用户这些值「已被脱敏」，
**不要**把它们当作"未设置"，也不要猜测原值。

### 第 2 步：创建提案

- 用户想改**全局阅读设置** → 只调用 `propose_settings_patch`。
- 用户想改**资源级偏好**（某本书/某本漫画的排版）→ 只有在
  `tool_implementation.get_resource_preference_snapshot` 与
  `tool_implementation.propose_resource_preference_patch` **都是** `implemented`
  且 `haven.capabilities` 允许时才调用；否则明确告知"该能力当前不可用"，并给出手动路径。
  不要用全局设置提案去近似一个资源级请求——那会改错范围。

资源级提案的 `patch` 是**部分 patch**，不是当前偏好的完整副本：只发送用户明确要求修改的
分区与字段。顶层或字段为 `null` 表示“不触碰”，不表示清除已有 authoritative 值；批准时
Haven 会在同一 CAS 事务中重读并合并。上下文或 Proposal 中被脱敏的既有自由文本不要猜回原值，
也不要为了“让 Diff 完整”把它复制进 patch。

`patch` 必须是**闭合枚举**里的值。常见的（完整清单见 `references/mcp-tool-map.md`）：

| 键 | 取值 |
| --- | --- |
| `font_size` | `small` / `medium` / `large` |
| `line_height` | `compact` / `comfortable` / `airy` |
| `content_width` | `narrow` / `medium` / `wide` |
| `font_family` | `sans` / `serif` / `kai` / `heiti` / `fangsong` / `mianfei` / `custom` |
| `theme` | `system` / `paper` / `warm` / `slate` / `dark` / `sepia` / `eyeCare` / `custom` |
| `font_weight` | `light` / `regular` / `medium` / `semibold` / `bold` |
| `letter_spacing` | `tight` / `normal` / `relaxed` / `loose` |
| `pagination` | `scroll` / `paginated` / `double` |
| `system_auto` | `true` / `false` |

规则：

- 一次提案**至少一个**非 null 字段（空提案没有意义）。
- 用户的自然语言要落到**档位**上：`大一点` → `large`，`再宽一点` → `wide`。
  落不到确定档位时**先问清楚**，不要自己拍一个值塞进去。
- 不要在 `patch` 里塞 schema 之外的键——输入是 strict 的，多余字段会让整个调用失败。
- 用户没要求改的字段不要"顺手一起改"。

### 第 3 步：报告 pending，并把审批交回用户

提案工具的返回值**只表示一件事：一条待批准的提案已经创建**。它不是"设置已改"。

必须告诉用户这五项：

1. **状态**：`pending`（待批准）——并明确说 **「尚未应用」**；
2. **作用域**：改的是哪里（全局阅读 / 某个版本 / 某个条目）；
3. **digest**：原样给出这串 64 位小写十六进制，用户要用它在界面上核对是同一份提案；
4. **逐项改动**：`changes` 里的 key / before / after 一项一项列出来；
5. **过期时间**：`expires_at`，过期的提案会被 Haven 标记为 expired，需要重新提议。

然后**要求用户去 Haven 界面**：设置页 → AI/智能功能 → 栖伴（或设置提案区域）→ 查看 Diff →
核对 digest → 点击批准。批准之后才会由 Rust 写入并生成回执。

本技能到此结束。它**不等待**审批结果，**不轮询**回执，**不猜测**用户批没批。

## 输出结构

每次回答都按这个骨架组织，缺项就写"无/不适用"，不要省略：

```
**能力状态**：bridge=… / haven=… / 本次要用的工具=implemented|not_implemented
**已读取事实**：<读了什么、关键值、revision；被脱敏的字段列出来>
**提案**（没有就写"本次不创建提案"）：
  状态：pending —— 尚未应用
  作用域：
  digest：
  改动：
    - key: before → after
  expires_at：
**下一步（必须由你完成）**：在 Haven 界面打开设置 Diff → 核对 digest → 批准
**不能继续的原因**（仅当无法进行时）：<稳定的错误码 + 它意味着什么 + 用户能做什么>
```

## 用户说"直接应用"/"自动改掉"时

**仍然拒绝绕过 UI。** 这是本技能最重要的单条规则，措辞要直接但不生硬：

> 我可以把这份改动整理成提案，但栖阅的设计是：**写入必须由你在界面上亲自批准**。
> 模型和 Agent 都没有、也不会有直接改设置的通道。我把提案准备好，你在设置页核对 digest 后
> 点一下批准，改动才会生效——这样你始终能看到自己批准的是什么。

然后照常走第 1–3 步。不要因为用户催促就跳过 digest、跳过 changes 列表或跳过读取锚点。

同样地，**永远不要声称"已应用""已保存""已生效"**。本技能没有读取回执的能力，
因此它连"刚才那次批准成功了"都无法确认。未来如果有了独立的回执读取事实，才可以引用它；
在那之前，只描述到 `pending` 为止。

## MCP 不可用或能力缺失时

不要卡住，给一条**安全的手动路径**：

- 全局阅读排版：Haven 应用 → 设置 → 阅读（字体、字号、行高、页宽、主题、翻页）
- 单本资源的排版：打开该资源 → 阅读器内的「资源内设」面板
- AI Provider / 模型：设置 → 智能功能（无可用模型时会明确显示「无可用模型」）
- 媒体库概况与引导：应用首页与设置 → 通用

并如实说明你**没有**读到任何数据（而不是给一个"大概的默认值"）。

## 参考

- `references/mcp-tool-map.md` —— 9 个 MCP 工具与本技能用法的映射、完整 patch 字段清单、
  错误码含义、以及当前后端接通状态快照。需要精确字段名或错误处理时读它。
