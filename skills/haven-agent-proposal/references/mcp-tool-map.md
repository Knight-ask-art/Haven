# MCP 工具映射与字段参考

本文件是 `SKILL.md` 的参考附件：需要精确字段名、错误码含义或当前接通状态时查阅。

> 命名说明：按 skill-creator 规范，技能包里不放置 `README.md` 这类"关于技能本身的说明文档"。
> 这份文件是**被引用的参考材料**，因此使用有语义的文件名。

## 目录

- [1. 9 个工具的映射](#1-9-个工具的映射)
- [2. 当前接通状态](#2-当前接通状态)
- [3. 读取工具返回什么](#3-读取工具返回什么)
- [4. patch 字段完整清单](#4-patch-字段完整清单)
- [5. 错误码含义](#5-错误码含义)
- [6. 本技能永远不会调用的东西](#6-本技能永远不会调用的东西)

---

## 1. 9 个工具的映射

工具清单是**冻结集合**（MCP 侧 `TOOL_NAMES`，`test/tools.test.ts` 断言注册结果与它完全相等）。

| 工具 | 类型 | 本技能什么时候用 |
| --- | --- | --- |
| `get_system_capabilities` | 只读 | **永远第一个调用**。判断桥接状态、能力位、各工具是否 implemented。 |
| `get_settings_snapshot` | 只读 | 改全局阅读设置之前读当前档位 + 取锚点。 |
| `get_setting_sources` | 只读 | 用户问"为什么这个值没生效""哪一层覆盖了它"。 |
| `get_resource_preference_snapshot` | 只读 | 改某本书/某漫画的排版之前读当前值 + 取锚点。 |
| `get_library_summary` | 只读 | 用户问库里有什么、有多少、最近在看什么。 |
| `get_media_capabilities` | 只读 | 用户问某个条目能不能读正文 / 渲染页面（**声明式**能力，不是运行时探测）。 |
| `get_onboarding_state` | 只读 | 用户问"我还没配置过，下一步做什么"。 |
| `propose_settings_patch` | 提案 | 用户要改**全局阅读设置**。只创建 pending 提案。 |
| `propose_resource_preference_patch` | 提案 | 用户要改**单个版本 / 条目**的排版。只创建 pending 提案。 |

命名是 `snake_case`；输入 schema 是 **strict** 的——多传一个字段整个调用就会失败，
不会被静默忽略。这不是苛刻，是刻意的：静默忽略 `apply: true` 会让调用方以为自己
的意图被接受了。

## 2. 当前接通状态

（截至 2026-09-20；`get_system_capabilities` 的 `tool_implementation` 是权威来源，
本表只是它的快照，**不要**把它当成比工具返回值更可信的东西。）

| 工具 | Haven 侧实现 | 不可用时你会看到 |
| --- | --- | --- |
| `get_system_capabilities` | ✅ | ——（永远可用） |
| `get_settings_snapshot` | ✅ | `HAVEN_BRIDGE_UNAVAILABLE`（桥接未接通时） |
| `propose_settings_patch` | ✅ | 同上 |
| `get_setting_sources` | ✅ | `HAVEN_BRIDGE_UNAVAILABLE`（桥接未接通时） |
| `get_resource_preference_snapshot` | ✅ | `HAVEN_BRIDGE_UNAVAILABLE`（桥接未接通时） |
| `get_library_summary` | ✅ | `HAVEN_BRIDGE_UNAVAILABLE`（桥接未接通时） |
| `get_media_capabilities` | ✅ | `HAVEN_BRIDGE_UNAVAILABLE`（桥接未接通时） |
| `get_onboarding_state` | ✅ | `HAVEN_BRIDGE_UNAVAILABLE`（桥接未接通时） |
| `propose_resource_preference_patch` | ✅ | `HAVEN_BRIDGE_UNAVAILABLE`（桥接未接通时） |

另外：**live 运行时传输已经实现，但仍需用户显式开启 Haven Broker 并配置端点**。当前生产默认是显式不可用桥接，因此即使上表里
"✅"的工具，在没有接通桥接的环境里也会返回 `HAVEN_BRIDGE_UNAVAILABLE`。
这不是故障，是本版本的已知边界——如实告诉用户，不要用别的方式凑答案。

## 3. 读取工具返回什么

### `get_settings_snapshot`

```jsonc
{
  "schema_version": 1,
  "subject_scope": "global",
  "section": "reading",
  "context_id": "…",          // 锚点：逐字回传
  "context_hash": "…",        // 锚点：64 位小写十六进制，逐字回传
  "revision": "…|null",       // 锚点：从未保存过时为 null
  "settings": { /* 见下方字段清单 */ },
  "redacted_fields": []       // 非空时这些字段被服务端清空，不要在 UI 里当作"未设置"
}
```

### `get_system_capabilities`

```jsonc
{
  "schema_version": 1,
  "mcp_server": { "name", "version", "tool_count", "tools": [...] },
  "bridge": { "kind": "unavailable|fixture|live", "available": bool, "detail": "…" },
  "haven": {
    "available": bool,
    "agent_api_version": 1,
    "capabilities": {
      "settings_read": bool, "settings_proposal": bool,
      "setting_sources_read": bool, "resource_preference_read": bool,
      "resource_preference_proposal": bool, "library_summary_read": bool,
      "media_capabilities_read": bool, "onboarding_read": bool,
      "metadata_proposal": bool,
      "rename_proposal": bool, "secret_read": bool, "filesystem_write": bool
    },
    "reason": "HAVEN_BRIDGE_UNAVAILABLE|null"
  },
  "tool_implementation": { "<tool_name>": "implemented|not_implemented" },
  "forbidden_operations": ["apply", "approve", "reject", "write", "delete", "rename",
                           "filesystem", "shell", "sql", "secret_read",
                           "arbitrary_prompt", "arbitrary_command"]
}
```

`capabilities` 里恒为 `false` 的四位（`metadata_proposal` / `rename_proposal` /
`secret_read` / `filesystem_write`）不是"暂时关着"，而是**本版本明确不实现**；
Haven 甚至会拒绝任何声明它们的能力清单。

## 4. patch 字段完整清单

取值是**闭合枚举**，不在列表里的值会让调用失败。

### 阅读排版（`propose_settings_patch.patch`，以及资源级提案的 `patch.reading`）

| 字段 | 取值 | 说明 |
| --- | --- | --- |
| `font_family` | `sans` `serif` `kai` `heiti` `fangsong` `mianfei` `custom` | 字体族档位 |
| `custom_font_family` | 字符串（≤200） | 仅当 `font_family='custom'` 有意义 |
| `font_size` | `small` `medium` `large` | 字号 |
| `line_height` | `compact` `comfortable` `airy` | 行高 |
| `content_width` | `narrow` `medium` `wide` | 正文宽度 |
| `theme` | `system` `paper` `warm` `slate` `dark` `sepia` `eyeCare` `custom` | 注意 `eyeCare` 是 camelCase |
| `custom_background` | 字符串（≤200） | 仅当 `theme='custom'` 有意义 |
| `custom_text` | 字符串（≤200） | 仅当 `theme='custom'` 有意义 |
| `font_weight` | `light` `regular` `medium` `semibold` `bold` | 字重 |
| `letter_spacing` | `tight` `normal` `relaxed` `loose` | 字距 |
| `system_auto` | 布尔 | 是否跟随系统排版 |
| `pagination` | `scroll` `paginated` `double` | 翻页模式 |

### 漫画排版（资源级提案的 `patch.comic`）

| 字段 | 取值 |
| --- | --- |
| `view_mode` | `single` `double` `strip` |
| `direction` | `rtl` `ltr` |
| `page_gap` | `zero` `twelve` `twenty_four` |
| `preload_pages` | `one` `three` `five` `unlimited` |

### 提案输入的整体形状

```jsonc
// 全局
{ "section": "reading",
  "context_id": "…", "context_hash": "…", "base_revision": "…|null",
  "patch": { "font_size": "large" },
  "response_format": "json|markdown" }

// 资源级（需要先读取同一作用域的快照）
{ "target_scope": "edition|media_item",
  "edition_id": "…", "media_item_id": "…|null",
  "context_id": "…", "context_hash": "…", "base_revision": "…|null",
  "patch": { "reading": {...}, "comic": {...} },
  "response_format": "json|markdown" }
```

约束：`patch` 至少一个非 null 字段；`target_scope='media_item'` 时 `media_item_id` 必填，
`='edition'` 时必须为 `null`；一条提案最多 12 个键。

资源级 `patch` 只表示本次请求的部分变更，不是 authoritative `PreferenceData` 的完整副本。
Haven 批准时会在同一 CAS 事务中重读并合并；未出现在 patch 中的字段不会被清除或覆盖。
因此 Agent 不应为了补全 Diff 而复制快照中已脱敏的路径、endpoint 或凭据样式文本。

### 提案返回的形状

```jsonc
{ "schema_version": 1,
  "proposal_id": "…",
  "status": "pending",          // 只有这一种取值：提案工具不产生别的状态
  "digest": "…64 位小写十六进制…",
  "target_label": "…",
  "subject_scope": "global|edition|media_item",
  "section": "reading|null", "edition_id": "…|null", "media_item_id": "…|null",
  "base_revision": "…|null",
  "created_at": "RFC3339", "expires_at": "RFC3339",
  "changes": [ { "key": "reading.fontSize", "before": "medium", "after": "large" } ] }
```

返回里**没有** approval token、secret、credentialRef、绝对路径或 SQL —— 不是"没打印"，
是结构里根本没有这些位置。

Agent Receipt（只在 Haven UI/内部 typed service 内部产生）使用脱敏的 before/after 审计投影；
authoritative 设置仍保存真实值，MCP v1 不暴露 Receipt 读取或 Apply 工具。

## 5. 错误码含义

| 错误码 | 含义 | 该怎么做 |
| --- | --- | --- |
| `HAVEN_BRIDGE_UNAVAILABLE` | Haven 运行时桥接没接通（默认关闭、端点未配置或 Haven 未运行） | 如实说明 + 给手动路径；确认 Broker 已开启且端点配置正确后再试。 |
| `HAVEN_CAPABILITY_UNAVAILABLE` | 当前 Haven 版本没有开放该能力 | 如实说明。**重试无用**，不要换工具凑近似答案。 |
| `HAVEN_BRIDGE_TIMEOUT` | 桥接超时 | 可重试；告知用户"应用可能正忙"。 |
| `HAVEN_BRIDGE_PROTOCOL_ERROR` | 桥接返回了不符合契约的载荷 | 视为实现缺陷，如实报告，不要解释成用户输入问题。 |
| `INVALID_ARGUMENT` | 输入不合法（字段名、取值、锚点格式） | 自己改正后重试一次；仍失败就把字段名报给用户。 |
| `HAVEN_RESPONSE_TOO_LARGE` | 响应超限且无法收缩 | 用更小的 `limit` 或更窄的条件重试。 |
| `SETTING_PROPOSAL_*` | Haven 提案内核的拒绝（例如上下文过期、digest 不匹配） | **零写入**。重新读取快照拿新锚点，再重新提议。 |
| `INTERNAL_ERROR` | 未归一化的内部错误 | 如实报告，不要编造原因。 |

错误响应**不包含** `structuredContent`，只有 `{"error": {code, message, retryable}}` 的文本。
一律以 `code` 为准来判断下一步，不要靠解析 message。

## 6. 本技能永远不会调用的东西

- 任何 `apply` / `approve` / `reject` / `write` / `delete` / `rename` 工具 —— **它们不存在**，
  MCP 侧对工具名跑禁用词根扫描，因此也不可能被加进来而不被发现。
- 任何文件系统、shell、SQL、secret 读取工具 —— 同上，不存在。
- 任何"把任意 prompt 或命令直通"的工具 —— 不存在。
- `metadata_patch` / `file_renames` / 批量文件操作 —— 不在协议里，也不在本技能的范围内。

因此遇到"帮我直接改掉"这类请求时，正确的回答不是"我做不到"，而是
**"我可以把改动整理成提案，但批准必须由你在界面上完成"** —— 并真的把提案准备好。
