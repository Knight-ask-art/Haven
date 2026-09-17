# Agent Approval Token 阶段计划

## 目标

在 Proposal / Binding 安全内核之后，加入一次性 Approval Token，使 Agent 设置写入只能走：

```text
Agent Proposal
  → UI 展示 canonical digest / Diff
  → Haven 签发一次性 Approval Token
  → 带 digest + token 进入 CAS Apply
  → 同一 UoW 消费 token、写设置、写 Receipt
```

本阶段不接 Tauri IPC、前端 UI、MCP、Skill、Provider 或 Python Agent；只冻结并验证 Rust Application/SQLite 契约。

## 不变量

1. 原始 token 只在签发结果中返回一次；SQLite 只保存 SHA-256 hash，不保存明文。
2. token 绑定 Proposal ID、当前 digest、Binding 生命周期和 pending 状态；重新签发会使旧 token 失效。
3. Agent provenance 的 apply 必须携带匹配 token；只携带 digest 的 Agent apply 必须 fail-closed。
4. token 校验、目标 CAS、Receipt 插入、Proposal/Binding 状态迁移必须在同一个 `BEGIN IMMEDIATE` UoW 中完成。
5. applied/rejected/expired 终态清除 token；重复使用、错误 token、错误 digest、过期 token 均不得写设置。
6. 普通用户 Proposal 保持现有 digest 确认路径，不被 Agent token 要求污染。
7. token、凭据、上下文正文和 Provider 信息不得进入日志、provenance、Receipt 或 wire 载荷。

## 实现范围

- Domain：`AgentApprovalToken` 不透明值对象、Binding token hash/签发时间与终态清除规则。
- Application：签发 token、带 token apply、Agent 入口强制 token、错误码与内存回归测试。
- Infrastructure：044 migration、hash/签发时间持久化、同事务条件更新、篡改/重放/回滚测试。
- 文档：记录这一阶段的契约和下一阶段 Typed IPC/UI 所需输入。

## 验收命令

```text
cargo fmt --manifest-path 后端/Cargo.toml --all -- --check
cargo test --manifest-path 后端/Cargo.toml -p haven-domain
cargo test --manifest-path 后端/Cargo.toml -p haven-application
cargo test --manifest-path 后端/Cargo.toml -p haven-infrastructure
git diff --check
```

完成前还必须由只读 Claude Code 独立复审：token 不可绕过、明文不落库、重复 apply 零写入、migration/旧数据兼容和普通 Proposal 回归均无 BLOCKER/P1。

## 当前实现状态（2026-09-17）

- Domain 已实现不透明的 `AgentApprovalToken`、可持久化的 SHA-256 `AgentApprovalTokenHash`
  以及 Binding 的签发、绑定校验、重签发覆盖和终态清除规则；原始 token 不实现普通
  `Serialize`/`Deserialize`，`Debug`/`Display` 只显示脱敏值，并在释放时清零。
- Application 已实现 token 签发、带 token 的 Agent apply，以及 Agent digest-only apply 的
  `AGENT_APPROVAL_TOKEN_REQUIRED` fail-closed 保护。token 校验、目标 CAS、条件消费、回执
  写入和 Proposal/Binding 状态迁移收敛在同一个 UoW 事务内；普通用户 Proposal 仍保留
  digest-only 的确认与回执幂等路径。
- SQLite 已实现 `044_agent_approval_tokens` 迁移、只保存 hash 的持久化、同事务条件签发/消费、
  终态清除和回滚语义；旧的 043 行升级后 token 字段为空，并保留既有约束和索引。
- 回归覆盖重签发使旧 token 失效、错误 token 零写入、digest-only Agent apply 被拒绝、一次性
  消费、终态清除、晚期回执失败回滚和 043 → 044 旧数据升级。
- 本阶段仍不接入 Tauri IPC、前端 UI、MCP、Skill、Provider 或 Python Agent；这些边界将在
  后续 Typed IPC/UI 阶段按本计划的同一 Proposal/Approval/Receipt 规范接入。

## 独立复审记录（2026-09-17）

Claude Code 对本阶段做了只读静态复审，结论为 `PASS`，未发现 BLOCKER/P1；它没有运行测试、
没有修改文件，也没有执行 Git 操作。本地测试结果以本计划“验收证据”之外的实际命令输出为准。

复审保留以下后续边界，不把它们混入本阶段的“已解决”范围：

- P2：当前尚未接入 UI/IPC，因此 Application 的 token 签发方法本身还没有“用户已确认”的
  独立证明。下一阶段不得把 `create`、`issue`、`apply` 作为同一 Agent/Provider 可调用面
  暴露；UI 必须提供用户确认的 digest/一次性证明，服务端再签发或直接进入受控 apply。
- P2：044 对 `context_hash` 采用了比 043 更严格的闭合十六进制约束。对于 043 中已经存在的
  异常历史行，迁移会整体回滚并 fail-closed，当前没有把坏数据静默修复或降级读取；接入正式
  升级流程前应补充可运维的预检错误和坏行迁移测试。
- P3：新建绑定的插入端口当前只接受无 token 的 pending 绑定；若未来扩大其调用面，应显式
  拒绝带 token 的输入，避免内存对象与持久化行静默分叉。
- P3：低层的 `create_with_agent_binding` 仍应在未来收紧为固定的 Agent provenance，并让
  `context_hash` 只能从真实 `AgentContextSnapshot` 派生；当前受控的
  `AgentProposalService` 路径已经使用固定 provenance，但低层端口不应直接成为外部入口。
- P3：`AgentSubject` 对 Global 设置目标的覆盖是刻意设计（全局设置不属于作品），但它是
  最小权限上的范围外扩；若产品不接受，应在后续 capability manifest 中增加独立的全局设置
  提案能力位。
- P3：当前 token 使用两枚 UUID v7，约 148 bit 有效随机熵，满足本阶段至少 128 bit 的一次性
  票据要求；若产品策略升级为 256 bit，应在 UI/Provider 接入前改用系统 CSPRNG 直接填充
  32 字节并保留错误处理。

因此，本阶段出口是“Rust Proposal/Approval/Receipt 安全内核已具备、无 BLOCKER/P1”，而不是
“用户审批 UI、Typed IPC 或 Agent Provider 已可用”。
