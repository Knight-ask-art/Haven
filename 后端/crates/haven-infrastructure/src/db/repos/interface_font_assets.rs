//! 界面字体资产 SQLite Repository + UoW（迁移 045，BE-INTERFACE-FONT-001）。
//!
//! 职责边界：
//! - `SqliteInterfaceFontAssetRepository` 只做资产存取原语（list / get / insert /
//!   delete / load_bytes / count）。**不实现**「不得删除正在使用的字体」——
//!   那是跨表不变量，由 Application 层在**同一个事务**内判定。
//! - `SqliteInterfaceFontUoW` 提供该事务：导入的「查重 → 配额 → 写入」与删除的
//!   「读当前选中 → 删除」都在单一 `BEGIN IMMEDIATE` 事务内，复用既有 Db 锁。
//!
//! 字节与元数据同表同行：不存在「元数据删了、字节还在」或反过来的中间状态。
//! 所有写入都走参数绑定；`file_name` 只作为展示文本落库，任何路径拼接都不使用它。

use std::sync::Arc;

use async_trait::async_trait;
use haven_application::services::interface_fonts::{InterfaceFontTxPorts, InterfaceFontUoW};
use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::contracts::{
    InterfaceFontAsset, InterfaceFontAssetBytes, InterfaceFontAssetInsert,
    InterfaceFontAssetRepository,
};
use rusqlite::{OptionalExtension, Transaction};

use crate::db::Db;
use crate::db::repos::map_db_error;

const ASSET_COLUMNS: &str = "id, family_name, file_name, extension, mime_type, byte_size, \
     sha256, created_at";

pub struct SqliteInterfaceFontAssetRepository {
    db: Arc<Db>,
}

impl SqliteInterfaceFontAssetRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl InterfaceFontAssetRepository for SqliteInterfaceFontAssetRepository {
    async fn list(&self) -> Result<Vec<InterfaceFontAsset>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {ASSET_COLUMNS} FROM interface_font_assets \
                 ORDER BY created_at DESC, id ASC"
            ))
            .map_err(map_db_error("查询导入字体失败"))?;
        let rows = stmt
            .query_map([], read_asset_row)
            .map_err(map_db_error("查询导入字体失败"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(map_db_error("查询导入字体失败"))
    }

    async fn get(&self, id: &str) -> Result<Option<InterfaceFontAsset>, AppError> {
        let conn = self.db.lock();
        conn.query_row(
            &format!("SELECT {ASSET_COLUMNS} FROM interface_font_assets WHERE id = ?1"),
            rusqlite::params![id],
            read_asset_row,
        )
        .optional()
        .map_err(map_db_error("查询导入字体失败"))
    }

    async fn find_by_digest(&self, sha256: &str) -> Result<Option<InterfaceFontAsset>, AppError> {
        let conn = self.db.lock();
        conn.query_row(
            &format!(
                "SELECT {ASSET_COLUMNS} FROM interface_font_assets WHERE sha256 = ?1 \
                 ORDER BY created_at ASC, id ASC LIMIT 1"
            ),
            rusqlite::params![sha256],
            read_asset_row,
        )
        .optional()
        .map_err(map_db_error("查询导入字体失败"))
    }

    async fn insert(&self, asset: &InterfaceFontAssetInsert) -> Result<(), AppError> {
        let conn = self.db.lock();
        insert_on_conn(&conn, asset)
    }

    async fn delete(&self, id: &str) -> Result<bool, AppError> {
        let conn = self.db.lock();
        let affected = conn
            .execute(
                "DELETE FROM interface_font_assets WHERE id = ?1",
                rusqlite::params![id],
            )
            .map_err(map_db_error("删除导入字体失败"))?;
        Ok(affected > 0)
    }

    async fn load_bytes(&self, id: &str) -> Result<Option<InterfaceFontAssetBytes>, AppError> {
        let conn = self.db.lock();
        load_bytes_on_conn(&conn, id)
    }

    async fn count(&self) -> Result<u32, AppError> {
        let conn = self.db.lock();
        count_on_conn(&conn)
    }
}

/// 事务版 UoW：导入（查重 → 配额 → 写入）与删除（读当前选中 → 删除）各自原子。
pub struct SqliteInterfaceFontUoW {
    db: Arc<Db>,
}

impl SqliteInterfaceFontUoW {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

impl InterfaceFontUoW for SqliteInterfaceFontUoW {
    fn run(
        &self,
        f: &dyn Fn(&dyn InterfaceFontTxPorts) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        // BEGIN IMMEDIATE：事务开始即取 RESERVED 写锁，读到的「当前选中字体」
        // 与随后的删除/写入之间不存在其他写者插入的窗口。
        let mut guard = self.db.lock();
        let tx = guard
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| tx_err("开启字体事务失败", e))?;
        let scope = SqliteInterfaceFontTx { tx: &tx };
        match f(&scope) {
            Ok(()) => tx.commit().map_err(|e| tx_err("提交字体事务失败", e)),
            Err(e) => Err(e), // tx Drop 时自动回滚
        }
    }
}

struct SqliteInterfaceFontTx<'a> {
    tx: &'a Transaction<'a>,
}

impl InterfaceFontTxPorts for SqliteInterfaceFontTx<'_> {
    /// 只回答「当前选中的是哪个资产」。
    ///
    /// 这里刻意用宽松的 `serde_json::Value` 取值而不是 `SettingsValue` 反序列化：
    /// `AppearanceSettings` 带 `deny_unknown_fields`，用一个未来版本的设置行做反序列化
    /// 会整体失败，从而让「无法判定 → 什么都不能删」这种最坏结果发生。
    /// 本函数只关心一个字段，就只取一个字段；行本身读不出来才是错误。
    fn active_asset_id(&self) -> Result<Option<String>, AppError> {
        let row: Option<String> = self
            .tx
            .query_row(
                "SELECT data_json FROM settings WHERE section = 'appearance'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| tx_err("查询界面字体设置失败", e))?;
        let Some(raw) = row else {
            return Ok(None);
        };
        let value: serde_json::Value = serde_json::from_str(&raw).map_err(|error| {
            AppError::new(
                "DATABASE_ERROR",
                ErrorKind::Database,
                "界面设置数据损坏",
                true,
            )
            .with_source(error)
        })?;
        Ok(value
            .get("interfaceFontAssetId")
            .and_then(|id| id.as_str())
            .map(str::to_owned))
    }

    fn asset_exists(&self, id: &str) -> Result<bool, AppError> {
        self.tx
            .query_row(
                "SELECT 1 FROM interface_font_assets WHERE id = ?1",
                rusqlite::params![id],
                |_| Ok(()),
            )
            .optional()
            .map(|found| found.is_some())
            .map_err(|e| tx_err("查询导入字体失败", e))
    }

    fn find_by_digest(&self, sha256: &str) -> Result<Option<InterfaceFontAsset>, AppError> {
        self.tx
            .query_row(
                &format!(
                    "SELECT {ASSET_COLUMNS} FROM interface_font_assets WHERE sha256 = ?1 \
                     ORDER BY created_at ASC, id ASC LIMIT 1"
                ),
                rusqlite::params![sha256],
                read_asset_row,
            )
            .optional()
            .map_err(|e| tx_err("查询导入字体失败", e))
    }

    fn count_assets(&self) -> Result<u32, AppError> {
        count_on_conn(self.tx)
    }

    fn insert_asset(&self, asset: &InterfaceFontAssetInsert) -> Result<(), AppError> {
        insert_on_conn(self.tx, asset)
    }

    fn delete_asset(&self, id: &str) -> Result<bool, AppError> {
        let affected = self
            .tx
            .execute(
                "DELETE FROM interface_font_assets WHERE id = ?1",
                rusqlite::params![id],
            )
            .map_err(|e| tx_err("删除导入字体失败", e))?;
        Ok(affected > 0)
    }
}

fn read_asset_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<InterfaceFontAsset> {
    Ok(InterfaceFontAsset {
        id: row.get("id")?,
        family_name: row.get("family_name")?,
        file_name: row.get("file_name")?,
        extension: row.get("extension")?,
        mime_type: row.get("mime_type")?,
        byte_size: row.get::<_, i64>("byte_size")?.max(0) as u64,
        sha256: row.get("sha256")?,
        created_at: UtcMillis(row.get("created_at")?),
    })
}

fn insert_on_conn(
    conn: &rusqlite::Connection,
    asset: &InterfaceFontAssetInsert,
) -> Result<(), AppError> {
    let byte_size = asset.bytes.len() as i64;
    conn.execute(
        "INSERT INTO interface_font_assets
            (id, family_name, file_name, extension, mime_type, byte_size, sha256, bytes, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            asset.id,
            asset.family_name,
            asset.file_name,
            asset.extension,
            asset.mime_type,
            byte_size,
            asset.sha256,
            asset.bytes,
            asset.created_at.0,
        ],
    )
    .map_err(map_db_error("保存导入字体失败"))?;
    Ok(())
}

fn load_bytes_on_conn(
    conn: &rusqlite::Connection,
    id: &str,
) -> Result<Option<InterfaceFontAssetBytes>, AppError> {
    conn.query_row(
        "SELECT id, mime_type, bytes FROM interface_font_assets WHERE id = ?1",
        rusqlite::params![id],
        |row| {
            Ok(InterfaceFontAssetBytes {
                id: row.get("id")?,
                mime_type: row.get("mime_type")?,
                bytes: row.get("bytes")?,
            })
        },
    )
    .optional()
    .map_err(map_db_error("读取导入字体失败"))
}

fn count_on_conn(conn: &rusqlite::Connection) -> Result<u32, AppError> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM interface_font_assets", [], |row| {
            row.get(0)
        })
        .map_err(map_db_error("统计导入字体失败"))?;
    Ok(count.clamp(0, u32::MAX as i64) as u32)
}

fn tx_err(msg: &'static str, e: rusqlite::Error) -> AppError {
    AppError::new("DATABASE_ERROR", ErrorKind::Database, msg, true).with_source(e)
}
