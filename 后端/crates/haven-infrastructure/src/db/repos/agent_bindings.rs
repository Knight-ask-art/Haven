//! Agent 动作绑定的 SQLite 只读 Repository 与设置提案事务辅助。
//!
//! 绑定只保存最小元数据；创建和状态迁移由 `SqliteSettingProposalUow` 在同一
//! `BEGIN IMMEDIATE` 事务中调用本模块的辅助函数完成。

use std::sync::Arc;

use async_trait::async_trait;
use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::agent::{
    AgentActionBinding, AgentActionKind, AgentApprovalState, AgentApprovalTokenHash,
    AgentScopeSubject,
};
use haven_domain::contracts::AgentActionBindingRepository;
use haven_domain::ids::{
    AgentContextSnapshotId, AgentRequestId, AgentSessionId, SettingProposalId,
};
use haven_domain::setting_proposal::canonical_json_of;
use rusqlite::{Connection, OptionalExtension, Transaction};

use crate::db::Db;
use crate::db::repos::map_db_error;

const AGENT_BINDING_COLUMNS: &str = "proposal_id, session_id, request_id, context_snapshot_id, \
    context_hash, subject_json, action_kind, created_at, expires_at, approval_state, \
    approval_token_hash, approval_token_issued_at";

pub struct SqliteAgentActionBindingRepository {
    db: Arc<Db>,
}

impl SqliteAgentActionBindingRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl AgentActionBindingRepository for SqliteAgentActionBindingRepository {
    async fn get(
        &self,
        proposal_id: SettingProposalId,
    ) -> Result<Option<AgentActionBinding>, AppError> {
        let conn = self.db.lock();
        load_agent_action_binding(&conn, proposal_id)
    }
}

/// 在设置提案 UoW 内插入绑定；不单独开启事务。
pub(crate) fn insert_agent_action_binding(
    tx: &Transaction<'_>,
    binding: &AgentActionBinding,
) -> Result<(), AppError> {
    let subject_json = canonical_json_of(&binding.subject())?;
    tx.execute(
        "INSERT INTO agent_action_bindings
            (proposal_id, session_id, request_id, context_snapshot_id, context_hash,
             subject_json, action_kind, created_at, expires_at, approval_state,
             approval_token_hash, approval_token_issued_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, NULL, ?8)",
        rusqlite::params![
            binding.proposal_id().to_string(),
            binding.session_id().to_string(),
            binding.request_id().to_string(),
            binding.context_snapshot_id().to_string(),
            binding.context_hash(),
            subject_json,
            binding.action_kind().as_str(),
            binding.created_at().0,
            binding.expires_at().0,
            binding.approval_state().as_str(),
        ],
    )
    .map_err(|error| {
        if is_constraint_violation(&error) {
            AppError::new(
                "AGENT_ACTION_BINDING_ALREADY_EXISTS",
                ErrorKind::AlreadyExists,
                "Agent 动作绑定已存在，不能覆盖",
                false,
            )
            .with_source(error)
        } else {
            map_db_error("保存 Agent 动作绑定失败")(error)
        }
    })?;
    Ok(())
}

/// 在设置提案 UoW 内读取绑定。
pub(crate) fn load_agent_action_binding(
    conn: &Connection,
    proposal_id: SettingProposalId,
) -> Result<Option<AgentActionBinding>, AppError> {
    let row = conn
        .query_row(
            &format!(
                "SELECT {AGENT_BINDING_COLUMNS} FROM agent_action_bindings WHERE proposal_id = ?1"
            ),
            rusqlite::params![proposal_id.to_string()],
            read_agent_binding_row,
        )
        .optional()
        .map_err(map_db_error("查询 Agent 动作绑定失败"))?;
    row.map(decode_agent_binding).transpose()
}

pub(crate) fn load_agent_action_binding_tx(
    tx: &Transaction<'_>,
    proposal_id: SettingProposalId,
) -> Result<Option<AgentActionBinding>, AppError> {
    let row = tx
        .query_row(
            &format!(
                "SELECT {AGENT_BINDING_COLUMNS} FROM agent_action_bindings WHERE proposal_id = ?1"
            ),
            rusqlite::params![proposal_id.to_string()],
            read_agent_binding_row,
        )
        .optional()
        .map_err(|error| tx_error("查询 Agent 动作绑定失败", error))?;
    row.map(decode_agent_binding).transpose()
}

pub(crate) fn update_agent_action_binding_state(
    tx: &Transaction<'_>,
    proposal_id: SettingProposalId,
    expected: AgentApprovalState,
    next: AgentApprovalState,
    updated_at: UtcMillis,
) -> Result<(), AppError> {
    let current: Option<String> = tx
        .query_row(
            "SELECT approval_state FROM agent_action_bindings WHERE proposal_id = ?1",
            rusqlite::params![proposal_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| tx_error("读取 Agent 动作绑定状态失败", error))?;
    let Some(current) = current else {
        // 普通 UI 提案没有 Agent 绑定：保持 no-op。
        return Ok(());
    };
    let current = AgentApprovalState::parse(&current)
        .ok_or_else(|| binding_integrity("绑定状态不是闭合集合中的取值"))?;
    if current != expected {
        return Err(binding_integrity("绑定状态与预期状态不一致"));
    }
    current.validate_transition(next)?;
    let affected = if next.is_terminal() {
        tx.execute(
            "UPDATE agent_action_bindings
             SET approval_state = ?2, approval_token_hash = NULL,
                 approval_token_issued_at = NULL, updated_at = ?3
             WHERE proposal_id = ?1 AND approval_state = ?4",
            rusqlite::params![
                proposal_id.to_string(),
                next.as_str(),
                updated_at.0,
                expected.as_str(),
            ],
        )
    } else {
        tx.execute(
            "UPDATE agent_action_bindings
             SET approval_state = ?2, updated_at = ?3
             WHERE proposal_id = ?1 AND approval_state = ?4",
            rusqlite::params![
                proposal_id.to_string(),
                next.as_str(),
                updated_at.0,
                expected.as_str(),
            ],
        )
    }
    .map_err(|error| tx_error("更新 Agent 动作绑定状态失败", error))?;
    if affected != 1 {
        return Err(binding_integrity("绑定状态条件更新未命中"));
    }
    Ok(())
}

#[derive(Debug)]
struct AgentBindingRow {
    proposal_id: String,
    session_id: String,
    request_id: String,
    context_snapshot_id: String,
    context_hash: String,
    subject_json: String,
    action_kind: String,
    created_at: i64,
    expires_at: i64,
    approval_state: String,
    approval_token_hash: Option<String>,
    approval_token_issued_at: Option<i64>,
}

fn read_agent_binding_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentBindingRow> {
    Ok(AgentBindingRow {
        proposal_id: row.get(0)?,
        session_id: row.get(1)?,
        request_id: row.get(2)?,
        context_snapshot_id: row.get(3)?,
        context_hash: row.get(4)?,
        subject_json: row.get(5)?,
        action_kind: row.get(6)?,
        created_at: row.get(7)?,
        expires_at: row.get(8)?,
        approval_state: row.get(9)?,
        approval_token_hash: row.get(10)?,
        approval_token_issued_at: row.get(11)?,
    })
}

fn decode_agent_binding(row: AgentBindingRow) -> Result<AgentActionBinding, AppError> {
    // 旧行是内容 Subject 的扁平形状，新行可能是设置范围；两种都必须恢复成
    // 同一份 canonical 字节，因此这里不做任何"迁移式重写"。
    let subject: AgentScopeSubject = serde_json::from_str(&row.subject_json)
        .map_err(|_| binding_integrity("subject_json 无法解析"))?;
    if canonical_json_of(&subject)? != row.subject_json {
        return Err(binding_integrity("subject_json 不是 canonical 形式"));
    }
    let action_kind = AgentActionKind::parse(&row.action_kind)
        .ok_or_else(|| binding_integrity("动作类型不是闭合集合中的取值"))?;
    let approval_state = AgentApprovalState::parse(&row.approval_state)
        .ok_or_else(|| binding_integrity("审批状态不是闭合集合中的取值"))?;
    AgentActionBinding::from_stored_with_token(
        row.proposal_id
            .parse::<SettingProposalId>()
            .map_err(|_| binding_integrity("提案 ID 非法"))?,
        row.session_id
            .parse::<AgentSessionId>()
            .map_err(|_| binding_integrity("会话 ID 非法"))?,
        row.request_id
            .parse::<AgentRequestId>()
            .map_err(|_| binding_integrity("请求 ID 非法"))?,
        row.context_snapshot_id
            .parse::<AgentContextSnapshotId>()
            .map_err(|_| binding_integrity("上下文快照 ID 非法"))?,
        row.context_hash,
        subject,
        action_kind,
        UtcMillis(row.created_at),
        UtcMillis(row.expires_at),
        approval_state,
        row.approval_token_hash,
        row.approval_token_issued_at.map(UtcMillis),
    )
}

/// 为 pending Agent 绑定签发或重新签发令牌摘要。
pub(crate) fn issue_agent_action_approval_token(
    tx: &Transaction<'_>,
    proposal_id: SettingProposalId,
    expected_digest: &str,
    approval_token_hash: &AgentApprovalTokenHash,
    issued_at: UtcMillis,
) -> Result<bool, AppError> {
    let affected = tx
        .execute(
            "UPDATE agent_action_bindings
             SET approval_token_hash = ?3, approval_token_issued_at = ?4, updated_at = ?4
             WHERE proposal_id = ?1 AND approval_state = 'pending'
               AND EXISTS (
                   SELECT 1 FROM setting_proposals
                   WHERE id = ?1 AND status = 'pending' AND digest = ?2
               )",
            rusqlite::params![
                proposal_id.to_string(),
                expected_digest,
                approval_token_hash.as_str(),
                issued_at.0,
            ],
        )
        .map_err(|error| tx_error("签发 Agent 审批令牌失败", error))?;
    Ok(affected == 1)
}

/// 条件消费令牌摘要；成功后明文关联的持久化材料同时清除。
pub(crate) fn consume_agent_action_approval_token(
    tx: &Transaction<'_>,
    proposal_id: SettingProposalId,
    expected_digest: &str,
    approval_token_hash: &AgentApprovalTokenHash,
    consumed_at: UtcMillis,
) -> Result<bool, AppError> {
    let affected = tx
        .execute(
            "UPDATE agent_action_bindings
             SET approval_token_hash = NULL, approval_token_issued_at = NULL, updated_at = ?4
             WHERE proposal_id = ?1 AND approval_state = 'pending'
               AND approval_token_hash = ?3
               AND EXISTS (
                   SELECT 1 FROM setting_proposals
                   WHERE id = ?1 AND status = 'pending' AND digest = ?2
               )",
            rusqlite::params![
                proposal_id.to_string(),
                expected_digest,
                approval_token_hash.as_str(),
                consumed_at.0,
            ],
        )
        .map_err(|error| tx_error("消费 Agent 审批令牌失败", error))?;
    Ok(affected == 1)
}

fn binding_integrity(detail: &'static str) -> AppError {
    AppError::new(
        "AGENT_ACTION_BINDING_INTEGRITY",
        ErrorKind::Internal,
        format!("Agent 动作绑定完整性校验失败：{detail}"),
        false,
    )
}

fn tx_error(context: &'static str, error: rusqlite::Error) -> AppError {
    AppError::new("DATABASE_ERROR", ErrorKind::Database, context, true).with_source(error)
}

fn is_constraint_violation(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _)
            if matches!(
                failure.extended_code,
                rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY
                    | rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
            )
    )
}
