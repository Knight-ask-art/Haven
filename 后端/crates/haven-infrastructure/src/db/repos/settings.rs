//! Settings Repository + SettingsUoW（Sqlite，BE-SETTINGS-001 + R-MAIN-07 复审修复）。
//!
//! 并发控制由 `SettingsUoW` 承担，全链路原子：
//! - 写路径 `BEGIN IMMEDIATE`：事务开始即取 RESERVED 写锁；真实双连接竞争在
//!   busy_timeout 内排队，后到事务读到最新已提交状态 → 稳定 REVISION_CONFLICT
//!   （DEFERRED 的 BUSY_SNAPSHOT 不会泄漏成 DATABASE_ERROR）。
//! - 读路径 `run_read`：`BEGIN DEFERRED` 只读（WAL 下不阻塞写者、不取写锁）。
//! - `cas_write`：**数据库层条件写**——`expected_revision` 作为 SQL 条件
//!   （`WHERE settings.revision = expected` / 首次无冲突 INSERT）；affected == 0 → 冲突。
//!
//! Repository 层只提供基础 CRUD 原语。

use std::sync::Arc;

use async_trait::async_trait;

use haven_application::services::settings::{SettingsTxPorts, SettingsUoW};
use haven_common::AppError;
use haven_domain::contracts::{SettingsRepository, SettingsRow};

use crate::db::Db;
use crate::db::repos::map_db_error;

pub struct SqliteSettingsRepository {
    db: Arc<Db>,
}

impl SqliteSettingsRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl SettingsRepository for SqliteSettingsRepository {
    async fn get(&self, section: &str) -> Result<Option<SettingsRow>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(
                "SELECT section, schema_version, revision, data_json, updated_at
                 FROM settings WHERE section = ?1",
            )
            .map_err(map_db_error("查询设置失败"))?;
        let mut rows = stmt
            .query_map(rusqlite::params![section], |row| {
                Ok(SettingsRow {
                    section: row.get("section")?,
                    schema_version: row.get("schema_version")?,
                    revision: row.get("revision")?,
                    data_json: row.get("data_json")?,
                    updated_at: haven_common::UtcMillis(row.get("updated_at")?),
                })
            })
            .map_err(map_db_error("查询设置失败"))?;
        rows.next()
            .transpose()
            .map_err(map_db_error("查询设置失败"))
    }

    async fn upsert(&self, row: &SettingsRow) -> Result<(), AppError> {
        let conn = self.db.lock();
        conn.execute(
            "INSERT INTO settings (section, schema_version, revision, data_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(section) DO UPDATE SET
                 schema_version = excluded.schema_version,
                 revision = excluded.revision,
                 data_json = excluded.data_json,
                 updated_at = excluded.updated_at",
            rusqlite::params![
                row.section,
                row.schema_version,
                row.revision,
                row.data_json,
                row.updated_at.0
            ],
        )
        .map_err(map_db_error("保存设置失败"))?;
        Ok(())
    }
}

/// 事务内 settings 操作（Sqlite）。
pub struct SqliteSettingsUoW {
    db: Arc<Db>,
}

impl SqliteSettingsUoW {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

impl SettingsUoW for SqliteSettingsUoW {
    fn run(
        &self,
        f: &dyn Fn(&dyn SettingsTxPorts) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        // R-MAIN-07：写路径 BEGIN IMMEDIATE——事务开始即取 RESERVED 写锁。
        // 真实双连接并发下，后到的事务在 busy_timeout 内等待先到者提交，
        // 然后读取**最新已提交状态**做 expected 校验 → 稳定 REVISION_CONFLICT，
        // 不会出现 DEFERRED 的 BUSY_SNAPSHOT（旧快照升级写锁失败）泄漏成 DATABASE_ERROR。
        let mut guard = self.db.lock();
        let tx = guard
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| tx_err("开启设置事务失败", e))?;
        let scope = SqliteSettingsTx { tx: &tx };
        match f(&scope) {
            Ok(()) => tx.commit().map_err(|e| tx_err("提交设置事务失败", e)),
            Err(e) => Err(e), // tx Drop 时自动回滚
        }
    }

    /// 读路径：BEGIN DEFERRED 只读（WAL 下不阻塞写者、不取写锁，不会在 busy 时泄漏错误）。
    fn run_read(
        &self,
        f: &dyn Fn(&dyn SettingsTxPorts) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        let mut guard = self.db.lock();
        let tx = guard
            .transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)
            .map_err(|e| tx_err("开启设置读取事务失败", e))?;
        let scope = SqliteSettingsTx { tx: &tx };
        match f(&scope) {
            Ok(()) => tx.commit().map_err(|e| tx_err("提交设置读取事务失败", e)),
            Err(e) => Err(e),
        }
    }
}

struct SqliteSettingsTx<'a> {
    tx: &'a rusqlite::Transaction<'a>,
}

impl SettingsTxPorts for SqliteSettingsTx<'_> {
    fn load(&self, section: &str) -> Result<Option<SettingsRow>, AppError> {
        use rusqlite::OptionalExtension;
        self.tx
            .query_row(
                "SELECT section, schema_version, revision, data_json, updated_at
                 FROM settings WHERE section = ?1",
                rusqlite::params![section],
                |row| {
                    Ok(SettingsRow {
                        section: row.get("section")?,
                        schema_version: row.get("schema_version")?,
                        revision: row.get("revision")?,
                        data_json: row.get("data_json")?,
                        updated_at: haven_common::UtcMillis(row.get("updated_at")?),
                    })
                },
            )
            .optional()
            .map_err(|e| tx_err("查询设置失败", e))
    }

    /// 数据库层条件写（R-MAIN-07）：`expected_revision` 作为 SQL 条件。
    ///
    /// - 已有行 + expected：`INSERT ... ON CONFLICT(section) DO UPDATE ... WHERE settings.revision = expected`；
    ///   若 revision 已被并发方推进 → WHERE 不匹配 → affected == 0 → 返回 false（冲突）。
    /// - 首次 + expected=None：无冲突 INSERT 成功 → affected == 1 → true；
    ///   并发下已有行时 DO UPDATE 的 `WHERE settings.revision = NULL` 恒假 → false（双重兜底）。
    ///
    /// 返回 `true` = 已写入；`false` = 未写入（并发竞争/条件不满足，
    /// 调用方映射 REVISION_CONFLICT）。
    fn cas_write(
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
}

fn tx_err(msg: &'static str, e: rusqlite::Error) -> AppError {
    AppError::new(
        "DATABASE_ERROR",
        haven_common::ErrorKind::Database,
        msg,
        true,
    )
    .with_source(e)
}
