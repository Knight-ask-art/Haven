-- 043_agent_action_bindings: Agent 动作与设置提案的最小持久化绑定。
--
-- 只保存生命周期/归属元数据：不保存上下文正文、模型参数、Provider endpoint、
-- 凭据、SQL 或本地文件路径。proposal_id 是唯一主键，并且禁止级联删除，避免
-- Agent 审计绑定在提案历史被改写时静默消失。

CREATE TABLE IF NOT EXISTS agent_action_bindings (
    proposal_id          TEXT PRIMARY KEY REFERENCES setting_proposals(id) ON DELETE RESTRICT,
    session_id           TEXT NOT NULL,
    request_id           TEXT NOT NULL,
    context_snapshot_id  TEXT NOT NULL,
    context_hash         TEXT NOT NULL CHECK (length(context_hash) = 64),
    subject_json         TEXT NOT NULL,
    action_kind          TEXT NOT NULL CHECK (action_kind IN ('settings_proposal')),
    created_at           INTEGER NOT NULL CHECK (created_at >= 0),
    expires_at           INTEGER NOT NULL CHECK (expires_at > created_at),
    approval_state       TEXT NOT NULL CHECK (
        approval_state IN ('pending', 'applied', 'rejected', 'expired')
    ),
    updated_at           INTEGER NOT NULL CHECK (updated_at >= 0)
);

CREATE INDEX IF NOT EXISTS idx_agent_action_bindings_session_created
    ON agent_action_bindings(session_id, created_at);
