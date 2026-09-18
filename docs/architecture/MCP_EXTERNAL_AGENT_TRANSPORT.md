---
doc_id: architecture.mcp-external-agent-transport
type: canonical
status: active
owner: architecture
visibility: public
source_of_truth: accepted-design-and-runtime-contracts
last_reviewed: 2026-10-04
review_after: 2027-01-04
---

# MCP 外部 Agent 传输

本文定义 Haven 产品对外的本地 Broker 协议、安全边界和连接配置。
与 [AI 系统架构](AI_SYSTEM.md) 共用 Proposal、批准、CAS 与 Receipt 内核。
具体可用能力以运行时 capability manifest 为准。

## 1. 目标与非目标

用户已安装的 MCP 客户端通过 stdio 接入 Haven MCP server，再连接本机 Haven。
不同客户端使用同一套工具、schema、错误和授权语义，不提供客户端专属特权。

只允许读取脱敏上下文和创建 pending 提案。没有 Apply、Approve、Reject、
Receipt 回读、文件系统、SQL、Shell、Secret 或任意 prompt 直通工具。
本地 Broker 不开放 TCP、HTTP、WebSocket 或 Streamable HTTP 网络监听。
同一用户同时只有一个 Haven 实例提供 Broker。

## 2. 连接配置

用户先在 Haven 中主动开启外部接入，再配置端点。Broker 默认关闭：
不创建端点，不输出可复制端点或连接模板。只有真实 listening 状态和合法平台
端点才可生成配置；Mock 标识不能复制成真实端点。

Haven MCP 使用 `HAVEN_MCP_BRIDGE=live` 与 `HAVEN_MCP_ENDPOINT`。
手动源码构建可使用 Node + MCP 入口；安装包一键配置使用随包分发的运行时。
模板中的占位符不是已安装可执行路径。

端点缺失时保持显式不可用；端点非法时不尝试连接。
不扫描文件、枚举管道、探测端口或猜测其它目标。

## 3. 拓扑与信任边界

```text
外部客户端/模型（不可信）
  → MCP stdio（strict schema、闭合工具集合）
  → Haven MCP Node 进程
  → 校验后的平台本地端点
  → Rust Broker（OS ACL、握手、配额、逐请求能力校验）
  → Typed Application service
  → pending Proposal
  → Haven UI：用户检查 digest → 批准 → CAS/Apply → Receipt
```

Broker 的业务面比 Application 更窄。Application 中用于 UI 的批准、拒绝、
提案状态与回执读取方法没有投影到 Broker。

Node 仅使用 `node:net` 连接合法本地端点，不监听、不发 DNS、不连远程地址。
不得使用 http/https/dgram/tls、child_process、文件系统、SQLite、keyring 或
Tauri 模块扩展权限。

## 4. 端点与 OS 权限

| 平台 | 端点与校验 | 权限 |
| --- | --- | --- |
| Windows | Named Pipe 名称为 haven-agent-v1- 加八位小写十六进制；平台完整形态由 endpoint 模块校验，总长不超过 256 | DACL 仅创建者 SID；拒绝远程 pipe 客户端 |
| Unix | 当前用户运行目录下的 haven/agent-v1.sock，或 Haven 的固定每用户回退路径；必须精确匹配，字节长不超过 100 | 目录 0700，socket 与实例锁 0600 |

端点每用户稳定，名称不是秘密。ACL 隔离其它用户，不能隔离同用户恶意进程；
真正的业务约束是“只能读取有界投影与创建 pending 提案”。

Haven 管理的 Unix 目录组件不能是符号链接，最终目录必须为当前用户所有，
并通过目录 fd/no-follow 检查。绑定前设置受限权限，不留先创建再收紧的窗口。
第二实例不能接管、删除或改名另一个实例的端点。

## 5. Broker 协议

### 5.1 Framing

每帧为四字节大端长度 + UTF-8 单个 JSON 对象；拒绝尾随载荷和非法字段。

| 项 | 上限 |
| --- | --- |
| 帧载荷 | 256 KiB |
| 单请求 | 64 KiB |
| 单响应 | 256 KiB |
| 每连接在途请求 | 8 |
| 并发连接 | 8 |

超大帧直接断连。业务请求/响应超限返回对应稳定错误；
未知业务帧被拒绝，不能变成动态方法调用。

### 5.2 握手与身份

首帧必须为 `hello`，protocol_version 必须等于 1，不降级协商。
`hello.client` 的 name/version/instance_id 只供诊断，不参与授权和领域身份。
合法握手返回 welcome、Haven 版本、capabilities、session_id 和 granted_requests。

Rust 为每个连接生成 AgentSessionId，为每个合法请求生成 AgentRequestId。
客户端 frame id 只作连接内关联，不能用作领域 request id。
提案 payload 不接受客户端提供的 session_id/request_id。

协议不符、非法或不完整首帧在握手阶段收尾断连，释放连接 permit。
后续请求也必须重新校验当前服务端能力，不能只信 welcome 的快照。

### 5.3 业务请求集合

| type | 语义 |
| --- | --- |
| context | 设置上下文、hash、revision 与能力 |
| setting_sources | 设置来源与权威层 |
| resource_preference | 资源作用域偏好投影 |
| library_summary | 有界媒体库摘要 |
| media_capabilities | 媒体项能力摘要 |
| onboarding | 引导完成状态 |
| create_proposal | 全局 reading pending 提案 |
| create_resource_proposal | 资源级 reading pending 提案 |

granted_requests 恰好八项。MCP 的第九个工具 get_system_capabilities
在 Node 本地投影桥接状态。`cancel` 是控制帧，不是业务能力。

通用业务外壳为 `{type, id, payload}`。全局提案携带 section、
context_id/context_hash/base_revision 和闭合 reading patch。
未列出的动词与字段均拒绝，不增加 approve/apply/get_proposal/receipt 等帧。

### 5.4 响应与错误

成功为 `{type: "ok", id, payload}`；失败为
`{type: "error", id, error: {code, message, retryable}}`。
失败不含 payload、堆栈、路径、SQL、凭据或原始 Provider 报文。

客户端按稳定 code 判断，不解析 message。端点/协议/尺寸/并发/配额/超时
分别使用 HAVEN_MCP_ENDPOINT_INVALID、HAVEN_BROKER_PROTOCOL_MISMATCH、
HAVEN_BROKER_UNKNOWN_FRAME、HAVEN_BROKER_REQUEST_TOO_LARGE、
HAVEN_BROKER_RESPONSE_TOO_LARGE、HAVEN_BROKER_TOO_MANY_INFLIGHT、
HAVEN_BROKER_QUOTA_EXCEEDED 和 HAVEN_BROKER_TIMEOUT。
提案内核保留自己的稳定拒绝码。

### 5.5 取消、超时与重试

cancel 引用当前连接的在途 id。未知/已完成 id 幂等；
已进入内核并持久化的提案不因传输取消回滚。
客户端断连清理该连接的任务、permit 和在途计数，不影响其它连接。

| 阶段 | 上限 |
| --- | --- |
| 首帧握手 | 5 s |
| 单次写出 | 5 s |
| Broker 请求 | 20 s |
| MCP bridge 调用 | 15 s |
| 无在途请求的空闲连接 | 30 min |

握手或写出超时直接断连，不向已失效连接追写。
MCP 取消/超时的 AbortSignal 必须贯穿连接、握手、发送、响应等待和 socket 清理。

retryable 取决于副作用：只读失败可重试；提案在连接尚未建立时失败可重试；
提案已发出后的断连、取消或超时不得自动重试，因为提案可能已落库但回复丢失。
不能以“收到错误”推断“没有创建提案”。

## 6. 配额与数据投影

| 配额 | 上限 |
| --- | --- |
| 每连接在途请求 | 8 |
| 每连接提案创建尝试 | 8 |
| 每进程运行窗口提案创建尝试 | 32 |

提案尝试配额在进入 Application 前计入，失败不退款；重连不能退回进程计数。
进程重启重置窗口计数，因此它是噪音限制，不是授权边界。
pending 提案沿用内核的 24 小时 TTL。

资源提案只含部分 patch。批准时由 Haven 内部合并与 CAS；
外部客户端看不到 authoritative 路径、endpoint、凭据样式文本或 Receipt。
MCP 输出仍受单字段、列表和整份字符上限约束。

## 7. 生命周期与端点恢复

Haven 关闭或端点不可达时返回明确的 bridge-unavailable 状态，不换端点、
不回退到 fixture。MCP server 可继续回答能力状态，不伪造 Haven 数据。

Windows 使用首实例 pipe 语义避免抢占。Unix 使用 no-follow、当前用户拥有的
普通实例锁文件与 flock；锁持有到监听及 socket 收尾结束，锁文件本身不删除。
锁冲突报告 busy，其它失败不能冒充另一个活实例。

Unix bind 遇到固定端点占用时只对该端点作有界 liveness probe：
可连接、超时或不确定结果均保留原对象。仅明确 ECONNREFUSED/ENOENT 且
lstat 确认是 socket，才允许清理这一固定残留并重绑一次。
普通文件、目录和符号链接不得 chmod、删除或跟随。
同用户非 Haven 进程参与 probe/unlink 的竞态不由这把锁完全隔离。

status 只投影 disabled/listening/unavailable，不通过暗中探测宣称 busy。
busy 是 enable 尝试绑定时的具体失败。

## 8. 威胁边界

| 威胁 | 约束 |
| --- | --- |
| 其它用户或远程客户端 | OS 权限与拒绝远程 Named Pipe |
| 同用户恶意进程 | 有界投影、只创建 pending 提案、配额与 TTL；不能批准 |
| 伪造客户端身份或能力 | 自报身份不参与授权，服务端逐请求校验 |
| 超大帧、连接洪水或慢读写 | 闭合尺寸、并发、握手、写出和空闲限制 |
| 隐式端点选择 | 只接受显式平台端点，不扫描、不猜测 |
| 错误信息泄漏 | 稳定、脱敏且有界的错误投影 |
| 将本地协议扩展为网络服务 | 必须另行设计认证与评审，不能沿用本地 ACL |

同用户进程可能读取允许的投影并产生待审噪音。
因此不能把端点名称、客户端品牌、ACL 或 Skill 当作同用户身份认证。

## 9. 客户端与分发

Codex、Claude Code 等客户端消费同一个 MCP stdio 契约。
各自的配置语法不改变工具权限。用户接入说明见
[Haven MCP README](../../mcp/haven-mcp/README.md)；
客户端自己的文档定义其安装与配置方式。

技能包是单独的产品分发物，安装属于用户。连接配置不能推导出 Skill 自动安装权限。

### 9.1.1 用户主动配置 MCP 客户端

Haven 只接受闭合客户端 id，不接受任意路径、URL、配置片段、shell 或 API 凭据。
Windows 分发形态可向固定用户级 Codex TOML / Claude Code JSON 中的 Haven
连接项作结构化合并；其它平台的写入不由此获得授权。

- 保留其它 server、键和用户内容，写前备份，同目录暂存、flush、原子替换。
- 替换前重读并逐字节比较；原先不存在的目标不能覆盖另一个进程新建的文件。
- 环境变量重定向、符号链接、重解析点、异常结构、非 UTF-8 和超大文件拒绝。
- Windows 保留 DACL 与继承保护位，落字节前贴权限并核对；
  Unix 权限策略使用 0600，但不等于已开放其它平台的分发写入能力。
- runtimeReady 检查随包命令和入口实际存在；配置指向打包的 node.exe 与
  dist/index.js，不依赖全局 Node、源码 checkout 或 shell。
- status 只读，apply 由用户主动触发，重复配置幂等；失败不静默覆盖原内容。

最终内容核对到原子替换之间的跨进程竞态不是已消除的事务保证。
一键配置不改变 Broker 默认关闭，也不代替用户对提案的批准。

## 10. 实现与回归定位

协议/端点/会话/生命周期由 `src-tauri/src/agent_broker/` 维护；
Application 管理 typed 上下文和提案；
`mcp/haven-mcp/src/` 维护工具、live bridge、取消与错误。
客户端配置由 Application/Infrastructure 的 mcp_client_config 服务管理；
安装运行时由 tools/mcp 和 Tauri bundle.resources 组装。

回归覆盖闭合工具与帧集合、strict schema、能力投影、服务端身份、
OS 权限、默认关闭、尺寸/并发/配额、取消与重试语义、
端点恢复及客户端配置合并。协议 fixture、真实进程、桌面交互和发行产物
各自证明不同范围，不能混称客户端兼容性。
