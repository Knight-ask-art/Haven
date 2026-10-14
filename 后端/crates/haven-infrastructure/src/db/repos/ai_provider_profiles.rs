//! AI Provider Profile SQLite Repository（A2 基础切片）。
//!
//! 规范：[`docs/architecture/AI_SYSTEM.md`](../../../../../docs/architecture/AI_SYSTEM.md) §3、§4。
//!
//! 边界：
//! - 只读写 `ai_provider_profiles`（非敏感配置）。**没有**任何 secret / credential_ref
//!   列，也不提供访问 CredentialStore 的能力——凭据清理由 Application 用例编排。
//! - 写路径全部是数据库层条件写（CAS）：`expected_revision` 决定哪条语句可以落地
//!   （`None` = 仅当行不存在时创建，`Some` = 仅当行存在且版本匹配时更新），
//!   条件与写入在同一条 SQL 里，`affected == 0` 即冲突——不把"先读再写"的
//!   竞态留给调用方。
//! - 读取端做完整校验：脏行（未知 kind、非法 id / endpoint、非法 revision）必须报错，
//!   不得被静默降级成一个"看起来能用"的 profile。

use std::sync::Arc;

use async_trait::async_trait;
use rusqlite::OptionalExtension;

use haven_common::{AppError, ErrorKind};
use haven_domain::ai_provider::{
    AiProviderKind, AiProviderProfile, AiProviderProfileDeleteOutcome,
};
use haven_domain::contracts::AiProviderProfileRepository;

use crate::db::Db;
use crate::db::repos::map_db_error;

const PROFILE_COLUMNS: &str = "profile_id, kind, display_name, endpoint, enabled, \
     selected_model_id, revision, created_at, updated_at";

pub struct SqliteAiProviderProfileRepository {
    db: Arc<Db>,
}

impl SqliteAiProviderProfileRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl AiProviderProfileRepository for SqliteAiProviderProfileRepository {
    async fn list(&self) -> Result<Vec<AiProviderProfile>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {PROFILE_COLUMNS} FROM ai_provider_profiles ORDER BY profile_id ASC"
            ))
            .map_err(map_db_error("查询 AI Provider 配置失败"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(StoredRow {
                    profile_id: row.get("profile_id")?,
                    kind: row.get("kind")?,
                    display_name: row.get("display_name")?,
                    endpoint: row.get("endpoint")?,
                    enabled: row.get::<_, i64>("enabled")?,
                    selected_model_id: row.get("selected_model_id")?,
                    revision: row.get("revision")?,
                    created_at: row.get("created_at")?,
                    updated_at: row.get("updated_at")?,
                })
            })
            .map_err(map_db_error("查询 AI Provider 配置失败"))?;
        let mut profiles = Vec::new();
        for row in rows {
            let row = row.map_err(map_db_error("查询 AI Provider 配置失败"))?;
            profiles.push(row.into_profile()?);
        }
        Ok(profiles)
    }

    async fn get(&self, profile_id: &str) -> Result<Option<AiProviderProfile>, AppError> {
        let conn = self.db.lock();
        let stored = conn
            .query_row(
                &format!(
                    "SELECT {PROFILE_COLUMNS} FROM ai_provider_profiles WHERE profile_id = ?1"
                ),
                rusqlite::params![profile_id],
                read_row,
            )
            .optional()
            .map_err(map_db_error("查询 AI Provider 配置失败"))?;
        stored.map(StoredRow::into_profile).transpose()
    }

    /// 条件写入。`expected_revision` 决定**哪一条语句**可以落地，而不是交给
    /// `ON CONFLICT` 的冲突分支去兜底：
    /// - `expected = None` → `INSERT ... ON CONFLICT(profile_id) DO NOTHING`：
    ///   只允许"行不存在"时创建；行已存在 → 0 行受影响 → 返回 `None`。
    /// - `expected = Some(revision)` → `UPDATE ... WHERE profile_id = ?1 AND revision = ?2`：
    ///   只允许改**已存在且版本匹配**的行；版本被并发方推进 → 0 行受影响 → `None`，
    ///   行已被删除（读之后删掉）同样 0 行 → `None`，**不会**被重新造出来。
    ///
    /// 两条分支各自都是单条条件语句：条件与写入之间没有可插入的窗口，不需要
    /// "先检查再写"。**不能**把两种情况合并回
    /// `INSERT ... ON CONFLICT DO UPDATE ... WHERE revision = expected`：那里的谓词只在
    /// 冲突分支生效，于是 `Some(revision)` 遇到"行已被删除"时会走无冲突的 INSERT 分支，
    /// 用过期版本把已删除的 profile 复活——这正是 CAS 要禁止的。
    ///
    /// `created_at` 不在 UPDATE 的 SET 列表里：更新不得改写创建时间。正因如此，
    /// 成功路径必须**回读**持久化行再返回：调用方拿到的候选值带着 `created_at = now`，
    /// 直接回传会让响应与权威状态不一致。回读与写入共用同一次 `Db::lock()`，
    /// 因此不会读到另一个写者的中间结果。
    async fn cas_upsert(
        &self,
        profile: &AiProviderProfile,
        expected_revision: Option<&str>,
    ) -> Result<Option<AiProviderProfile>, AppError> {
        let revision = new_revision();
        let conn = self.db.lock();
        let affected = match expected_revision {
            // 创建：只在行确实不存在时落地。`DO NOTHING` 把"行已存在"表达成
            // 0 行受影响（= 冲突），而不是把已有行覆盖成候选值。
            None => conn.execute(
                "INSERT INTO ai_provider_profiles
                    (profile_id, kind, display_name, endpoint, enabled, selected_model_id,
                     revision, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(profile_id) DO NOTHING",
                rusqlite::params![
                    profile.profile_id(),
                    profile.kind().as_str(),
                    profile.display_name(),
                    profile.endpoint(),
                    i64::from(profile.enabled()),
                    profile.selected_model_id(),
                    revision,
                    profile.created_at(),
                    profile.updated_at(),
                ],
            ),
            // 更新：只在行存在且版本匹配时落地。行不存在时不写、不建行，
            // 因此"删除后的过期保存"只可能冲突。
            Some(expected) => conn.execute(
                "UPDATE ai_provider_profiles SET
                     kind = ?2,
                     display_name = ?3,
                     endpoint = ?4,
                     enabled = ?5,
                     selected_model_id = ?6,
                     revision = ?7,
                     updated_at = ?8
                 WHERE profile_id = ?1 AND revision = ?9",
                rusqlite::params![
                    profile.profile_id(),
                    profile.kind().as_str(),
                    profile.display_name(),
                    profile.endpoint(),
                    i64::from(profile.enabled()),
                    profile.selected_model_id(),
                    revision,
                    profile.updated_at(),
                    expected,
                ],
            ),
        }
        .map_err(map_db_error("保存 AI Provider 配置失败"))?;
        if affected == 0 {
            return Ok(None);
        }
        let stored = conn
            .query_row(
                &format!(
                    "SELECT {PROFILE_COLUMNS} FROM ai_provider_profiles WHERE profile_id = ?1"
                ),
                rusqlite::params![profile.profile_id()],
                read_row,
            )
            .optional()
            .map_err(map_db_error("查询 AI Provider 配置失败"))?
            .ok_or_else(|| {
                AppError::new(
                    "AI_PROVIDER_PROFILE_CORRUPT",
                    ErrorKind::Database,
                    "AI Provider 配置写入后无法回读",
                    false,
                )
            })?;
        stored.into_profile().map(Some)
    }

    /// 条件删除。用一条 DELETE 的 `WHERE revision = ?` 完成，
    /// 因此不存在"检查与删除之间被改写"的窗口；`affected == 0` 时再区分
    /// "不存在"与"版本冲突"，让调用方拿到可行动的结果而不是布尔值。
    async fn cas_delete(
        &self,
        profile_id: &str,
        expected_revision: Option<&str>,
    ) -> Result<AiProviderProfileDeleteOutcome, AppError> {
        let conn = self.db.lock();
        let removed = conn
            .execute(
                "DELETE FROM ai_provider_profiles WHERE profile_id = ?1 AND revision = ?2",
                rusqlite::params![profile_id, expected_revision],
            )
            .map_err(map_db_error("删除 AI Provider 配置失败"))?;
        if removed > 0 {
            return Ok(AiProviderProfileDeleteOutcome::Deleted);
        }
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM ai_provider_profiles WHERE profile_id = ?1",
                rusqlite::params![profile_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_db_error("查询 AI Provider 配置失败"))?;
        Ok(match exists {
            Some(_) => AiProviderProfileDeleteOutcome::RevisionConflict,
            None => AiProviderProfileDeleteOutcome::NotFound,
        })
    }
}

/// CAS 用版本号。v4 随机即可：它只需要唯一且不可预测，不需要排序语义。
fn new_revision() -> String {
    uuid::Uuid::new_v4().to_string()
}

struct StoredRow {
    profile_id: String,
    kind: String,
    display_name: String,
    endpoint: String,
    enabled: i64,
    selected_model_id: Option<String>,
    revision: String,
    created_at: i64,
    updated_at: i64,
}

impl StoredRow {
    fn into_profile(self) -> Result<AiProviderProfile, AppError> {
        let kind =
            AiProviderKind::parse(&self.kind).ok_or_else(|| corrupt("未知的 Provider 种类"))?;
        AiProviderProfile::from_stored(
            self.profile_id,
            self.display_name,
            kind,
            self.endpoint,
            self.enabled != 0,
            self.selected_model_id,
            self.revision,
            self.created_at,
            self.updated_at,
        )
        .map_err(|error| corrupt_with(error.user_message()))
    }
}

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredRow> {
    Ok(StoredRow {
        profile_id: row.get("profile_id")?,
        kind: row.get("kind")?,
        display_name: row.get("display_name")?,
        endpoint: row.get("endpoint")?,
        enabled: row.get("enabled")?,
        selected_model_id: row.get("selected_model_id")?,
        revision: row.get("revision")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn corrupt(message: &'static str) -> AppError {
    corrupt_with(message)
}

fn corrupt_with(message: &str) -> AppError {
    AppError::new(
        "AI_PROVIDER_PROFILE_CORRUPT",
        ErrorKind::Database,
        format!("AI Provider 配置行无法解析：{message}"),
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::ai_provider::AiProviderProfile;

    fn repo() -> (SqliteAiProviderProfileRepository, Arc<Db>) {
        let db = Arc::new(Db::open_in_memory().expect("内存库必须可用"));
        (SqliteAiProviderProfileRepository::new(db.clone()), db)
    }

    fn profile(id: &str, endpoint: &str) -> AiProviderProfile {
        AiProviderProfile::new(
            id,
            "自建网关",
            AiProviderKind::OpenAiCompatible,
            endpoint,
            true,
            None,
            1_700_000_000_000,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn cas_upsert_creates_then_conflicts_on_stale_revision() {
        let (repo, _db) = repo();
        assert!(repo.list().await.unwrap().is_empty());

        let candidate = profile("gw", "https://gateway.example.invalid/v1");
        let written = repo.cas_upsert(&candidate, None).await.unwrap().unwrap();
        let revision = written.revision().expect("写入必须返回持久化行").to_owned();
        assert!(!revision.is_empty());
        // 返回的必须是**持久化后**的行（回读），而不是调用方传进来的候选值。
        assert_eq!(repo.get("gw").await.unwrap().unwrap(), written);

        // 再次以 expected=None 写入必须冲突（行已存在）。
        assert!(
            repo.cas_upsert(&candidate, None).await.unwrap().is_none(),
            "expected=None 且行已存在必须冲突"
        );
        // 用过期版本也必须冲突。
        assert!(
            repo.cas_upsert(&candidate, Some("stale"))
                .await
                .unwrap()
                .is_none()
        );

        let stored = repo.get("gw").await.unwrap().unwrap();
        assert_eq!(stored.revision(), Some(revision.as_str()));
        assert_eq!(stored.created_at(), 1_700_000_000_000);

        // 正确版本更新成功，且不改写 created_at。
        let updated = AiProviderProfile::new(
            "gw",
            "改名后的网关",
            AiProviderKind::OpenAiCompatible,
            "https://gateway.example.invalid/v2",
            false,
            Some("gpt-4o-mini".into()),
            1_700_000_999_000,
        )
        .unwrap();
        let next = repo
            .cas_upsert(&updated, Some(&revision))
            .await
            .unwrap()
            .unwrap();
        assert_ne!(next.revision(), Some(revision.as_str()));
        // 返回行必须是数据库里的权威行：`created_at` 保留原值，而不是候选值里的 `now`。
        assert_eq!(
            next.created_at(),
            1_700_000_000_000,
            "回读行必须保留数据库里的创建时间"
        );
        let stored = repo.get("gw").await.unwrap().unwrap();
        assert_eq!(stored, next, "cas_upsert 返回行必须等于随后的 get");
        assert_eq!(stored.display_name(), "改名后的网关");
        assert_eq!(stored.endpoint(), "https://gateway.example.invalid/v2");
        assert!(!stored.enabled());
        assert_eq!(stored.selected_model_id(), Some("gpt-4o-mini"));
        assert_eq!(
            stored.created_at(),
            1_700_000_000_000,
            "更新不得改写创建时间"
        );
    }

    #[tokio::test]
    async fn cas_upsert_with_revision_on_missing_row_conflicts_without_creating() {
        let (repo, _db) = repo();
        let candidate = profile("gw", "https://gateway.example.invalid/v1");
        assert!(
            repo.cas_upsert(&candidate, Some("rev-1"))
                .await
                .unwrap()
                .is_none()
        );
        assert!(repo.get("gw").await.unwrap().is_none());
        assert!(repo.cas_upsert(&candidate, None).await.unwrap().is_some());
    }

    /// 删除 → 用历史版本保存：行不得复活，也不得被写回任何候选值。
    ///
    /// 与上一条的区别是这里存在过**两个**历史版本（`first` 被 `second` 取代），
    /// 删除用的是当前版本。两个历史版本都必须落空，否则"读到旧版本 → 并发删除 →
    /// 用旧版本保存"会把用户删除掉的配置原地恢复。
    #[tokio::test]
    async fn stale_update_after_delete_cannot_resurrect_profile() {
        let (repo, db) = repo();
        let candidate = profile("gw", "https://gateway.example.invalid/v1");
        let first = repo
            .cas_upsert(&candidate, None)
            .await
            .unwrap()
            .unwrap()
            .revision()
            .expect("写入必须返回持久化行")
            .to_owned();

        let renamed = AiProviderProfile::new(
            "gw",
            "改名后的网关",
            AiProviderKind::OpenAiCompatible,
            "https://gateway.example.invalid/v2",
            false,
            Some("gpt-4o-mini".into()),
            1_700_000_999_000,
        )
        .unwrap();
        let second = repo
            .cas_upsert(&renamed, Some(&first))
            .await
            .unwrap()
            .unwrap()
            .revision()
            .expect("更新必须返回持久化行")
            .to_owned();
        assert_ne!(first, second, "更新必须推进版本");

        // 旧版本删不掉（不匹配），当前版本才删得掉。
        assert_eq!(
            repo.cas_delete("gw", Some(&first)).await.unwrap(),
            AiProviderProfileDeleteOutcome::RevisionConflict
        );
        assert_eq!(
            repo.cas_delete("gw", Some(&second)).await.unwrap(),
            AiProviderProfileDeleteOutcome::Deleted
        );

        for stale in [first.as_str(), second.as_str(), "stale"] {
            assert!(
                repo.cas_upsert(&candidate, Some(stale))
                    .await
                    .unwrap()
                    .is_none(),
                "删除后携带过期版本必须冲突: {stale}"
            );
        }
        assert!(repo.get("gw").await.unwrap().is_none());
        assert!(repo.list().await.unwrap().is_empty());
        let remaining: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM ai_provider_profiles", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(remaining, 0, "冲突路径不得留下任何行");
    }

    #[tokio::test]
    async fn cas_delete_distinguishes_not_found_from_conflict() {
        let (repo, _db) = repo();
        assert_eq!(
            repo.cas_delete("gw", None).await.unwrap(),
            AiProviderProfileDeleteOutcome::NotFound
        );

        let candidate = profile("gw", "https://gateway.example.invalid/v1");
        let written = repo.cas_upsert(&candidate, None).await.unwrap().unwrap();
        let revision = written.revision().unwrap().to_owned();
        assert_eq!(
            repo.cas_delete("gw", Some("stale")).await.unwrap(),
            AiProviderProfileDeleteOutcome::RevisionConflict,
            "版本不匹配不得删除"
        );
        assert!(repo.get("gw").await.unwrap().is_some());

        assert_eq!(
            repo.cas_delete("gw", Some(&revision)).await.unwrap(),
            AiProviderProfileDeleteOutcome::Deleted
        );
        assert!(repo.get("gw").await.unwrap().is_none());
        assert_eq!(
            repo.cas_delete("gw", Some(&revision)).await.unwrap(),
            AiProviderProfileDeleteOutcome::NotFound
        );
    }

    #[tokio::test]
    async fn list_is_sorted_and_rejects_unknown_kind() {
        let (repo, db) = repo();
        for id in ["zeta", "alpha"] {
            repo.cas_upsert(&profile(id, "https://gateway.example.invalid/v1"), None)
                .await
                .unwrap();
        }
        let ids: Vec<String> = repo
            .list()
            .await
            .unwrap()
            .iter()
            .map(|p| p.profile_id().to_owned())
            .collect();
        assert_eq!(ids, ["alpha", "zeta"]);

        // 未知 kind 必须被表约束拒绝：脏行根本进不来，读取端不必"猜一个默认种类"。
        // （读取路径仍有 `AiProviderKind::parse` 兜底，防止未来放宽约束时静默降级。）
        let error = db
            .lock()
            .execute(
                "INSERT INTO ai_provider_profiles
                    (profile_id, kind, display_name, endpoint, enabled, selected_model_id,
                     revision, created_at, updated_at)
                 VALUES ('dirty', 'anthropic', '外部', 'https://gateway.example.invalid/v1',
                         1, NULL, 'rev', 1, 1)",
                [],
            )
            .unwrap_err();
        assert!(
            error.to_string().contains("CHECK"),
            "未知 kind 必须被 CHECK 约束拒绝: {error}"
        );
    }

    #[tokio::test]
    async fn table_constraints_reject_secret_shaped_and_degenerate_rows() {
        let (_repo, db) = repo();
        let conn = db.lock();
        // 端点必须是 http(s)：file:// 与本地路径都进不来。
        for endpoint in ["file:///etc/passwd", "C:\\gateway", "ftp://host/x"] {
            assert!(
                conn.execute(
                    "INSERT INTO ai_provider_profiles
                        (profile_id, kind, display_name, endpoint, enabled, selected_model_id,
                         revision, created_at, updated_at)
                     VALUES ('gw', 'openai_compatible', '网关', ?1, 1, NULL, 'rev', 1, 1)",
                    rusqlite::params![endpoint],
                )
                .is_err(),
                "非法端点必须被表约束拒绝: {endpoint}"
            );
        }
        // profile_id 必须从字母数字开始。
        assert!(
            conn.execute(
                "INSERT INTO ai_provider_profiles
                    (profile_id, kind, display_name, endpoint, enabled, selected_model_id,
                     revision, created_at, updated_at)
                 VALUES ('-bad', 'openai_compatible', '网关',
                         'https://gateway.example.invalid/v1', 1, NULL, 'rev', 1, 1)",
                [],
            )
            .is_err()
        );
        // 空 revision 不允许（CAS 的前提）。
        assert!(
            conn.execute(
                "INSERT INTO ai_provider_profiles
                    (profile_id, kind, display_name, endpoint, enabled, selected_model_id,
                     revision, created_at, updated_at)
                 VALUES ('gw', 'openai_compatible', '网关',
                         'https://gateway.example.invalid/v1', 1, NULL, '', 1, 1)",
                [],
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn migration_is_idempotent_and_preserves_existing_settings() {
        let db = Arc::new(Db::open_in_memory().expect("内存库必须可用"));
        // 迁移已在 `Db::open_in_memory` 内跑过一次；再跑一次不得失败、不得丢数据。
        let repo = SqliteAiProviderProfileRepository::new(db.clone());
        repo.cas_upsert(&profile("gw", "https://gateway.example.invalid/v1"), None)
            .await
            .unwrap();
        {
            let mut conn = db.lock();
            crate::db::migrations::run(&mut conn).expect("重复迁移必须幂等");
        }
        assert_eq!(repo.list().await.unwrap().len(), 1, "重复迁移不得丢数据");
    }
}
