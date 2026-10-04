//! 设置变更提案 SQLite Repository + UoW（V02-SETTING-PROPOSAL-001）。
//!
//! 职责边界：
//! - `SqliteSettingProposalRepository` 只做提案存取（create / get / get_receipt）。
//!   **不提供任何状态迁移或"直接执行"方法**：真正生效必须走 Application 的
//!   显式 `apply_confirmed` 事务路径。
//! - `SqliteSettingProposalUow` 把 apply 的全部步骤（重读提案 → 条件 CAS 写目标 →
//!   插回执 → pending → applied）放进**同一个** `BEGIN IMMEDIATE` 事务，复用既有
//!   Db 锁；任何一步失败都整体回滚，不会留下"目标已改但没有回执"的半成功状态。
//!   唯一例外是过期 pending 提案：事务只提交 `pending → expired`，不写目标、不插回执。
//!
//! 读取端不做"只信一列"的妥协：canonical JSON 与 digest 必须自洽（领域层重算），
//! 冗余 target 列必须与载荷一致；回执必须属于一条**真实存在且已 applied** 的提案，
//! 其 digest / target 要与父提案载荷交叉一致，`provenance_json` 自身也必须是
//! canonical JSON。任何一项不满足即返回稳定 `SETTING_PROPOSAL_INTEGRITY_MISMATCH`。
//! 反向同样成立：一条自称 applied 却读不出回执的提案是完整性错误，不是"未应用"。

use std::sync::Arc;

use async_trait::async_trait;
use haven_application::services::setting_proposals::{SettingProposalTxPorts, SettingProposalUoW};
use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::agent::{
    AgentActionBinding, AgentActionKind, AgentApprovalState, AgentApprovalTokenHash,
    AgentScopeSubject,
};
use haven_domain::contracts::{
    EditionPreference, MediaItemPreference, SettingProposalRepository, SettingsRow,
};
use haven_domain::ids::{EditionId, MediaItemId, SettingProposalId};
use haven_domain::setting_proposal::{
    ProvenanceActorKind, ProvenanceReason, ProvenanceSourceKind, SettingChangeReceipt,
    SettingProposal, SettingProposalStatus, SettingProvenance, SettingTarget, canonical_json_of,
    setting_proposal_integrity_error,
};
use haven_domain::settings::PreferenceData;
use rusqlite::{OptionalExtension, Transaction};

use crate::db::Db;
use crate::db::repos::agent_bindings::{
    consume_agent_action_approval_token, insert_agent_action_binding,
    issue_agent_action_approval_token, load_agent_action_binding, load_agent_action_binding_tx,
    update_agent_action_binding_state,
};
use crate::db::repos::map_db_error;

const PROPOSAL_COLUMNS: &str = "id, scope, section, edition_id, media_item_id, payload_json, \
     digest, status, created_at, expires_at";

/// 回执列 + 父提案的**载荷级**列：父提案靠 `from_stored` 重建，而不是只信它的冗余列，
/// 这样"两边列同样错"也会被载荷戳穿。
const RECEIPT_COLUMNS: &str = "r.id, r.proposal_id, r.proposal_digest, r.scope, r.section, \
     r.edition_id, r.media_item_id, r.before_json, r.after_json, r.applied_revision, r.changed, \
     r.provenance_json, r.applied_at, p.payload_json, p.digest, p.status, p.created_at";

pub struct SqliteSettingProposalRepository {
    db: Arc<Db>,
}

impl SqliteSettingProposalRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl SettingProposalRepository for SqliteSettingProposalRepository {
    async fn create(&self, proposal: &SettingProposal) -> Result<(), AppError> {
        if proposal.status() != SettingProposalStatus::Pending {
            return Err(AppError::new(
                "SETTING_PROPOSAL_INVALID_STATUS",
                ErrorKind::Validation,
                "只能创建 pending 状态的提案",
                false,
            ));
        }
        let conn = self.db.lock();
        let target = proposal.target();
        // 普通 INSERT：id 冲突是错误，绝不覆盖既有提案（覆盖会改写审计事实）。
        let result = conn.execute(
            "INSERT INTO setting_proposals
                (id, scope, section, edition_id, media_item_id, payload_json, digest, status,
                 created_at, expires_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?9)",
            rusqlite::params![
                proposal.id().to_string(),
                target.scope_str(),
                target.section().map(|section| section.as_str()),
                target.edition_id().map(|id| id.to_string()),
                target.media_item_id().map(|id| id.to_string()),
                proposal.canonical_json(),
                proposal.digest(),
                proposal.status().as_str(),
                proposal.created_at().0,
                proposal.expires_at().0,
            ],
        );
        match result {
            Ok(_) => Ok(()),
            Err(error) if is_constraint_violation(&error) => Err(AppError::new(
                "SETTING_PROPOSAL_ALREADY_EXISTS",
                ErrorKind::AlreadyExists,
                "提案已存在，不能覆盖",
                false,
            )
            .with_source(error)),
            Err(error) => Err(map_db_error("保存设置提案失败")(error)),
        }
    }

    async fn get(&self, id: SettingProposalId) -> Result<Option<SettingProposal>, AppError> {
        let conn = self.db.lock();
        let row = conn
            .query_row(
                &format!("SELECT {PROPOSAL_COLUMNS} FROM setting_proposals WHERE id = ?1"),
                rusqlite::params![id.to_string()],
                read_proposal_row,
            )
            .optional()
            .map_err(map_db_error("查询设置提案失败"))?;
        row.map(decode_proposal).transpose()
    }

    async fn get_receipt(
        &self,
        id: SettingProposalId,
    ) -> Result<Option<SettingChangeReceipt>, AppError> {
        let conn = self.db.lock();
        // 提案状态先读：`None` 的含义被契约钉死为"未应用"，因此一条自称 applied
        // 却拿不出回执的行不能从这里流成 `None`——那会把"存储被改写"报成"还没应用"，
        // 与 `apply_confirmed` 对同一状态的判定（完整性错误）正好相反。
        let status: Option<String> = conn
            .query_row(
                "SELECT status FROM setting_proposals WHERE id = ?1",
                rusqlite::params![id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_db_error("查询设置提案失败"))?;
        let Some(status) = status else {
            // 提案不存在：与 `get` 的 None 语义一致。
            return Ok(None);
        };
        let status = SettingProposalStatus::parse(&status)
            .ok_or_else(|| setting_proposal_integrity_error("提案状态不是闭合集合中的取值"))?;

        let row = conn
            .query_row(
                &format!(
                    "SELECT {RECEIPT_COLUMNS}
                     FROM setting_change_receipts r
                     JOIN setting_proposals p ON p.id = r.proposal_id
                     WHERE r.proposal_id = ?1"
                ),
                rusqlite::params![id.to_string()],
                read_receipt_row,
            )
            .optional()
            .map_err(map_db_error("查询设置变更回执失败"))?;
        match row {
            Some(row) => {
                let receipt = decode_receipt(row)?;
                validate_agent_receipt_binding(&conn, id, &receipt)?;
                Ok(Some(receipt))
            }
            None if status == SettingProposalStatus::Applied => {
                Err(setting_proposal_integrity_error("已应用的提案缺少变更回执"))
            }
            None => Ok(None),
        }
    }
}

/// SQLite 版 SettingProposal Unit of Work。
pub struct SqliteSettingProposalUow {
    db: Arc<Db>,
}

impl SqliteSettingProposalUow {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

impl SettingProposalUoW for SqliteSettingProposalUow {
    fn run(
        &self,
        f: &dyn Fn(&dyn SettingProposalTxPorts) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        // BEGIN IMMEDIATE：进入即取 RESERVED 写锁。读-改-写（重读提案 → CAS 写目标 →
        // 插回执 → 改状态）必须在同一个事务里，不能退化成多个独立事务。
        let mut guard = self.db.lock();
        let tx = guard
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| tx_err("开启设置提案事务失败", e))?;
        let scope = SqliteSettingProposalTx { tx: &tx };
        match f(&scope) {
            Ok(()) => tx.commit().map_err(|e| tx_err("提交设置提案事务失败", e)),
            Err(e) => Err(e), // tx Drop 时自动回滚
        }
    }
}

struct SqliteSettingProposalTx<'a> {
    tx: &'a Transaction<'a>,
}

impl SettingProposalTxPorts for SqliteSettingProposalTx<'_> {
    fn insert_proposal(&self, proposal: &SettingProposal) -> Result<(), AppError> {
        let target = proposal.target();
        self.tx
            .execute(
                "INSERT INTO setting_proposals
                    (id, scope, section, edition_id, media_item_id, payload_json, digest, status,
                     created_at, expires_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?9)",
                rusqlite::params![
                    proposal.id().to_string(),
                    target.scope_str(),
                    target.section().map(|section| section.as_str()),
                    target.edition_id().map(|id| id.to_string()),
                    target.media_item_id().map(|id| id.to_string()),
                    proposal.canonical_json(),
                    proposal.digest(),
                    proposal.status().as_str(),
                    proposal.created_at().0,
                    proposal.expires_at().0,
                ],
            )
            .map_err(|error| {
                if is_constraint_violation(&error) {
                    AppError::new(
                        "SETTING_PROPOSAL_ALREADY_EXISTS",
                        ErrorKind::AlreadyExists,
                        "提案已存在，不能覆盖",
                        false,
                    )
                    .with_source(error)
                } else {
                    tx_err("保存设置提案失败", error)
                }
            })?;
        Ok(())
    }

    fn insert_agent_action_binding(&self, binding: &AgentActionBinding) -> Result<(), AppError> {
        insert_agent_action_binding(self.tx, binding)
    }

    fn load_agent_action_binding(
        &self,
        proposal_id: SettingProposalId,
    ) -> Result<Option<AgentActionBinding>, AppError> {
        load_agent_action_binding_tx(self.tx, proposal_id)
    }

    fn verify_agent_subject(&self, subject: AgentScopeSubject) -> Result<(), AppError> {
        subject.validate()?;
        // 全局设置范围没有作品/版本/条目可核对：它在同一事务内的权威性由
        // `verify_settings_base_revision` 承担（设置行没有外键可查）。
        let Some(subject) = subject.as_content() else {
            return Ok(());
        };
        let work_exists: Option<i64> = self
            .tx
            .query_row(
                "SELECT 1 FROM works WHERE id = ?1",
                rusqlite::params![subject.work_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| tx_err("查询 Agent Subject 作品失败", error))?;
        if work_exists.is_none() {
            return Err(agent_subject_not_found(
                "AGENT_SUBJECT_WORK_NOT_FOUND",
                "Agent Subject 对应的作品不存在",
            ));
        }

        let Some(edition_id) = subject.edition_id else {
            return Ok(());
        };
        let edition_work_id: Option<String> = self
            .tx
            .query_row(
                "SELECT work_id FROM editions WHERE id = ?1",
                rusqlite::params![edition_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| tx_err("查询 Agent Subject 版本失败", error))?;
        let Some(edition_work_id) = edition_work_id else {
            return Err(agent_subject_not_found(
                "AGENT_SUBJECT_EDITION_NOT_FOUND",
                "Agent Subject 对应的版本不存在",
            ));
        };
        let edition_work_id: haven_domain::ids::WorkId = edition_work_id
            .parse()
            .map_err(|_| setting_proposal_integrity_error("版本归属作品 ID 损坏"))?;
        if edition_work_id != subject.work_id {
            return Err(agent_subject_scope_mismatch(
                "版本不属于 Agent Subject 的作品",
            ));
        }

        let Some(media_item_id) = subject.media_item_id else {
            return Ok(());
        };
        let media_edition_id: Option<String> = self
            .tx
            .query_row(
                "SELECT edition_id FROM media_items WHERE id = ?1",
                rusqlite::params![media_item_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| tx_err("查询 Agent Subject 媒体条目失败", error))?;
        let Some(media_edition_id) = media_edition_id else {
            return Err(agent_subject_not_found(
                "AGENT_SUBJECT_MEDIA_ITEM_NOT_FOUND",
                "Agent Subject 对应的媒体条目不存在",
            ));
        };
        let media_edition_id: EditionId = media_edition_id
            .parse()
            .map_err(|_| setting_proposal_integrity_error("媒体条目归属版本 ID 损坏"))?;
        if media_edition_id != edition_id {
            return Err(agent_subject_scope_mismatch(
                "媒体条目不属于 Agent Subject 的版本",
            ));
        }
        Ok(())
    }

    fn verify_settings_base_revision(
        &self,
        section: &str,
        expected_revision: Option<&str>,
    ) -> Result<bool, AppError> {
        let current: Option<String> = self
            .tx
            .query_row(
                "SELECT revision FROM settings WHERE section = ?1",
                rusqlite::params![section],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| tx_err("查询设置 revision 失败", error))?;
        Ok(current.as_deref() == expected_revision)
    }

    fn mark_rejected(
        &self,
        id: SettingProposalId,
        expected_digest: &str,
        rejected_at: UtcMillis,
    ) -> Result<bool, AppError> {
        let affected = self
            .tx
            .execute(
                "UPDATE setting_proposals SET status = 'rejected', updated_at = ?3
                 WHERE id = ?1 AND status = 'pending' AND digest = ?2",
                rusqlite::params![id.to_string(), expected_digest, rejected_at.0],
            )
            .map_err(|error| tx_err("拒绝设置提案失败", error))?;
        Ok(affected > 0)
    }

    fn update_agent_action_binding_state(
        &self,
        proposal_id: SettingProposalId,
        expected: AgentApprovalState,
        next: AgentApprovalState,
    ) -> Result<(), AppError> {
        update_agent_action_binding_state(self.tx, proposal_id, expected, next, UtcMillis::now())
    }

    fn load_proposal(&self, id: SettingProposalId) -> Result<Option<SettingProposal>, AppError> {
        let row = self
            .tx
            .query_row(
                &format!("SELECT {PROPOSAL_COLUMNS} FROM setting_proposals WHERE id = ?1"),
                rusqlite::params![id.to_string()],
                read_proposal_row,
            )
            .optional()
            .map_err(|e| tx_err("查询设置提案失败", e))?;
        row.map(decode_proposal).transpose()
    }

    fn load_receipt(
        &self,
        id: SettingProposalId,
    ) -> Result<Option<SettingChangeReceipt>, AppError> {
        let row = self
            .tx
            .query_row(
                &format!(
                    "SELECT {RECEIPT_COLUMNS}
                     FROM setting_change_receipts r
                     JOIN setting_proposals p ON p.id = r.proposal_id
                     WHERE r.proposal_id = ?1"
                ),
                rusqlite::params![id.to_string()],
                read_receipt_row,
            )
            .optional()
            .map_err(|e| tx_err("查询设置变更回执失败", e))?;
        row.map(decode_receipt).transpose()
    }

    fn edition_exists(&self, edition_id: EditionId) -> Result<bool, AppError> {
        let found: Option<i64> = self
            .tx
            .query_row(
                "SELECT 1 FROM editions WHERE id = ?1",
                rusqlite::params![edition_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| tx_err("查询版本失败", e))?;
        Ok(found.is_some())
    }

    fn media_item_edition(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Option<EditionId>, AppError> {
        let raw: Option<String> = self
            .tx
            .query_row(
                "SELECT edition_id FROM media_items WHERE id = ?1",
                rusqlite::params![media_item_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| tx_err("查询媒体条目归属失败", e))?;
        raw.map(|value| {
            value
                .parse()
                .map_err(|_| setting_proposal_integrity_error("媒体条目归属版本 ID 损坏"))
        })
        .transpose()
    }

    fn load_settings(&self, section: &str) -> Result<Option<SettingsRow>, AppError> {
        self.tx
            .query_row(
                "SELECT section, schema_version, revision, data_json, updated_at
                 FROM settings WHERE section = ?1",
                rusqlite::params![section],
                read_settings_row,
            )
            .optional()
            .map_err(|e| tx_err("查询设置失败", e))
    }

    fn cas_write_settings(
        &self,
        _section: &str,
        expected_revision: Option<&str>,
        row: &SettingsRow,
    ) -> Result<bool, AppError> {
        let affected = self
            .tx
            .execute(
                "INSERT INTO settings (section, schema_version, revision, data_json, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(section) DO UPDATE SET
                     schema_version = excluded.schema_version,
                     revision = excluded.revision,
                     data_json = excluded.data_json,
                     updated_at = excluded.updated_at
                 WHERE settings.revision = ?6",
                rusqlite::params![
                    row.section,
                    row.schema_version,
                    row.revision,
                    row.data_json,
                    row.updated_at.0,
                    expected_revision,
                ],
            )
            .map_err(|e| tx_err("保存设置失败", e))?;
        Ok(affected > 0)
    }

    fn load_edition_preference(
        &self,
        edition_id: EditionId,
    ) -> Result<Option<EditionPreference>, AppError> {
        let row = self
            .tx
            .query_row(
                "SELECT edition_id, data_json, revision, updated_at
                 FROM edition_preferences WHERE edition_id = ?1",
                rusqlite::params![edition_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| tx_err("查询版本内设失败", e))?;
        row.map(|(id, data_json, revision, updated_at)| {
            Ok(EditionPreference {
                edition_id: id
                    .parse()
                    .map_err(|_| setting_proposal_integrity_error("版本内设 ID 损坏"))?,
                data: decode_preference_data(&data_json)?,
                revision,
                updated_at: UtcMillis(updated_at),
            })
        })
        .transpose()
    }

    fn load_media_item_preference(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Option<MediaItemPreference>, AppError> {
        let row = self
            .tx
            .query_row(
                "SELECT media_item_id, edition_id, data_json, revision, updated_at
                 FROM media_item_preferences WHERE media_item_id = ?1",
                rusqlite::params![media_item_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| tx_err("查询媒体资源内设失败", e))?;
        row.map(|(id, edition_id, data_json, revision, updated_at)| {
            Ok(MediaItemPreference {
                media_item_id: id
                    .parse()
                    .map_err(|_| setting_proposal_integrity_error("媒体资源内设 ID 损坏"))?,
                edition_id: edition_id
                    .parse()
                    .map_err(|_| setting_proposal_integrity_error("媒体资源内设版本 ID 损坏"))?,
                data: decode_preference_data(&data_json)?,
                revision,
                updated_at: UtcMillis(updated_at),
            })
        })
        .transpose()
    }

    fn cas_upsert_edition(
        &self,
        preference: &EditionPreference,
        expected_revision: Option<&str>,
    ) -> Result<bool, AppError> {
        let current: Option<String> = self
            .tx
            .query_row(
                "SELECT revision FROM edition_preferences WHERE edition_id = ?1",
                rusqlite::params![preference.edition_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| tx_err("读取版本内设版本失败", e))?;
        if !revision_matches(current.as_deref(), expected_revision) {
            return Ok(false);
        }
        let data_json = encode_preference_data(&preference.data)?;
        self.tx
            .execute(
                "INSERT INTO edition_preferences (edition_id, data_json, revision, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(edition_id) DO UPDATE SET
                   data_json = excluded.data_json,
                   revision = excluded.revision,
                   updated_at = excluded.updated_at",
                rusqlite::params![
                    preference.edition_id.to_string(),
                    data_json,
                    preference.revision,
                    preference.updated_at.0
                ],
            )
            .map_err(|e| tx_err("保存版本内设失败", e))?;
        Ok(true)
    }

    fn cas_upsert_media_item(
        &self,
        preference: &MediaItemPreference,
        expected_revision: Option<&str>,
    ) -> Result<bool, AppError> {
        let current: Option<String> = self
            .tx
            .query_row(
                "SELECT revision FROM media_item_preferences WHERE media_item_id = ?1",
                rusqlite::params![preference.media_item_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| tx_err("读取媒体资源内设版本失败", e))?;
        if !revision_matches(current.as_deref(), expected_revision) {
            return Ok(false);
        }
        let data_json = encode_preference_data(&preference.data)?;
        self.tx
            .execute(
                "INSERT INTO media_item_preferences
                   (media_item_id, edition_id, data_json, revision, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(media_item_id) DO UPDATE SET
                   edition_id = excluded.edition_id,
                   data_json = excluded.data_json,
                   revision = excluded.revision,
                   updated_at = excluded.updated_at",
                rusqlite::params![
                    preference.media_item_id.to_string(),
                    preference.edition_id.to_string(),
                    data_json,
                    preference.revision,
                    preference.updated_at.0
                ],
            )
            .map_err(|e| tx_err("保存媒体资源内设失败", e))?;
        Ok(true)
    }

    fn insert_receipt(&self, receipt: &SettingChangeReceipt) -> Result<(), AppError> {
        let target = receipt.target;
        // provenance 也按 canonical JSON 落库：读取端要能"重算即等于存储字节"，
        // 写入端就必须先收敛成同一形式，否则两边永远差一个键序。
        let provenance_json = canonical_json_of(&receipt.provenance).map_err(|e| {
            AppError::new(
                "SERIALIZE_FAILED",
                ErrorKind::Parse,
                "回执来源序列化失败",
                false,
            )
            .with_source(e)
        })?;
        self.tx
            .execute(
                "INSERT INTO setting_change_receipts
                    (id, proposal_id, proposal_digest, scope, section, edition_id, media_item_id,
                     before_json, after_json, applied_revision, changed, provenance_json, applied_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                rusqlite::params![
                    receipt.id.to_string(),
                    receipt.proposal_id.to_string(),
                    receipt.proposal_digest,
                    target.scope_str(),
                    target.section().map(|section| section.as_str()),
                    target.edition_id().map(|id| id.to_string()),
                    target.media_item_id().map(|id| id.to_string()),
                    receipt.before_canonical_json,
                    receipt.after_canonical_json,
                    receipt.applied_revision,
                    i64::from(receipt.changed),
                    provenance_json,
                    receipt.applied_at.0,
                ],
            )
            .map_err(|e| tx_err("保存设置变更回执失败", e))?;
        Ok(())
    }

    fn mark_applied(
        &self,
        id: SettingProposalId,
        expected_digest: &str,
        applied_at: UtcMillis,
    ) -> Result<bool, AppError> {
        let affected = self
            .tx
            .execute(
                "UPDATE setting_proposals SET status = 'applied', updated_at = ?3
                 WHERE id = ?1 AND status = 'pending' AND digest = ?2",
                rusqlite::params![id.to_string(), expected_digest, applied_at.0],
            )
            .map_err(|e| tx_err("更新提案状态失败", e))?;
        Ok(affected > 0)
    }

    fn mark_expired(
        &self,
        id: SettingProposalId,
        expected_digest: &str,
        expired_at: UtcMillis,
    ) -> Result<(), AppError> {
        // 条件与 mark_applied 同形：只有仍是 pending 且 digest 一致的行才会迁移，
        // applied/rejected/expired 一律原样不动（终态不可再改写）。
        // 条件未命中（affected == 0）不是错误：调用方无论如何都按过期语义拒绝执行，
        // 且这个分支没有任何目标写入需要撤销。
        self.tx
            .execute(
                "UPDATE setting_proposals SET status = 'expired', updated_at = ?3
                 WHERE id = ?1 AND status = 'pending' AND digest = ?2",
                rusqlite::params![id.to_string(), expected_digest, expired_at.0],
            )
            .map_err(|e| tx_err("过期设置提案失败", e))?;
        Ok(())
    }

    fn issue_agent_action_approval_token(
        &self,
        id: SettingProposalId,
        expected_digest: &str,
        approval_token_hash: &AgentApprovalTokenHash,
        issued_at: UtcMillis,
    ) -> Result<bool, AppError> {
        issue_agent_action_approval_token(
            self.tx,
            id,
            expected_digest,
            approval_token_hash,
            issued_at,
        )
    }

    fn consume_agent_action_approval_token(
        &self,
        id: SettingProposalId,
        expected_digest: &str,
        approval_token_hash: &AgentApprovalTokenHash,
        consumed_at: UtcMillis,
    ) -> Result<bool, AppError> {
        consume_agent_action_approval_token(
            self.tx,
            id,
            expected_digest,
            approval_token_hash,
            consumed_at,
        )
    }
}

// ---------- 行解码 ----------

struct ProposalRow {
    id: String,
    scope: String,
    section: Option<String>,
    edition_id: Option<String>,
    media_item_id: Option<String>,
    payload_json: String,
    digest: String,
    status: String,
    created_at: i64,
    expires_at: i64,
}

fn read_proposal_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProposalRow> {
    Ok(ProposalRow {
        id: row.get(0)?,
        scope: row.get(1)?,
        section: row.get(2)?,
        edition_id: row.get(3)?,
        media_item_id: row.get(4)?,
        payload_json: row.get(5)?,
        digest: row.get(6)?,
        status: row.get(7)?,
        created_at: row.get(8)?,
        expires_at: row.get(9)?,
    })
}

/// 存储行 → 提案：完整性 + 冗余 target 列一致性。
fn decode_proposal(row: ProposalRow) -> Result<SettingProposal, AppError> {
    let id: SettingProposalId = row
        .id
        .parse()
        .map_err(|_| setting_proposal_integrity_error("提案 ID 非法"))?;
    let status = SettingProposalStatus::parse(&row.status)
        .ok_or_else(|| setting_proposal_integrity_error("提案状态不是闭合集合中的取值"))?;
    let proposal = SettingProposal::from_stored(
        id,
        row.payload_json,
        row.digest,
        status,
        UtcMillis(row.created_at),
    )?;

    let target = proposal.target();
    if target.scope_str() != row.scope {
        return Err(setting_proposal_integrity_error(
            "冗余 scope 列与载荷不一致",
        ));
    }
    if target.section().map(|section| section.as_str()) != row.section.as_deref() {
        return Err(setting_proposal_integrity_error(
            "冗余 section 列与载荷不一致",
        ));
    }
    if target.edition_id().map(|id| id.to_string()).as_deref() != row.edition_id.as_deref() {
        return Err(setting_proposal_integrity_error(
            "冗余 edition_id 列与载荷不一致",
        ));
    }
    if target.media_item_id().map(|id| id.to_string()).as_deref() != row.media_item_id.as_deref() {
        return Err(setting_proposal_integrity_error(
            "冗余 media_item_id 列与载荷不一致",
        ));
    }
    if proposal.expires_at().0 != row.expires_at {
        return Err(setting_proposal_integrity_error(
            "冗余 expires_at 列与载荷不一致",
        ));
    }
    Ok(proposal)
}

struct ReceiptRow {
    id: String,
    proposal_id: String,
    proposal_digest: String,
    scope: String,
    section: Option<String>,
    edition_id: Option<String>,
    media_item_id: Option<String>,
    before_json: String,
    after_json: String,
    applied_revision: Option<String>,
    changed: i64,
    provenance_json: String,
    applied_at: i64,
    /// 父提案的载荷级列（JOIN 得到）：用于重建父提案并识别外键错配。
    parent_payload_json: String,
    parent_digest: String,
    parent_status: String,
    parent_created_at: i64,
}

fn read_receipt_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReceiptRow> {
    Ok(ReceiptRow {
        id: row.get(0)?,
        proposal_id: row.get(1)?,
        proposal_digest: row.get(2)?,
        scope: row.get(3)?,
        section: row.get(4)?,
        edition_id: row.get(5)?,
        media_item_id: row.get(6)?,
        before_json: row.get(7)?,
        after_json: row.get(8)?,
        applied_revision: row.get(9)?,
        changed: row.get(10)?,
        provenance_json: row.get(11)?,
        applied_at: row.get(12)?,
        parent_payload_json: row.get(13)?,
        parent_digest: row.get(14)?,
        parent_status: row.get(15)?,
        parent_created_at: row.get(16)?,
    })
}

/// 存储行 → 回执：自身一致性校验 + 与父提案的载荷级交叉校验。
fn decode_receipt(row: ReceiptRow) -> Result<SettingChangeReceipt, AppError> {
    let target = decode_target(&row.scope, row.section, row.edition_id, row.media_item_id)?;
    if row.changed != 0 && row.changed != 1 {
        return Err(setting_proposal_integrity_error("回执 changed 不是 0/1"));
    }
    // provenance_json 自身必须是 canonical JSON：写入端落的也是 canonical 形式，
    // 读取端再按结构化 provenance 重算一遍，两边不一致即视为行被改写。
    let provenance: SettingProvenance = serde_json::from_str(&row.provenance_json)
        .map_err(|_| setting_proposal_integrity_error("回执来源无法解析"))?;
    if canonical_json_of(&provenance)? != row.provenance_json {
        return Err(setting_proposal_integrity_error(
            "回执来源不是 canonical 形式",
        ));
    }
    let proposal_id: SettingProposalId = row
        .proposal_id
        .parse()
        .map_err(|_| setting_proposal_integrity_error("回执提案 ID 非法"))?;
    let receipt = SettingChangeReceipt {
        id: row
            .id
            .parse()
            .map_err(|_| setting_proposal_integrity_error("回执 ID 非法"))?,
        proposal_id,
        proposal_digest: row.proposal_digest,
        target,
        before_canonical_json: row.before_json,
        after_canonical_json: row.after_json,
        applied_revision: row.applied_revision,
        changed: row.changed == 1,
        provenance,
        applied_at: UtcMillis(row.applied_at),
    };
    receipt.validate()?;

    // 回执是"提案已生效"的凭证，因此父提案必须真实存在、**载荷完整**且处于
    // applied：指向一条 pending/rejected/expired 的提案，或指向一条被改写的行，
    // 都不构成一次真实的设置变更。这里用 `from_stored` 重建父提案，顺带把它自己的
    // canonical/digest/语义完整性跑一遍，再与回执的 digest 与 target 交叉比对——
    // 比对的是**重建后的 target**，而不是冗余列，避免"两边列同样错"时两边自洽。
    let parent_status = SettingProposalStatus::parse(&row.parent_status)
        .ok_or_else(|| setting_proposal_integrity_error("父提案状态不在闭合集合内"))?;
    let parent = SettingProposal::from_stored(
        proposal_id,
        row.parent_payload_json,
        row.parent_digest,
        parent_status,
        UtcMillis(row.parent_created_at),
    )?;
    if parent.status() != SettingProposalStatus::Applied {
        return Err(setting_proposal_integrity_error(
            "回执所属提案不是已应用状态",
        ));
    }
    if receipt.proposal_digest != parent.digest() {
        return Err(setting_proposal_integrity_error("回执摘要与所属提案不一致"));
    }
    if receipt.target != parent.target() {
        return Err(setting_proposal_integrity_error("回执目标与所属提案不一致"));
    }
    if receipt.provenance != *parent.provenance() {
        return Err(setting_proposal_integrity_error("回执来源与所属提案不一致"));
    }
    Ok(receipt)
}

/// 冗余 target 列 → `SettingTarget`（闭合形状由 042 的 CHECK 兜底，这里再挡一次）。
fn decode_target(
    scope: &str,
    section: Option<String>,
    edition_id: Option<String>,
    media_item_id: Option<String>,
) -> Result<SettingTarget, AppError> {
    let parse_edition = |value: &str| -> Result<EditionId, AppError> {
        value
            .parse()
            .map_err(|_| setting_proposal_integrity_error("目标版本 ID 非法"))
    };
    let parse_media = |value: &str| -> Result<MediaItemId, AppError> {
        value
            .parse()
            .map_err(|_| setting_proposal_integrity_error("目标媒体条目 ID 非法"))
    };
    match (scope, section, edition_id, media_item_id) {
        ("global", Some(section), None, None) => {
            let section = haven_domain::settings::SettingsSection::parse(&section)
                .ok_or_else(|| setting_proposal_integrity_error("目标分区不在闭合集合内"))?;
            Ok(SettingTarget::global(section))
        }
        ("edition", None, Some(edition_id), None) => {
            Ok(SettingTarget::edition(parse_edition(&edition_id)?))
        }
        ("media_item", None, Some(edition_id), Some(media_item_id)) => Ok(
            SettingTarget::media_item(parse_edition(&edition_id)?, parse_media(&media_item_id)?),
        ),
        _ => Err(setting_proposal_integrity_error("目标作用域与列形状不一致")),
    }
}

fn read_settings_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SettingsRow> {
    Ok(SettingsRow {
        section: row.get("section")?,
        schema_version: row.get("schema_version")?,
        revision: row.get("revision")?,
        data_json: row.get("data_json")?,
        updated_at: UtcMillis(row.get("updated_at")?),
    })
}

fn decode_preference_data(data_json: &str) -> Result<PreferenceData, AppError> {
    serde_json::from_str(data_json)
        .map_err(|_| setting_proposal_integrity_error("资源内设数据无法解析"))
}

fn encode_preference_data(data: &PreferenceData) -> Result<String, AppError> {
    serde_json::to_string(data).map_err(|e| {
        AppError::new(
            "SETTINGS_DATA_INVALID",
            ErrorKind::Validation,
            "资源内设数据非法",
            false,
        )
        .with_source(e)
    })
}

fn revision_matches(current: Option<&str>, expected: Option<&str>) -> bool {
    match (current, expected) {
        (None, None) => true,
        (Some(current), Some(expected)) => current == expected,
        _ => false,
    }
}

fn is_constraint_violation(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(inner, _)
            if inner.code == rusqlite::ErrorCode::ConstraintViolation
                && matches!(
                    inner.extended_code,
                    rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY
                        | rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                )
    )
}

fn tx_err(msg: &'static str, e: rusqlite::Error) -> AppError {
    AppError::new("DATABASE_ERROR", ErrorKind::Database, msg, true).with_source(e)
}

fn validate_agent_receipt_binding(
    conn: &rusqlite::Connection,
    proposal_id: SettingProposalId,
    receipt: &SettingChangeReceipt,
) -> Result<(), AppError> {
    let binding = load_agent_action_binding(conn, proposal_id)?;
    let requires_binding = receipt.provenance.source_kind == ProvenanceSourceKind::Agent
        || receipt.provenance.actor_kind == ProvenanceActorKind::Agent
        || receipt.provenance.reason == ProvenanceReason::AgentSuggestion;
    if !requires_binding {
        if binding.is_some() {
            return Err(AppError::new(
                "AGENT_ACTION_BINDING_INTEGRITY",
                ErrorKind::Internal,
                "非 Agent 提案回执不应存在动作绑定",
                false,
            ));
        }
        return Ok(());
    }
    if receipt.provenance.source_kind != ProvenanceSourceKind::Agent
        || receipt.provenance.actor_kind != ProvenanceActorKind::Agent
        || receipt.provenance.reason != ProvenanceReason::AgentSuggestion
    {
        return Err(AppError::new(
            "AGENT_ACTION_BINDING_INTEGRITY",
            ErrorKind::Internal,
            "Agent 提案的 provenance 组合不完整",
            false,
        ));
    }
    let binding = binding.ok_or_else(|| {
        AppError::new(
            "AGENT_ACTION_BINDING_MISSING",
            ErrorKind::Internal,
            "Agent 提案回执缺少动作绑定",
            false,
        )
    })?;
    let proposal_lifecycle = conn
        .query_row(
            "SELECT created_at, expires_at FROM setting_proposals WHERE id = ?1",
            rusqlite::params![proposal_id.to_string()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(map_db_error("查询 Agent 提案生命周期失败"))?
        .ok_or_else(|| setting_proposal_integrity_error("Agent 回执所属提案不存在"))?;
    if binding.proposal_id() != proposal_id
        || binding.action_kind() != AgentActionKind::SettingsProposal
        || binding.created_at() != UtcMillis(proposal_lifecycle.0)
        || binding.expires_at() != UtcMillis(proposal_lifecycle.1)
        || binding.approval_state() != AgentApprovalState::Applied
    {
        return Err(AppError::new(
            "AGENT_ACTION_BINDING_INTEGRITY",
            ErrorKind::Internal,
            "Agent 提案回执的动作绑定状态不一致",
            false,
        ));
    }
    Ok(())
}

fn agent_subject_not_found(code: &'static str, message: &'static str) -> AppError {
    AppError::new(code, ErrorKind::NotFound, message, false)
}

fn agent_subject_scope_mismatch(detail: &'static str) -> AppError {
    AppError::new(
        "AGENT_SUBJECT_SCOPE_MISMATCH",
        ErrorKind::Forbidden,
        format!("Agent Subject 归属校验失败：{detail}"),
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_application::services::agent::{
        AgentProposalService, AgentSettingsActionRequest, AgentSubjectScopePort,
        RepositoryAgentSubjectScope,
    };
    use haven_application::services::setting_proposals::{
        SettingProposalRequest, SettingProposalService,
    };
    use haven_domain::agent::{
        AgentActionBindingInput, AgentActionKind, AgentApprovalState, AgentApprovalToken,
        AgentBoundaryMode, AgentContentKind, AgentContextPayload, AgentContextSnapshot,
        AgentLocatorConfidence, AgentSubject,
    };
    use haven_domain::contracts::{
        AgentActionBindingRepository, EditionPreference, ResourcePreferenceRepository,
        SettingProposalRepository, SettingsRepository, SettingsRow,
    };
    use haven_domain::ids::{AgentContextSnapshotId, AgentRequestId, AgentSessionId, WorkId};
    use haven_domain::setting_proposal::{
        ProvenanceReason, SettingProposalChange, SettingProposalStatus, SettingProvenance,
    };
    use haven_domain::settings::{ReadingFontSize, ReadingPatch, SettingsPatch, SettingsSection};

    use crate::db::repos::SqliteRepositories;

    fn service(db: &Arc<Db>) -> (SettingProposalService, Arc<SqliteSettingProposalRepository>) {
        let repository = Arc::new(SqliteSettingProposalRepository::new(db.clone()));
        let uow = Arc::new(SqliteSettingProposalUow::new(db.clone()));
        let service = SettingProposalService::new(repository.clone(), uow);
        (service, repository)
    }

    fn provenance() -> SettingProvenance {
        SettingProvenance::user(ProvenanceReason::UserRequest)
            .with_source_ref("sqlite-integration-test")
            .unwrap()
    }

    fn seed_work(db: &Db) -> WorkId {
        let work_id = WorkId::new();
        db.lock()
            .execute(
                "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
                 VALUES (?1, 'Agent 提案测试', 'fiction', 'completed', 1, 1)",
                rusqlite::params![work_id.to_string()],
            )
            .unwrap();
        work_id
    }

    fn agent_subject(db: &Db) -> AgentSubject {
        AgentSubject::work(seed_work(db), AgentContentKind::Book)
    }

    fn agent_binding_input(subject: AgentSubject) -> AgentActionBindingInput {
        AgentActionBindingInput::new(
            AgentSessionId::new(),
            AgentRequestId::new(),
            AgentContextSnapshotId::new(),
            "a".repeat(64),
            subject,
            AgentActionKind::SettingsProposal,
        )
        .unwrap()
    }

    fn seed_edition(db: &Db, edition_id: EditionId) -> haven_domain::ids::WorkId {
        let work_id = haven_domain::ids::WorkId::new();
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
                 VALUES (?1, '提案测试', 'fiction', 'completed', 1, 1)",
                rusqlite::params![work_id.to_string()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
                 VALUES (?1, ?2, '提案测试版本', 'book', 1, 1)",
                rusqlite::params![edition_id.to_string(), work_id.to_string()],
            )
            .unwrap();
        }
        work_id
    }

    fn seed_media_item(db: &Db, media_item_id: MediaItemId, edition_id: EditionId) {
        db.lock()
            .execute(
                "INSERT INTO media_items
                    (id, edition_id, media_type, title, status, created_at, updated_at)
                 VALUES (?1, ?2, 'book', '提案测试资源', 'available', 1, 1)",
                rusqlite::params![media_item_id.to_string(), edition_id.to_string()],
            )
            .unwrap();
    }

    fn resource_preference() -> PreferenceData {
        PreferenceData {
            reading: Some(ReadingPatch {
                font_size: Some(ReadingFontSize::Large),
                ..ReadingPatch::default()
            }),
            comic: None,
        }
    }

    fn reading_patch(font_size: ReadingFontSize) -> SettingProposalChange {
        SettingProposalChange::SettingsPatch(SettingsPatch::Reading(ReadingPatch {
            font_size: Some(font_size),
            ..ReadingPatch::default()
        }))
    }

    fn count_rows(db: &Db, table: &str) -> i64 {
        // table 只能来自本测试中的固定字面量，避免把动态 SQL 引入生产路径；
        // 这里仅用于验证事务的最终行数。
        db.lock()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    #[test]
    fn duplicate_mapping_only_accepts_primary_key_and_unique_constraints() {
        let primary_key = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::ConstraintViolation,
                extended_code: rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY,
            },
            None,
        );
        let unique = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::ConstraintViolation,
                extended_code: rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE,
            },
            None,
        );
        let check = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::ConstraintViolation,
                extended_code: rusqlite::ffi::SQLITE_CONSTRAINT_CHECK,
            },
            None,
        );

        assert!(is_constraint_violation(&primary_key));
        assert!(is_constraint_violation(&unique));
        assert!(!is_constraint_violation(&check));
    }

    #[tokio::test]
    async fn sqlite_create_reject_and_readback_use_real_repository() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (service, repository) = service(&db);
        let proposal = service
            .create(SettingProposalRequest::new(
                SettingTarget::global(SettingsSection::Reading),
                reading_patch(ReadingFontSize::Large),
                provenance(),
            ))
            .await
            .unwrap();

        let stored = repository.get(proposal.id()).await.unwrap().unwrap();
        assert_eq!(stored, proposal);
        assert!(
            repository
                .get_receipt(proposal.id())
                .await
                .unwrap()
                .is_none()
        );

        service
            .reject(proposal.id(), proposal.digest())
            .await
            .unwrap();
        let rejected = repository.get(proposal.id()).await.unwrap().unwrap();
        assert_eq!(rejected.status(), SettingProposalStatus::Rejected);
    }

    #[tokio::test]
    async fn sqlite_agent_binding_is_atomic_and_roundtrips_canonical_subject() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (service, _) = service(&db);
        let subject = agent_subject(&db);
        let (proposal, binding) = service
            .create_with_agent_binding(
                SettingProposalRequest::new(
                    SettingTarget::global(SettingsSection::Reading),
                    reading_patch(ReadingFontSize::Large),
                    SettingProvenance::agent(ProvenanceReason::AgentSuggestion),
                ),
                agent_binding_input(subject),
            )
            .await
            .unwrap();

        let repositories = SqliteRepositories::new(db.clone());
        let stored = repositories
            .agent_bindings
            .get(proposal.id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored, binding);
        assert_eq!(stored.approval_state(), AgentApprovalState::Pending);

        let subject_json: String = db
            .lock()
            .query_row(
                "SELECT subject_json FROM agent_action_bindings WHERE proposal_id = ?1",
                rusqlite::params![proposal.id().to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(subject_json, canonical_json_of(&subject).unwrap());
    }

    #[tokio::test]
    async fn sqlite_agent_binding_insert_failure_rolls_back_the_proposal() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (service, _) = service(&db);
        db.lock()
            .execute_batch(
                "CREATE TRIGGER test_fail_agent_binding_insert
                 BEFORE INSERT ON agent_action_bindings
                 BEGIN SELECT RAISE(ABORT, 'test binding failure'); END;",
            )
            .unwrap();

        let error = service
            .create_with_agent_binding(
                SettingProposalRequest::new(
                    SettingTarget::global(SettingsSection::Reading),
                    reading_patch(ReadingFontSize::Large),
                    SettingProvenance::agent(ProvenanceReason::AgentSuggestion),
                ),
                agent_binding_input(agent_subject(&db)),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "DATABASE_ERROR");
        assert_eq!(count_rows(&db, "setting_proposals"), 0);
        assert_eq!(count_rows(&db, "agent_action_bindings"), 0);
    }

    #[tokio::test]
    async fn sqlite_agent_binding_state_tracks_reject_apply_and_expire() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repository = Arc::new(SqliteSettingProposalRepository::new(db.clone()));
        let uow = Arc::new(SqliteSettingProposalUow::new(db.clone()));
        let service = SettingProposalService::new(repository.clone(), uow.clone());
        let repositories = SqliteRepositories::new(db.clone());

        let (rejected, _) = service
            .create_with_agent_binding(
                SettingProposalRequest::new(
                    SettingTarget::global(SettingsSection::Reading),
                    reading_patch(ReadingFontSize::Large),
                    SettingProvenance::agent(ProvenanceReason::AgentSuggestion),
                ),
                agent_binding_input(agent_subject(&db)),
            )
            .await
            .unwrap();
        let rejected_token = service
            .issue_agent_approval_token(rejected.id(), rejected.digest())
            .await
            .unwrap();
        service
            .reject(rejected.id(), rejected.digest())
            .await
            .unwrap();
        assert_eq!(
            repositories
                .agent_bindings
                .get(rejected.id())
                .await
                .unwrap()
                .unwrap()
                .approval_state(),
            AgentApprovalState::Rejected
        );
        assert!(
            !repositories
                .agent_bindings
                .get(rejected.id())
                .await
                .unwrap()
                .unwrap()
                .has_approval_token()
        );
        // 编译期/运行期都保留令牌只在这个测试作用域内；拒绝后旧令牌必须无效。
        assert_eq!(
            service
                .apply_confirmed_with_approval_token(
                    rejected.id(),
                    rejected.digest(),
                    &rejected_token,
                )
                .await
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_APPROVAL_TOKEN_INVALID"
        );

        let (applied, _) = service
            .create_with_agent_binding(
                SettingProposalRequest::new(
                    SettingTarget::global(SettingsSection::Reading),
                    reading_patch(ReadingFontSize::Large),
                    SettingProvenance::agent(ProvenanceReason::AgentSuggestion),
                ),
                agent_binding_input(agent_subject(&db)),
            )
            .await
            .unwrap();
        let applied_token = service
            .issue_agent_approval_token(applied.id(), applied.digest())
            .await
            .unwrap();
        service
            .apply_confirmed_with_approval_token(applied.id(), applied.digest(), &applied_token)
            .await
            .unwrap();
        assert_eq!(
            repositories
                .agent_bindings
                .get(applied.id())
                .await
                .unwrap()
                .unwrap()
                .approval_state(),
            AgentApprovalState::Applied
        );

        let subject = agent_subject(&db);
        let expired = SettingProposal::new(
            SettingProposalId::new(),
            SettingTarget::global(SettingsSection::Reading),
            reading_patch(ReadingFontSize::Small),
            None,
            SettingProvenance::agent(ProvenanceReason::AgentSuggestion),
            UtcMillis(1),
            UtcMillis(2),
        )
        .unwrap();
        let binding = AgentActionBinding::pending(
            expired.id(),
            agent_binding_input(subject),
            UtcMillis(1),
            UtcMillis(2),
        )
        .unwrap();
        uow.run(&|tx| {
            tx.insert_proposal(&expired)?;
            tx.insert_agent_action_binding(&binding)
        })
        .unwrap();
        let expired_token = AgentApprovalToken::issue();
        let expired_token_hash = expired_token.bind(expired.id(), expired.digest()).unwrap();
        uow.run(&|tx| {
            assert!(tx.issue_agent_action_approval_token(
                expired.id(),
                expired.digest(),
                &expired_token_hash,
                UtcMillis(1),
            )?);
            Ok(())
        })
        .unwrap();

        let error = service
            .apply_confirmed_with_approval_token(expired.id(), expired.digest(), &expired_token)
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_EXPIRED");
        assert_eq!(
            repository
                .get(expired.id())
                .await
                .unwrap()
                .unwrap()
                .status(),
            SettingProposalStatus::Expired
        );
        assert_eq!(
            repositories
                .agent_bindings
                .get(expired.id())
                .await
                .unwrap()
                .unwrap()
                .approval_state(),
            AgentApprovalState::Expired
        );
    }

    #[tokio::test]
    async fn sqlite_agent_approval_token_is_hashed_reissued_and_consumed_once() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (service, repository) = service(&db);
        let repositories = SqliteRepositories::new(db.clone());
        let (proposal, _) = service
            .create_with_agent_binding(
                SettingProposalRequest::new(
                    SettingTarget::global(SettingsSection::Reading),
                    reading_patch(ReadingFontSize::Large),
                    SettingProvenance::agent(ProvenanceReason::AgentSuggestion),
                ),
                agent_binding_input(agent_subject(&db)),
            )
            .await
            .unwrap();

        let first = service
            .issue_agent_approval_token(proposal.id(), proposal.digest())
            .await
            .unwrap();
        let first_binding = repositories
            .agent_bindings
            .get(proposal.id())
            .await
            .unwrap()
            .unwrap();
        let first_hash = first_binding
            .approval_token_hash()
            .unwrap()
            .as_str()
            .to_owned();
        assert_ne!(first_hash, first.as_str());
        assert!(!first_hash.contains(first.as_str()));
        let stored_hash: String = db
            .lock()
            .query_row(
                "SELECT approval_token_hash FROM agent_action_bindings WHERE proposal_id = ?1",
                rusqlite::params![proposal.id().to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_hash, first_hash);
        assert!(!stored_hash.contains(first.as_str()));

        // Agent 提案不能通过只提交 digest 的旧入口绕过一次性审批令牌。
        let error = service
            .apply_confirmed(proposal.id(), proposal.digest())
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "AGENT_APPROVAL_TOKEN_REQUIRED");
        assert_eq!(count_rows(&db, "settings"), 0);
        assert_eq!(count_rows(&db, "setting_change_receipts"), 0);
        assert_eq!(
            repository
                .get(proposal.id())
                .await
                .unwrap()
                .unwrap()
                .status(),
            SettingProposalStatus::Pending
        );

        // 重新签发覆盖旧摘要，旧令牌不再有任何写入能力。
        let second = service
            .issue_agent_approval_token(proposal.id(), proposal.digest())
            .await
            .unwrap();
        let second_binding = repositories
            .agent_bindings
            .get(proposal.id())
            .await
            .unwrap()
            .unwrap();
        let second_hash = second_binding
            .approval_token_hash()
            .unwrap()
            .as_str()
            .to_owned();
        assert_ne!(first_hash, second_hash);
        let error = service
            .apply_confirmed_with_approval_token(proposal.id(), proposal.digest(), &first)
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "AGENT_APPROVAL_TOKEN_INVALID");
        assert_eq!(count_rows(&db, "settings"), 0);
        assert_eq!(count_rows(&db, "setting_change_receipts"), 0);
        assert_eq!(
            repository
                .get(proposal.id())
                .await
                .unwrap()
                .unwrap()
                .status(),
            SettingProposalStatus::Pending
        );

        let receipt = service
            .apply_confirmed_with_approval_token(proposal.id(), proposal.digest(), &second)
            .await
            .unwrap();
        assert!(receipt.changed);
        assert_eq!(count_rows(&db, "settings"), 1);
        assert_eq!(count_rows(&db, "setting_change_receipts"), 1);
        let consumed = repositories
            .agent_bindings
            .get(proposal.id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(consumed.approval_state(), AgentApprovalState::Applied);
        assert!(!consumed.has_approval_token());
        let consumed_columns: (Option<String>, Option<i64>) = db
            .lock()
            .query_row(
                "SELECT approval_token_hash, approval_token_issued_at
                 FROM agent_action_bindings WHERE proposal_id = ?1",
                rusqlite::params![proposal.id().to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(consumed_columns, (None, None));

        // 一次性消费后即使再次提交同一枚令牌，也不能回放回执或重写目标。
        let error = service
            .apply_confirmed_with_approval_token(proposal.id(), proposal.digest(), &second)
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "AGENT_APPROVAL_TOKEN_INVALID");
        assert_eq!(count_rows(&db, "settings"), 1);
        assert_eq!(count_rows(&db, "setting_change_receipts"), 1);
    }

    #[tokio::test]
    async fn sqlite_agent_resource_patch_approval_merges_without_receipt_leak() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (service, repository) = service(&db);
        let edition_id = EditionId::new();
        let work_id = seed_edition(&db, edition_id);
        let preferences = crate::db::repos::SqliteResourcePreferenceRepository::new(db.clone());
        let existing_revision = "pref-existing".to_owned();
        let existing = EditionPreference {
            edition_id,
            data: PreferenceData {
                reading: Some(ReadingPatch {
                    custom_font_family: Some("D:\\private\\font.ttf".to_owned()),
                    ..ReadingPatch::default()
                }),
                comic: None,
            },
            revision: existing_revision.clone(),
            updated_at: UtcMillis::now(),
        };
        assert!(
            preferences
                .cas_upsert_edition(&existing, None)
                .await
                .unwrap()
        );

        let patch = PreferenceData {
            reading: Some(ReadingPatch {
                font_size: Some(ReadingFontSize::Large),
                ..ReadingPatch::default()
            }),
            comic: None,
        };
        let target = SettingTarget::edition(edition_id);
        let (proposal, _) = service
            .create_with_agent_binding(
                SettingProposalRequest::new(
                    target,
                    SettingProposalChange::AgentResourcePreferencePatch(patch.clone()),
                    SettingProvenance::agent(ProvenanceReason::AgentSuggestion),
                )
                .with_base_revision(Some(existing_revision)),
                agent_binding_input(AgentSubject::edition(
                    work_id,
                    edition_id,
                    AgentContentKind::Book,
                )),
            )
            .await
            .unwrap();

        let receipt = service
            .approve_agent_resource_proposal_in_one_uow(proposal.id(), proposal.digest(), target)
            .await
            .unwrap();
        assert!(receipt.changed);
        assert!(!receipt.before_canonical_json.contains("private"));
        assert!(!receipt.after_canonical_json.contains("font.ttf"));
        assert_eq!(
            repository
                .get(proposal.id())
                .await
                .unwrap()
                .unwrap()
                .status(),
            SettingProposalStatus::Applied
        );
        let stored = preferences.get_edition(edition_id).await.unwrap().unwrap();
        let reading = stored.data.reading.unwrap();
        assert_eq!(
            reading.custom_font_family.as_deref(),
            Some("D:\\private\\font.ttf")
        );
        assert_eq!(reading.font_size, Some(ReadingFontSize::Large));
    }

    #[tokio::test]
    async fn sqlite_agent_approval_token_consumption_rolls_back_with_late_receipt_failure() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (service, repository) = service(&db);
        let repositories = SqliteRepositories::new(db.clone());
        let (proposal, _) = service
            .create_with_agent_binding(
                SettingProposalRequest::new(
                    SettingTarget::global(SettingsSection::Reading),
                    reading_patch(ReadingFontSize::Large),
                    SettingProvenance::agent(ProvenanceReason::AgentSuggestion),
                ),
                agent_binding_input(agent_subject(&db)),
            )
            .await
            .unwrap();
        let token = service
            .issue_agent_approval_token(proposal.id(), proposal.digest())
            .await
            .unwrap();
        let token_hash = repositories
            .agent_bindings
            .get(proposal.id())
            .await
            .unwrap()
            .unwrap()
            .approval_token_hash()
            .unwrap()
            .as_str()
            .to_owned();

        db.lock()
            .execute_batch(
                "CREATE TRIGGER test_fail_agent_receipt_insert
                 BEFORE INSERT ON setting_change_receipts
                 BEGIN SELECT RAISE(ABORT, 'test receipt failure'); END;",
            )
            .unwrap();
        let error = service
            .apply_confirmed_with_approval_token(proposal.id(), proposal.digest(), &token)
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "DATABASE_ERROR");
        assert_eq!(count_rows(&db, "settings"), 0);
        assert_eq!(count_rows(&db, "setting_change_receipts"), 0);
        assert_eq!(
            repository
                .get(proposal.id())
                .await
                .unwrap()
                .unwrap()
                .status(),
            SettingProposalStatus::Pending
        );
        let after_rollback = repositories
            .agent_bindings
            .get(proposal.id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            after_rollback
                .approval_token_hash()
                .map(|hash| hash.as_str()),
            Some(token_hash.as_str())
        );
        assert!(after_rollback.issued_at().is_some());

        db.lock()
            .execute("DROP TRIGGER test_fail_agent_receipt_insert", [])
            .unwrap();
        service
            .apply_confirmed_with_approval_token(proposal.id(), proposal.digest(), &token)
            .await
            .unwrap();
        assert_eq!(count_rows(&db, "settings"), 1);
        assert_eq!(count_rows(&db, "setting_change_receipts"), 1);
    }

    #[tokio::test]
    async fn sqlite_expired_agent_proposal_is_finalized_even_after_subject_deletion() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repository = Arc::new(SqliteSettingProposalRepository::new(db.clone()));
        let uow = Arc::new(SqliteSettingProposalUow::new(db.clone()));
        let service = SettingProposalService::new(repository.clone(), uow.clone());
        let repositories = SqliteRepositories::new(db.clone());
        let subject = agent_subject(&db);
        let expired = SettingProposal::new(
            SettingProposalId::new(),
            SettingTarget::global(SettingsSection::Reading),
            reading_patch(ReadingFontSize::Small),
            None,
            SettingProvenance::agent(ProvenanceReason::AgentSuggestion),
            UtcMillis(1),
            UtcMillis(2),
        )
        .unwrap();
        let binding = AgentActionBinding::pending(
            expired.id(),
            agent_binding_input(subject),
            UtcMillis(1),
            UtcMillis(2),
        )
        .unwrap();
        uow.run(&|tx| {
            tx.insert_proposal(&expired)?;
            tx.insert_agent_action_binding(&binding)
        })
        .unwrap();
        let expired_token = AgentApprovalToken::issue();
        let expired_token_hash = expired_token.bind(expired.id(), expired.digest()).unwrap();
        uow.run(&|tx| {
            assert!(tx.issue_agent_action_approval_token(
                expired.id(),
                expired.digest(),
                &expired_token_hash,
                UtcMillis(1),
            )?);
            Ok(())
        })
        .unwrap();
        db.lock()
            .execute(
                "DELETE FROM works WHERE id = ?1",
                rusqlite::params![subject.work_id.to_string()],
            )
            .unwrap();

        let error = service
            .apply_confirmed_with_approval_token(expired.id(), expired.digest(), &expired_token)
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_EXPIRED");
        assert_eq!(
            repository
                .get(expired.id())
                .await
                .unwrap()
                .unwrap()
                .status(),
            SettingProposalStatus::Expired
        );
        assert_eq!(
            repositories
                .agent_bindings
                .get(expired.id())
                .await
                .unwrap()
                .unwrap()
                .approval_state(),
            AgentApprovalState::Expired
        );
    }

    #[tokio::test]
    async fn sqlite_agent_binding_creation_rejects_a_target_outside_subject_scope() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (service, _) = service(&db);
        let error = service
            .create_with_agent_binding(
                SettingProposalRequest::new(
                    SettingTarget::edition(EditionId::new()),
                    SettingProposalChange::AgentResourcePreferencePatch(PreferenceData {
                        reading: Some(haven_domain::settings::ReadingPatch {
                            font_size: Some(ReadingFontSize::Large),
                            ..Default::default()
                        }),
                        comic: None,
                    }),
                    SettingProvenance::agent(ProvenanceReason::AgentSuggestion),
                ),
                agent_binding_input(agent_subject(&db)),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "AGENT_ACTION_SUBJECT_MISMATCH");
        assert_eq!(count_rows(&db, "setting_proposals"), 0);
        assert_eq!(count_rows(&db, "agent_action_bindings"), 0);
    }

    #[tokio::test]
    async fn sqlite_agent_proposal_without_binding_is_an_integrity_error() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (service, repository) = service(&db);
        let proposal = SettingProposal::new(
            SettingProposalId::new(),
            SettingTarget::global(SettingsSection::Reading),
            reading_patch(ReadingFontSize::Large),
            None,
            SettingProvenance::agent(ProvenanceReason::AgentSuggestion),
            UtcMillis::now(),
            UtcMillis(UtcMillis::now().0.saturating_add(60_000)),
        )
        .unwrap();
        repository.create(&proposal).await.unwrap();

        let error = service
            .apply_confirmed(proposal.id(), proposal.digest())
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "AGENT_ACTION_BINDING_MISSING");
        assert_eq!(count_rows(&db, "settings"), 0);
    }

    #[tokio::test]
    async fn sqlite_tampered_agent_binding_state_blocks_execution() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (service, repository) = service(&db);
        let (proposal, _) = service
            .create_with_agent_binding(
                SettingProposalRequest::new(
                    SettingTarget::global(SettingsSection::Reading),
                    reading_patch(ReadingFontSize::Large),
                    SettingProvenance::agent(ProvenanceReason::AgentSuggestion),
                ),
                agent_binding_input(agent_subject(&db)),
            )
            .await
            .unwrap();
        db.lock()
            .execute(
                "UPDATE agent_action_bindings SET approval_state = 'rejected' WHERE proposal_id = ?1",
                rusqlite::params![proposal.id().to_string()],
            )
            .unwrap();

        let error = service
            .apply_confirmed(proposal.id(), proposal.digest())
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "AGENT_ACTION_BINDING_INTEGRITY");
        assert_eq!(
            repository
                .get(proposal.id())
                .await
                .unwrap()
                .unwrap()
                .status(),
            SettingProposalStatus::Pending
        );
        assert_eq!(count_rows(&db, "settings"), 0);
    }

    #[tokio::test]
    async fn sqlite_agent_receipt_readback_rejects_missing_or_tampered_binding() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (service, repository) = service(&db);
        let (proposal, _) = service
            .create_with_agent_binding(
                SettingProposalRequest::new(
                    SettingTarget::global(SettingsSection::Reading),
                    reading_patch(ReadingFontSize::Large),
                    SettingProvenance::agent(ProvenanceReason::AgentSuggestion),
                ),
                agent_binding_input(agent_subject(&db)),
            )
            .await
            .unwrap();
        let approval_token = service
            .issue_agent_approval_token(proposal.id(), proposal.digest())
            .await
            .unwrap();
        service
            .apply_confirmed_with_approval_token(proposal.id(), proposal.digest(), &approval_token)
            .await
            .unwrap();

        let original_expires_at: i64 = db
            .lock()
            .query_row(
                "SELECT expires_at FROM agent_action_bindings WHERE proposal_id = ?1",
                rusqlite::params![proposal.id().to_string()],
                |row| row.get(0),
            )
            .unwrap();
        db.lock()
            .execute(
                "UPDATE agent_action_bindings SET expires_at = ?1 WHERE proposal_id = ?2",
                rusqlite::params![original_expires_at + 1, proposal.id().to_string()],
            )
            .unwrap();
        let error = repository.get_receipt(proposal.id()).await.unwrap_err();
        assert_eq!(error.code().as_str(), "AGENT_ACTION_BINDING_INTEGRITY");
        db.lock()
            .execute(
                "UPDATE agent_action_bindings SET expires_at = ?1 WHERE proposal_id = ?2",
                rusqlite::params![original_expires_at, proposal.id().to_string()],
            )
            .unwrap();

        db.lock()
            .execute(
                "UPDATE agent_action_bindings SET approval_state = 'rejected' WHERE proposal_id = ?1",
                rusqlite::params![proposal.id().to_string()],
            )
            .unwrap();
        let error = repository.get_receipt(proposal.id()).await.unwrap_err();
        assert_eq!(error.code().as_str(), "AGENT_ACTION_BINDING_INTEGRITY");

        db.lock()
            .execute(
                "DELETE FROM agent_action_bindings WHERE proposal_id = ?1",
                rusqlite::params![proposal.id().to_string()],
            )
            .unwrap();
        let error = repository.get_receipt(proposal.id()).await.unwrap_err();
        assert_eq!(error.code().as_str(), "AGENT_ACTION_BINDING_MISSING");
    }

    #[tokio::test]
    async fn sqlite_agent_service_expiry_survives_subject_deletion_at_production_entry() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repository = Arc::new(SqliteSettingProposalRepository::new(db.clone()));
        let uow = Arc::new(SqliteSettingProposalUow::new(db.clone()));
        let setting_service = SettingProposalService::new(repository.clone(), uow);
        let repositories = Arc::new(SqliteRepositories::new(db.clone()));
        let scope = Arc::new(RepositoryAgentSubjectScope::new(
            repositories.clone(),
            repositories.clone(),
            repositories.clone(),
        ));
        let agent_service = AgentProposalService::new(setting_service, scope, repositories.clone());

        let subject = agent_subject(&db);
        let context = AgentContextSnapshot::new(
            AgentContextSnapshotId::new(),
            AgentContextPayload::new(
                subject,
                None,
                AgentLocatorConfidence::Unresolved,
                AgentBoundaryMode::UserProvided,
                "revision",
            ),
        )
        .unwrap();
        let action = agent_service
            .create_setting_proposal(
                AgentSettingsActionRequest::new(
                    AgentSessionId::new(),
                    AgentRequestId::new(),
                    &context,
                    SettingTarget::global(SettingsSection::Reading),
                    reading_patch(ReadingFontSize::Large),
                )
                .with_ttl_ms(500),
            )
            .await
            .unwrap();
        let approval_token = agent_service
            .issue_approval_token(&action, action.setting_proposal_digest())
            .await
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(600));
        db.lock()
            .execute(
                "DELETE FROM works WHERE id = ?1",
                rusqlite::params![subject.work_id.to_string()],
            )
            .unwrap();

        let error = agent_service
            .apply_confirmed(&action, action.setting_proposal_digest(), &approval_token)
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_EXPIRED");
        assert_eq!(
            repository
                .get(action.setting_proposal_id())
                .await
                .unwrap()
                .unwrap()
                .status(),
            SettingProposalStatus::Expired
        );
        assert_eq!(
            repositories
                .agent_bindings
                .get(action.setting_proposal_id())
                .await
                .unwrap()
                .unwrap()
                .approval_state(),
            AgentApprovalState::Expired
        );
        assert_eq!(count_rows(&db, "settings"), 0);
    }

    #[tokio::test]
    async fn sqlite_agent_subject_scope_checks_real_work_edition_and_media_ownership() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repositories = Arc::new(SqliteRepositories::new(db.clone()));
        let scope = RepositoryAgentSubjectScope::new(
            repositories.clone(),
            repositories.clone(),
            repositories.clone(),
        );

        let work_id = seed_work(&db);
        let other_work_id = seed_work(&db);
        let edition_id = EditionId::new();
        let other_edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
                 VALUES (?1, ?2, 'Agent 版本', 'book', 1, 1)",
                rusqlite::params![edition_id.to_string(), work_id.to_string()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
                 VALUES (?1, ?2, '其他版本', 'book', 1, 1)",
                rusqlite::params![other_edition_id.to_string(), other_work_id.to_string()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO media_items
                    (id, edition_id, media_type, title, status, created_at, updated_at)
                 VALUES (?1, ?2, 'book', 'Agent 媒体', 'available', 1, 1)",
                rusqlite::params![media_item_id.to_string(), edition_id.to_string()],
            )
            .unwrap();
        }

        scope
            .verify_subject(AgentSubject::work(work_id, AgentContentKind::Book))
            .await
            .unwrap();
        scope
            .verify_subject(AgentSubject::media_item(
                work_id,
                edition_id,
                media_item_id,
                AgentContentKind::Book,
            ))
            .await
            .unwrap();

        let error = scope
            .verify_subject(AgentSubject::work(WorkId::new(), AgentContentKind::Book))
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "AGENT_SUBJECT_WORK_NOT_FOUND");

        let error = scope
            .verify_subject(AgentSubject::edition(
                other_work_id,
                edition_id,
                AgentContentKind::Book,
            ))
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "AGENT_SUBJECT_SCOPE_MISMATCH");

        let error = scope
            .verify_subject(AgentSubject::media_item(
                work_id,
                other_edition_id,
                media_item_id,
                AgentContentKind::Book,
            ))
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "AGENT_SUBJECT_SCOPE_MISMATCH");
    }

    #[tokio::test]
    async fn sqlite_global_apply_receipt_readback_and_replay_are_idempotent() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (service, repository) = service(&db);
        let proposal = service
            .create(SettingProposalRequest::new(
                SettingTarget::global(SettingsSection::Reading),
                reading_patch(ReadingFontSize::Large),
                provenance(),
            ))
            .await
            .unwrap();

        let receipt = service
            .apply_confirmed(proposal.id(), proposal.digest())
            .await
            .unwrap();
        assert!(receipt.changed);
        assert!(receipt.applied_revision.is_some());
        assert_eq!(receipt.proposal_id, proposal.id());

        let stored_proposal = repository.get(proposal.id()).await.unwrap().unwrap();
        assert_eq!(stored_proposal.status(), SettingProposalStatus::Applied);
        let stored_receipt = repository
            .get_receipt(proposal.id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored_receipt, receipt);
        assert_eq!(count_rows(&db, "setting_change_receipts"), 1);

        let replayed = service
            .apply_confirmed(proposal.id(), proposal.digest())
            .await
            .unwrap();
        assert_eq!(replayed.id, receipt.id);
        assert_eq!(replayed.applied_revision, receipt.applied_revision);
        assert_eq!(count_rows(&db, "setting_change_receipts"), 1);
        assert_eq!(count_rows(&db, "settings"), 1);

        let settings = crate::db::repos::SqliteSettingsRepository::new(db.clone());
        let row = settings.get("reading").await.unwrap().unwrap();
        let value: haven_domain::settings::SettingsValue =
            serde_json::from_str(&row.data_json).unwrap();
        match value {
            haven_domain::settings::SettingsValue::Reading(value) => {
                assert_eq!(value.font_size, ReadingFontSize::Large);
            }
            other => panic!("unexpected settings section: {other:?}"),
        }
    }

    #[tokio::test]
    async fn sqlite_revision_conflict_leaves_pending_proposal_and_no_receipt() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (service, repository) = service(&db);
        let settings = crate::db::repos::SqliteSettingsRepository::new(db.clone());
        let baseline = haven_domain::settings::SettingsValue::default_for(SettingsSection::Reading);
        settings
            .upsert(&SettingsRow {
                section: "reading".to_owned(),
                schema_version: 1,
                revision: "set-baseline".to_owned(),
                data_json: serde_json::to_string(&baseline).unwrap(),
                updated_at: UtcMillis::now(),
            })
            .await
            .unwrap();

        let proposal = service
            .create(
                SettingProposalRequest::new(
                    SettingTarget::global(SettingsSection::Reading),
                    reading_patch(ReadingFontSize::Large),
                    provenance(),
                )
                .with_base_revision(Some("set-baseline".to_owned())),
            )
            .await
            .unwrap();

        settings
            .upsert(&SettingsRow {
                section: "reading".to_owned(),
                schema_version: 1,
                revision: "set-other-session".to_owned(),
                data_json: serde_json::to_string(&baseline).unwrap(),
                updated_at: UtcMillis::now(),
            })
            .await
            .unwrap();

        let error = service
            .apply_confirmed(proposal.id(), proposal.digest())
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "REVISION_CONFLICT");
        assert_eq!(
            repository
                .get(proposal.id())
                .await
                .unwrap()
                .unwrap()
                .status(),
            SettingProposalStatus::Pending
        );
        assert!(
            repository
                .get_receipt(proposal.id())
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(count_rows(&db, "setting_change_receipts"), 0);
        assert_eq!(
            settings.get("reading").await.unwrap().unwrap().revision,
            "set-other-session"
        );
    }

    #[tokio::test]
    async fn sqlite_expired_apply_commits_expired_status_without_target_or_receipt() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (service, repository) = service(&db);
        let expired = SettingProposal::new(
            SettingProposalId::new(),
            SettingTarget::global(SettingsSection::Reading),
            reading_patch(ReadingFontSize::Small),
            None,
            provenance(),
            UtcMillis(1),
            UtcMillis(2),
        )
        .unwrap();
        repository.create(&expired).await.unwrap();

        let error = service
            .apply_confirmed(expired.id(), expired.digest())
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_EXPIRED");
        assert_eq!(
            repository
                .get(expired.id())
                .await
                .unwrap()
                .unwrap()
                .status(),
            SettingProposalStatus::Expired
        );
        assert!(
            repository
                .get_receipt(expired.id())
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(count_rows(&db, "setting_change_receipts"), 0);
        assert_eq!(count_rows(&db, "settings"), 0);
    }

    #[tokio::test]
    async fn sqlite_receipt_readback_rejects_tampered_parent_state_and_provenance() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (service, repository) = service(&db);
        let proposal = service
            .create(SettingProposalRequest::new(
                SettingTarget::global(SettingsSection::Reading),
                reading_patch(ReadingFontSize::Large),
                provenance(),
            ))
            .await
            .unwrap();
        let receipt = service
            .apply_confirmed(proposal.id(), proposal.digest())
            .await
            .unwrap();

        // Receipt 不能成为 pending/rejected/expired 提案的伪凭证。
        db.lock()
            .execute(
                "UPDATE setting_proposals SET status = 'pending' WHERE id = ?1",
                rusqlite::params![proposal.id().to_string()],
            )
            .unwrap();
        let error = repository.get_receipt(proposal.id()).await.unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_INTEGRITY_MISMATCH");

        // 恢复父状态后，来源 JSON 只要不是 canonical，仍必须被拒绝。
        db.lock()
            .execute(
                "UPDATE setting_proposals SET status = 'applied' WHERE id = ?1",
                rusqlite::params![proposal.id().to_string()],
            )
            .unwrap();
        let non_canonical_provenance = serde_json::to_string(&receipt.provenance).unwrap();
        db.lock()
            .execute(
                "UPDATE setting_change_receipts SET provenance_json = ?1 WHERE proposal_id = ?2",
                rusqlite::params![non_canonical_provenance, proposal.id().to_string()],
            )
            .unwrap();
        let error = repository.get_receipt(proposal.id()).await.unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_INTEGRITY_MISMATCH");
    }

    #[tokio::test]
    async fn sqlite_media_item_apply_records_scoped_receipt_and_replays() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (service, repository) = service(&db);
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        seed_edition(&db, edition_id);
        seed_media_item(&db, media_item_id, edition_id);

        let target = SettingTarget::media_item(edition_id, media_item_id);
        let data = resource_preference();
        let proposal = service
            .create(SettingProposalRequest::new(
                target,
                SettingProposalChange::ResourcePreference(data.clone()),
                provenance(),
            ))
            .await
            .unwrap();

        let receipt = service
            .apply_confirmed(proposal.id(), proposal.digest())
            .await
            .unwrap();
        assert!(receipt.changed);
        assert_eq!(receipt.target, target);
        // before/after 落在资源级类型上：before 是该作用域此前的空覆盖。
        assert_eq!(
            receipt.before_canonical_json,
            canonical_json_of(&PreferenceData::default()).unwrap()
        );
        assert_eq!(
            receipt.after_canonical_json,
            canonical_json_of(&data).unwrap()
        );

        // 回执从存储读回必须与内存里那张逐字段一致（含冗余 target 列与父提案交叉校验）。
        let stored_receipt = repository
            .get_receipt(proposal.id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored_receipt, receipt);

        let preferences = crate::db::repos::SqliteResourcePreferenceRepository::new(db.clone());
        let stored = preferences
            .get_media_item(media_item_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.edition_id, edition_id);
        assert_eq!(stored.data, data);
        assert_eq!(stored.revision, receipt.applied_revision.clone().unwrap());

        // 幂等重放：同一张回执，不再写目标、不再插回执。
        let replayed = service
            .apply_confirmed(proposal.id(), proposal.digest())
            .await
            .unwrap();
        assert_eq!(replayed.id, receipt.id);
        assert_eq!(replayed.applied_revision, receipt.applied_revision);
        assert_eq!(count_rows(&db, "setting_change_receipts"), 1);
        assert_eq!(count_rows(&db, "media_item_preferences"), 1);
    }

    #[tokio::test]
    async fn sqlite_media_item_target_must_belong_to_the_proposed_edition() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (service, repository) = service(&db);
        let edition_id = EditionId::new();
        let stranger = EditionId::new();
        let media_item_id = MediaItemId::new();
        seed_edition(&db, edition_id);
        seed_edition(&db, stranger);
        seed_media_item(&db, media_item_id, stranger);

        let proposal = service
            .create(SettingProposalRequest::new(
                SettingTarget::media_item(edition_id, media_item_id),
                SettingProposalChange::ResourcePreference(resource_preference()),
                provenance(),
            ))
            .await
            .unwrap();

        let error = service
            .apply_confirmed(proposal.id(), proposal.digest())
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_TARGET_NOT_FOUND");
        assert_eq!(count_rows(&db, "setting_change_receipts"), 0);
        assert_eq!(count_rows(&db, "media_item_preferences"), 0);
        assert_eq!(
            repository
                .get(proposal.id())
                .await
                .unwrap()
                .unwrap()
                .status(),
            SettingProposalStatus::Pending
        );

        // 媒体条目根本不存在：同样是稳定的 target-not-found，而不是"写出一条孤儿偏好"。
        let missing = MediaItemId::new();
        let proposal = service
            .create(SettingProposalRequest::new(
                SettingTarget::media_item(edition_id, missing),
                SettingProposalChange::ResourcePreference(resource_preference()),
                provenance(),
            ))
            .await
            .unwrap();
        let error = service
            .apply_confirmed(proposal.id(), proposal.digest())
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_TARGET_NOT_FOUND");
        assert_eq!(count_rows(&db, "media_item_preferences"), 0);
    }

    #[tokio::test]
    async fn sqlite_edition_apply_requires_an_existing_edition_and_is_idempotent() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (service, repository) = service(&db);

        // 目标版本不存在：零写入、无回执、提案仍在 pending。
        let missing = service
            .create(SettingProposalRequest::new(
                SettingTarget::edition(EditionId::new()),
                SettingProposalChange::ResourcePreference(resource_preference()),
                provenance(),
            ))
            .await
            .unwrap();
        let error = service
            .apply_confirmed(missing.id(), missing.digest())
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_TARGET_NOT_FOUND");
        assert_eq!(count_rows(&db, "edition_preferences"), 0);
        assert_eq!(count_rows(&db, "setting_change_receipts"), 0);

        let edition_id = EditionId::new();
        seed_edition(&db, edition_id);
        let data = resource_preference();
        let proposal = service
            .create(SettingProposalRequest::new(
                SettingTarget::edition(edition_id),
                SettingProposalChange::ResourcePreference(data.clone()),
                provenance(),
            ))
            .await
            .unwrap();
        let receipt = service
            .apply_confirmed(proposal.id(), proposal.digest())
            .await
            .unwrap();
        assert!(receipt.changed);
        assert_eq!(receipt.target, SettingTarget::edition(edition_id));
        assert_eq!(
            repository
                .get_receipt(proposal.id())
                .await
                .unwrap()
                .unwrap(),
            receipt
        );

        let preferences = crate::db::repos::SqliteResourcePreferenceRepository::new(db.clone());
        let stored = preferences.get_edition(edition_id).await.unwrap().unwrap();
        assert_eq!(stored.data, data);
        assert_eq!(stored.revision, receipt.applied_revision.clone().unwrap());
    }

    /// 回执是"提案已生效"的唯一凭证：一条自称 applied 却拿不出回执的行，
    /// 读取端必须和 `apply_confirmed` 一样报完整性错误，而不是把它当成"尚未应用"。
    #[tokio::test]
    async fn sqlite_applied_proposal_without_receipt_is_an_integrity_error_on_read() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (service, repository) = service(&db);
        let proposal = service
            .create(SettingProposalRequest::new(
                SettingTarget::global(SettingsSection::Reading),
                reading_patch(ReadingFontSize::Large),
                provenance(),
            ))
            .await
            .unwrap();
        service
            .apply_confirmed(proposal.id(), proposal.digest())
            .await
            .unwrap();

        db.lock()
            .execute(
                "DELETE FROM setting_change_receipts WHERE proposal_id = ?1",
                rusqlite::params![proposal.id().to_string()],
            )
            .unwrap();

        let error = repository.get_receipt(proposal.id()).await.unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_INTEGRITY_MISMATCH");

        // 提案不存在仍然是"没有回执"，不是完整性错误（与 `get` 的 None 语义一致）。
        assert!(
            repository
                .get_receipt(SettingProposalId::new())
                .await
                .unwrap()
                .is_none()
        );
    }

    /// `BEGIN IMMEDIATE` 的原子性：两个连接同时确认同一条提案，只能有一次目标写入、
    /// 一张回执，后到者拿到的是同一张已存回执（幂等重放），而不是第二次执行。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sqlite_concurrent_confirm_on_two_connections_writes_once() {
        let dir =
            std::env::temp_dir().join(format!("haven-setting-proposal-{}", UtcMillis::now().0));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("proposals.db");

        let first_db = Arc::new(Db::open(&path).unwrap());
        let second_db = Arc::new(Db::open(&path).unwrap());
        let (first_service, first_repository) = service(&first_db);
        let (second_service, _) = service(&second_db);

        let proposal = first_service
            .create(SettingProposalRequest::new(
                SettingTarget::global(SettingsSection::Reading),
                reading_patch(ReadingFontSize::Large),
                provenance(),
            ))
            .await
            .unwrap();

        let (left, right) = tokio::join!(
            first_service.apply_confirmed(proposal.id(), proposal.digest()),
            second_service.apply_confirmed(proposal.id(), proposal.digest()),
        );
        let left = left.unwrap();
        let right = right.unwrap();
        assert_eq!(left.id, right.id, "并发确认必须收敛到同一张回执");
        assert_eq!(left.applied_revision, right.applied_revision);
        assert!(left.changed && right.changed);
        assert_eq!(count_rows(&first_db, "setting_change_receipts"), 1);
        assert_eq!(
            first_repository
                .get_receipt(proposal.id())
                .await
                .unwrap()
                .unwrap(),
            left
        );

        drop((first_db, second_db));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
