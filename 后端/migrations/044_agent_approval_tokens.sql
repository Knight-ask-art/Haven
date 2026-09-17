-- 044_agent_approval_tokens: Agent 一次性审批令牌的摘要与签发时间。
--
-- 043 已经落库的绑定全部从 NULL token 开始；明文永不进入 SQLite。
-- SQLite 无法为已有表追加完整的跨列不变量，故重建表：
-- - hash 与 issued_at 必须同时存在或同时为空；
-- - 终态不得携带 token 材料；
-- - hash 必须是 64 位小写十六进制 SHA-256；
-- - issued_at 必须落在 [created_at, expires_at) 内。

CREATE TABLE agent_action_bindings_new (
    proposal_id             TEXT PRIMARY KEY REFERENCES setting_proposals(id) ON DELETE RESTRICT,
    session_id              TEXT NOT NULL,
    request_id              TEXT NOT NULL,
    context_snapshot_id     TEXT NOT NULL,
    context_hash            TEXT NOT NULL CHECK (
        length(context_hash) = 64
        AND context_hash NOT GLOB '*[^0-9a-f]*'
    ),
    subject_json            TEXT NOT NULL,
    action_kind             TEXT NOT NULL CHECK (action_kind IN ('settings_proposal')),
    created_at              INTEGER NOT NULL CHECK (created_at >= 0),
    expires_at              INTEGER NOT NULL CHECK (expires_at > created_at),
    approval_state          TEXT NOT NULL CHECK (
        approval_state IN ('pending', 'applied', 'rejected', 'expired')
    ),
    approval_token_hash     TEXT CHECK (
        approval_token_hash IS NULL
        OR (
            length(approval_token_hash) = 64
            AND approval_token_hash NOT GLOB '*[^0-9a-f]*'
        )
    ),
    approval_token_issued_at INTEGER CHECK (
        approval_token_issued_at IS NULL
        OR (
            approval_token_issued_at >= created_at
            AND approval_token_issued_at < expires_at
            AND approval_state = 'pending'
        )
    ),
    updated_at              INTEGER NOT NULL CHECK (updated_at >= 0),
    CHECK (
        (approval_token_hash IS NULL) = (approval_token_issued_at IS NULL)
    ),
    CHECK (
        approval_state = 'pending'
        OR (approval_token_hash IS NULL AND approval_token_issued_at IS NULL)
    )
);

INSERT INTO agent_action_bindings_new (
    proposal_id, session_id, request_id, context_snapshot_id, context_hash,
    subject_json, action_kind, created_at, expires_at, approval_state,
    approval_token_hash, approval_token_issued_at, updated_at
)
SELECT
    proposal_id, session_id, request_id, context_snapshot_id, context_hash,
    subject_json, action_kind, created_at, expires_at, approval_state,
    NULL, NULL, updated_at
FROM agent_action_bindings;

DROP TABLE agent_action_bindings;
ALTER TABLE agent_action_bindings_new RENAME TO agent_action_bindings;

CREATE INDEX idx_agent_action_bindings_session_created
    ON agent_action_bindings(session_id, created_at);
