//! 内置 Agent Skill 启用状态的 SQLite Repository（原生 Skill 运行时）。
//!
//! 规范：[`docs/architecture/AI_SYSTEM.md`](../../../../../docs/architecture/AI_SYSTEM.md) §6。
//!
//! 边界：
//! - 只读写 `agent_skill_states`。**没有**正文列、没有提示词列、没有路径列——
//!   技能正文随二进制分发，仓储不持有它。
//! - 读取端做完整校验：脏行（非法 id、非法摘要、越界 `enabled`）必须报错，
//!   不得被静默降级成一条"看起来能用"的记录。这与 AI Provider Profile 仓储同规则：
//!   脏行被降级读取，等价于让一个不受控的值进入模型请求。
//! - `put` 是幂等覆盖：`(skill_id, enabled, instructions_hash)` 三者都没变时
//!   不更新 `updated_at`，避免 UI 重复点击伪造出"刚刚改过"的时间线。

use std::sync::Arc;

use async_trait::async_trait;
use rusqlite::OptionalExtension;

use haven_common::{AppError, ErrorKind};
use haven_domain::agent_skill::{AgentSkillEnablement, AgentSkillId};
use haven_domain::contracts::AgentSkillStateRepository;

use crate::db::Db;
use crate::db::repos::map_db_error;

const STATE_COLUMNS: &str = "skill_id, instructions_hash, enabled, updated_at";

pub struct SqliteAgentSkillStateRepository {
    db: Arc<Db>,
}

impl SqliteAgentSkillStateRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl AgentSkillStateRepository for SqliteAgentSkillStateRepository {
    async fn list(&self) -> Result<Vec<AgentSkillEnablement>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {STATE_COLUMNS} FROM agent_skill_states ORDER BY skill_id ASC"
            ))
            .map_err(map_db_error("查询技能启用状态失败"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>("skill_id")?,
                    row.get::<_, String>("instructions_hash")?,
                    row.get::<_, i64>("enabled")?,
                ))
            })
            .map_err(map_db_error("查询技能启用状态失败"))?;
        let mut states = Vec::new();
        for row in rows {
            let (skill_id, instructions_hash, enabled) =
                row.map_err(map_db_error("查询技能启用状态失败"))?;
            states.push(into_enablement(&skill_id, &instructions_hash, enabled)?);
        }
        Ok(states)
    }

    async fn get(&self, skill_id: &AgentSkillId) -> Result<Option<AgentSkillEnablement>, AppError> {
        let conn = self.db.lock();
        let stored = conn
            .query_row(
                &format!("SELECT {STATE_COLUMNS} FROM agent_skill_states WHERE skill_id = ?1"),
                rusqlite::params![skill_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>("instructions_hash")?,
                        row.get::<_, i64>("enabled")?,
                    ))
                },
            )
            .optional()
            .map_err(map_db_error("查询技能启用状态失败"))?;
        match stored {
            None => Ok(None),
            Some((instructions_hash, enabled)) => Ok(Some(into_enablement(
                skill_id.as_str(),
                &instructions_hash,
                enabled,
            )?)),
        }
    }

    /// 幂等覆盖写入。
    ///
    /// `ON CONFLICT DO UPDATE ... WHERE` 的谓词比较的是**已有行与目标行是否真的不同**：
    /// 完全相同时 `affected == 0`，`updated_at` 保持不变，调用方看到的仍是最初那次
    /// 用户操作的时间。这与 CAS 不同——这里不需要版本号，因为"用户又点了一次同一个开关"
    /// 不是冲突，只是没有新事实。
    async fn put(
        &self,
        enablement: &AgentSkillEnablement,
        updated_at_ms: i64,
    ) -> Result<(), AppError> {
        let conn = self.db.lock();
        conn.execute(
            "INSERT INTO agent_skill_states (skill_id, instructions_hash, enabled, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(skill_id) DO UPDATE SET
                 instructions_hash = excluded.instructions_hash,
                 enabled = excluded.enabled,
                 updated_at = excluded.updated_at
             WHERE agent_skill_states.instructions_hash <> excluded.instructions_hash
                OR agent_skill_states.enabled <> excluded.enabled",
            rusqlite::params![
                enablement.skill_id().as_str(),
                enablement.instructions_hash(),
                if enablement.enabled() { 1_i64 } else { 0_i64 },
                updated_at_ms,
            ],
        )
        .map_err(map_db_error("写入技能启用状态失败"))?;
        Ok(())
    }
}

/// 把一行还原成领域值。任一字段不合法即报错——**不**回退成默认值。
fn into_enablement(
    skill_id: &str,
    instructions_hash: &str,
    enabled: i64,
) -> Result<AgentSkillEnablement, AppError> {
    let id = AgentSkillId::parse(skill_id).map_err(|_| corrupt_state())?;
    let enabled = match enabled {
        0 => false,
        1 => true,
        _ => return Err(corrupt_state()),
    };
    AgentSkillEnablement::parse(id, instructions_hash, enabled).map_err(|_| corrupt_state())
}

/// 行本身不自洽。这是一个**完整性问题**而不是用户输入错误：
/// 调用方没有做错任何事，继续跑下去却会把未经验证的值当成用户同意。
fn corrupt_state() -> AppError {
    AppError::new(
        "AGENT_SKILL_STATE_CORRUPT",
        ErrorKind::Internal,
        "技能启用状态记录不自洽",
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> SqliteAgentSkillStateRepository {
        let db = Arc::new(Db::open_in_memory().expect("内存库必须可迁移"));
        SqliteAgentSkillStateRepository::new(db)
    }

    fn id(raw: &str) -> AgentSkillId {
        AgentSkillId::parse(raw).expect("测试 id 必须合法")
    }

    fn enablement(raw: &str, hash: &str, enabled: bool) -> AgentSkillEnablement {
        AgentSkillEnablement::parse(id(raw), hash, enabled).expect("测试记录必须合法")
    }

    #[tokio::test]
    async fn missing_state_is_none_not_a_default() {
        let repo = repo();
        assert!(
            repo.get(&id("haven-agent-proposal"))
                .await
                .unwrap()
                .is_none()
        );
        assert!(repo.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn put_then_get_round_trips_the_content_hash() {
        let repo = repo();
        let hash = "a".repeat(64);
        repo.put(&enablement("haven-agent-proposal", &hash, true), 1_000)
            .await
            .unwrap();
        let stored = repo
            .get(&id("haven-agent-proposal"))
            .await
            .unwrap()
            .unwrap();
        assert!(stored.enabled());
        assert_eq!(stored.instructions_hash(), hash);
        assert_eq!(repo.list().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn disabling_is_a_record_not_a_deletion() {
        let repo = repo();
        let hash = "a".repeat(64);
        repo.put(&enablement("haven-agent-proposal", &hash, true), 1_000)
            .await
            .unwrap();
        repo.put(&enablement("haven-agent-proposal", &hash, false), 2_000)
            .await
            .unwrap();
        let stored = repo
            .get(&id("haven-agent-proposal"))
            .await
            .unwrap()
            .unwrap();
        assert!(!stored.enabled());
        // 行仍在：禁用是可读的历史事实，不是抹掉证据。
        assert_eq!(repo.list().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn put_is_idempotent_and_preserves_the_original_timestamp() {
        let repo = repo();
        let hash = "a".repeat(64);
        repo.put(&enablement("haven-agent-proposal", &hash, true), 1_000)
            .await
            .unwrap();
        repo.put(&enablement("haven-agent-proposal", &hash, true), 9_999)
            .await
            .unwrap();
        let conn = repo.db.lock();
        let updated_at: i64 = conn
            .query_row(
                "SELECT updated_at FROM agent_skill_states WHERE skill_id = ?1",
                rusqlite::params!["haven-agent-proposal"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(updated_at, 1_000, "重复写同样的状态不得改写时间戳");
    }

    #[tokio::test]
    async fn a_new_content_hash_is_a_new_fact_and_updates_the_timestamp() {
        let repo = repo();
        repo.put(
            &enablement("haven-agent-proposal", &"a".repeat(64), true),
            1_000,
        )
        .await
        .unwrap();
        repo.put(
            &enablement("haven-agent-proposal", &"b".repeat(64), true),
            2_000,
        )
        .await
        .unwrap();
        let stored = repo
            .get(&id("haven-agent-proposal"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.instructions_hash(), "b".repeat(64));
    }

    /// 重启保真：状态落在磁盘上，进程重新打开数据库之后读到的是同一件事。
    ///
    /// 这条用例刻意用**真实文件**库而不是内存库——内存库在同一个句柄里读写永远成功，
    /// 无法区分"持久化"与"缓存在连接里"。它覆盖的是验收里
    /// 「启用状态在重启后仍然成立」这一条：重建的是 `Db`，不是 `SqliteRepositories`。
    #[tokio::test]
    async fn enabled_state_survives_reopening_the_database() {
        let directory = tempfile::tempdir().expect("临时目录必须可建");
        let path = directory.path().join("haven.db");
        let hash = "c".repeat(64);

        {
            let repo = SqliteAgentSkillStateRepository::new(Arc::new(
                Db::open(&path).expect("首次打开必须可迁移"),
            ));
            assert!(
                repo.get(&id("haven-agent-proposal"))
                    .await
                    .unwrap()
                    .is_none(),
                "全新数据库里没有任何启用记录"
            );
            repo.put(&enablement("haven-agent-proposal", &hash, true), 1_000)
                .await
                .unwrap();
            // 句柄在这里析构：下面的读取只能来自磁盘。
        }

        let repo = SqliteAgentSkillStateRepository::new(Arc::new(
            Db::open(&path).expect("重新打开必须成功"),
        ));
        let stored = repo
            .get(&id("haven-agent-proposal"))
            .await
            .unwrap()
            .expect("重新打开后启用记录必须仍在");
        assert!(stored.enabled());
        assert_eq!(stored.instructions_hash(), hash);
        assert_eq!(repo.list().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_corrupt_row_is_an_error_not_a_downgrade() {
        let repo = repo();
        // 长度合法但不是小写十六进制：通过数据库 CHECK，专门验证读取端完整性守卫。
        let invalid_hash = "Z".repeat(64);
        {
            let conn = repo.db.lock();
            conn.execute(
                "INSERT INTO agent_skill_states (skill_id, instructions_hash, enabled, updated_at)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params!["haven-agent-proposal", invalid_hash, 1, 1_000],
            )
            .unwrap();
        }
        let error = repo
            .get(&id("haven-agent-proposal"))
            .await
            .expect_err("脏行必须报错");
        assert_eq!(error.code().as_str(), "AGENT_SKILL_STATE_CORRUPT");
        assert!(!error.retryable());
    }
}
