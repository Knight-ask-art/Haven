//! SQLite 引导与迁移。
//!
//! 规范：`plan/TECHNICAL_ARCHITECTURE.md` §25。
//! - rusqlite bundled SQLite
//! - WAL + foreign_keys=ON + busy_timeout
//! - 第一天建立迁移系统，禁止发布后用 `CREATE TABLE IF NOT EXISTS` 偷渡版本迁移
//! - 迁移在事务内执行，失败回滚
//! - 本模块不依赖 Tauri，可独立测试

pub mod migrations;
pub mod repos;
pub mod uow;

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use haven_common::AppError;

/// 数据库句柄。内部持锁：SQLite 连接本身不是线程安全的，
/// 跨线程访问通过 Mutex 串行化；重查询应在外层 `spawn_blocking`。
pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    /// 打开（不存在则创建）数据库并执行全部未应用迁移。
    pub fn open(path: &Path) -> Result<Self, AppError> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| {
            AppError::new(
                "DATABASE_OPEN_FAILED",
                haven_common::ErrorKind::Database,
                format!("打开数据库失败: {path:?}"),
                false,
            )
            .with_source(e)
        })?;

        configure(&conn, true)?;

        let mut conn = conn;
        migrations::run(&mut conn)?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// 临时内存数据库（测试用）。
    pub fn open_in_memory() -> Result<Self, AppError> {
        let conn = Connection::open_in_memory().map_err(|e| {
            AppError::new(
                "DATABASE_OPEN_FAILED",
                haven_common::ErrorKind::Database,
                "打开内存数据库失败",
                false,
            )
            .with_source(e)
        })?;
        configure(&conn, false)?;
        let mut conn = conn;
        migrations::run(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        // 中毒不放弃连接：panic 发生时持有的事务已随 Drop 回滚，
        // 连接本身仍可用；此处沿用 scan.rs 的抗毒化模式，避免一次
        // panic 让后续所有数据操作级联 panic。
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 在单一事务中执行闭包（scanner 单文件原子写入 / 事务编排）。
    /// - 闭包返回 Ok → commit；Err → 自动 rollback（tx Drop）。
    /// - 闭包接收 `&Transaction`（Deref 到 Connection，可传给 repo 的 save_on_conn）。
    pub fn with_tx<T>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let guard = self.lock();
        let tx = guard.unchecked_transaction().map_err(|e| {
            AppError::new(
                "DATABASE_ERROR",
                haven_common::ErrorKind::Database,
                "开启事务失败",
                true,
            )
            .with_source(e)
        })?;
        let result = f(&tx);
        match result {
            Ok(value) => {
                tx.commit().map_err(|e| {
                    AppError::new(
                        "DATABASE_ERROR",
                        haven_common::ErrorKind::Database,
                        "提交事务失败",
                        true,
                    )
                    .with_source(e)
                })?;
                Ok(value)
            }
            Err(e) => Err(e), // tx Drop 时自动回滚
        }
    }

    /// 数据库文件路径（供诊断/测试）。
    pub fn path(&self) -> Option<std::path::PathBuf> {
        self.lock().path().map(Into::into)
    }
}

fn configure(conn: &Connection, use_wal: bool) -> Result<(), AppError> {
    if use_wal {
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(db_err("设置 WAL 失败"))?;
    }
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(db_err("启用外键失败"))?;
    conn.pragma_update(
        None,
        "busy_timeout",
        Duration::from_secs(5).as_millis() as i64,
    )
    .map_err(db_err("设置 busy_timeout 失败"))?;
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(db_err("设置 synchronous 失败"))?;
    Ok(())
}

fn db_err(msg: &'static str) -> impl Fn(rusqlite::Error) -> AppError {
    move |e| {
        AppError::new(
            "DATABASE_ERROR",
            haven_common::ErrorKind::Database,
            msg,
            true,
        )
        .with_source(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_db_migrates() {
        let db = Db::open_in_memory().expect("open in-memory db");
        let version: i64 = db
            .lock()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .expect("read user_version");
        let applied: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .expect("count migrations");
        assert!(applied >= 1, "至少应用一个迁移，实际 {applied}");
        let _ = version;
    }

    #[test]
    fn file_db_opens_and_reopens_idempotently() {
        let dir = std::env::temp_dir().join(format!(
            "haven-db-test-{}",
            haven_common::UtcMillis::now().0
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("test.db");

        {
            let _db = Db::open(&db_path).expect("first open");
        }
        {
            let db = Db::open(&db_path).expect("second open (idempotent)");
            let tables: Vec<String> = db
                .lock()
                .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .map(|x| x.unwrap())
                .collect();
            assert!(tables.iter().any(|t| t == "works"));
            assert!(tables.iter().any(|t| t == "progress"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let db = Db::open_in_memory().unwrap();
        let foreign_keys: i64 = db
            .lock()
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(foreign_keys, 1, "每个连接都必须显式启用外键");
        let result = db.lock().execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES ('00000000-0000-0000-0000-000000000001', 'x', 'fiction', 'completed', 1, 1)",
            [],
        );
        assert!(result.is_ok());
        let orphan = db.lock().execute(
            "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
             VALUES ('00000000-0000-0000-0000-000000000002', '00000000-0000-0000-0000-000000009999', 'orphan', 'book', 1, 1)",
            [],
        );
        assert!(orphan.is_err(), "外键必须生效，孤儿 edition 应被拒绝");
    }
}
