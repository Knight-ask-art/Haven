# Claude Code 子代理工作流

这份文档说明本项目如何把 Claude Code 当作实现、测试和复审子代理使用。
它不是 AI Provider 或 Haven MCP 的权限说明；安全边界仍以
[`AI_SYSTEM.md`](./AI_SYSTEM.md) 为准。

## 1. 默认前提

用户明确要求“使用 Claude Code”时，默认本机已经配置好了第三方模型与调用入口，
不要重新设计 Provider，也不要把 Anthropic OAuth 登录当作前置步骤。只需要确认命令
存在：

```bash
command -v claude
claude --version
```

PowerShell 中相应使用 `Get-Command claude`。

`Not logged in` 只说明当前命令走到了默认登录提示，不等于 Claude Code 未安装，
也不等于第三方模型不可用；不要擅自执行 `/login` 或改写 Provider 配置。需要确认调用是否
真的可用时，发送一个不含项目内容的最小探针，例如要求模型只回复固定字符串；探针失败时，
检查当前 shell 是否加载了既有 Provider 配置，而不是把它当作代码问题。不要把 API Key、
Token、endpoint 密钥或完整环境变量写进 prompt、日志、文档和 Git。

## 2. 每次派发必须给出的边界

不要只说“请看看项目”。Prompt 至少写清楚：

```text
Objective: 要完成的具体目标
Allowed Paths: 可以读取/修改的路径
Forbidden Paths: 禁止触碰的路径、文件和动作
Acceptance: 可观察的完成条件
Tests: 必须运行的测试或检查
Evidence: 需要返回的 diff、测试和未验证边界
```

主代理负责规格、安全边界、最终 diff、测试和 Git；Claude Code 的“完成”、状态消息
或测试描述都不是验收证据。

## 3. 只读复审

默认使用计划模式，不写仓库、不改 Git。下面是 Git Bash/FastCtx shell 的一次性复审命令：

```bash
claude --print --output-format text --no-session-persistence \
  --permission-mode plan --permission-prompts none \
  --effort low \
  "Objective: 只读复审指定范围。Allowed Paths: ...。Forbidden Paths: 写文件、Git 写操作、docs/reviews/。Acceptance: 返回按优先级排列的具体问题，并给出文件/行号证据。Tests: 只运行明确允许的检查。Evidence: 只报告实际观察到的内容。"
```

本项目使用第三方 Provider 时不要加 `--restricted`：该选项会忽略 user/project/local
settings，可能连同已经配置好的 Provider 一起屏蔽；它还会移除 Bash、PowerShell、REPL
和 WebFetch，复审者无法取得完整 Git/测试证据。`plan` 与 `permission-prompts none`
已经提供本次只读复审所需的权限边界。若任务要求连 Claude Code 自己的计划文件也不能写，
要把对应计划路径列入 Forbidden Paths。

该命令带有 `--no-session-persistence`，是一次性复审，不能用 `--continue` 或
`--resume` 续接；后续追问应开新会话并带上复审摘要。复审结果必须与主代理重新读取的源码、
diff 和测试结果分开记录；没有真实运行证据时，
不能把“审查通过”写成桌面端、真实客户端或端到端链路已经验收。

## 4. 编写代码和测试

只有用户明确授权实现时才进入写入模式。优先在隔离 worktree 中工作，先让 Claude Code
说明计划，再按允许路径实现和补测试。不要使用
`--dangerously-skip-permissions`，不要让 Claude Code 自行 `git add`、`commit`、`push`、
`reset`、`clean` 或删除用户文件。

实现结束后主代理必须独立完成：

1. 读取 `git status`、完整 diff 和 `git diff --check`；
2. 核对修改没有越过 Allowed Paths，也没有覆盖用户已有修改；
3. 重新运行对应测试、类型检查、构建和契约/生成物一致性检查；
4. 分开报告实现证据、测试证据、独立复审证据和真实运行证据。

## 5. 会话复用与上下文切换

- 同一项工作尽量继续同一个 Claude Code 会话，使用 `claude --continue` 或
  `claude --resume <session-id>`，并保持同一个 worktree。
- 上下文窗口超过 50% 时，不继续堆新任务：先让旧会话输出交接摘要（目标、已改文件、
  已跑测试、风险、下一步），然后开新会话，把摘要和原来的 Allowed/Forbidden Paths 一起传入。
- 多个相互独立的子任务才开多个 Claude Code 会话；不要让多个会话同时写同一批文件。
- 新会话不能只依赖旧会话的口头“已完成”，必须重新检查工作树和实际 diff。

## 6. Claude Code 与 Haven MCP 的区别

Claude Code 是开发子代理；Haven MCP 是外部 Agent 访问栖阅的受控接口，二者不是同一层。
接入 Haven MCP 时仍只允许读取脱敏上下文和创建 pending Proposal，不存在 Apply、Approve、
Reject、文件系统、SQL 或 Secret 工具。真正写入必须回到 Haven UI 的 Diff → 用户批准 →
Rust CAS → Receipt 链路。MCP 的真实客户端和端到端状态以
[`MCP_EXTERNAL_AGENT_TRANSPORT.md`](./MCP_EXTERNAL_AGENT_TRANSPORT.md) 为准，不能因为
Claude Code 本身能运行就声称 Haven MCP 已经完成真实验收。
