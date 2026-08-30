//! 迁移系统：按顺序执行 `后端/migrations/NNN_name.sql`。
//!
//! 规则：
//! - 迁移文件只允许追加，不允许修改已发布文件。
//! - 每个迁移在事务内执行，失败回滚。
//! - `schema_migrations` 表记录已应用版本。
//! - 迁移 SQL 通过 `include_str!` 编译期嵌入（路径相对本文件）。

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use haven_common::AppError;

/// 迁移列表。新迁移在此追加（与文件一一对应）。
const MIGRATIONS: &[(&str, &str)] = &[
    (
        "001_initial",
        include_str!("../../../../migrations/001_initial.sql"),
    ),
    (
        "002_history_consistency",
        include_str!("../../../../migrations/002_history_consistency.sql"),
    ),
    (
        "003_fingerprint_and_history_unique",
        include_str!("../../../../migrations/003_fingerprint_and_history_unique.sql"),
    ),
    (
        "004_locator_index",
        include_str!("../../../../migrations/004_locator_index.sql"),
    ),
    (
        "005_favorites_revision",
        include_str!("../../../../migrations/005_favorites_revision.sql"),
    ),
    (
        "006_favorite_versions_fk",
        include_str!("../../../../migrations/006_favorite_versions_fk.sql"),
    ),
    (
        "007_settings",
        include_str!("../../../../migrations/007_settings.sql"),
    ),
    (
        "008_storage_root_unique",
        include_str!("../../../../migrations/008_storage_root_unique.sql"),
    ),
    (
        "009_availability_source",
        include_str!("../../../../migrations/009_availability_source.sql"),
    ),
    (
        "010_query_indexes",
        include_str!("../../../../migrations/010_query_indexes.sql"),
    ),
    (
        "011_download_tasks",
        include_str!("../../../../migrations/011_download_tasks.sql"),
    ),
    (
        "012_download_transfer_metrics",
        include_str!("../../../../migrations/012_download_transfer_metrics.sql"),
    ),
    (
        "013_download_offline_resource",
        include_str!("../../../../migrations/013_download_offline_resource.sql"),
    ),
    (
        "014_work_source_refs",
        include_str!("../../../../migrations/014_work_source_refs.sql"),
    ),
    (
        "015_image_proxy",
        include_str!("../../../../migrations/015_image_proxy.sql"),
    ),
    (
        "016_work_ratings",
        include_str!("../../../../migrations/016_work_ratings.sql"),
    ),
    (
        "017_enrichment_state",
        include_str!("../../../../migrations/017_enrichment_state.sql"),
    ),
    (
        "018_progress_keyframe",
        include_str!("../../../../migrations/018_progress_keyframe.sql"),
    ),
    (
        "019_work_director_actor",
        include_str!("../../../../migrations/019_work_director_actor.sql"),
    ),
    (
        "020_trending_board_cache",
        include_str!("../../../../migrations/020_trending_board_cache.sql"),
    ),
    (
        "021_artwork_cache",
        include_str!("../../../../migrations/021_artwork_cache.sql"),
    ),
    (
        "022_artwork_legacy_sources",
        include_str!("../../../../migrations/022_artwork_legacy_sources.sql"),
    ),
    (
        "023_search_history",
        include_str!("../../../../migrations/023_search_history.sql"),
    ),
    (
        "024_resource_preferences",
        include_str!("../../../../migrations/024_resource_preferences.sql"),
    ),
    (
        "025_work_fts",
        include_str!("../../../../migrations/025_work_fts.sql"),
    ),
    (
        "026_download_batches",
        include_str!("../../../../migrations/026_download_batches.sql"),
    ),
    (
        "027_work_fts_triggers",
        include_str!("../../../../migrations/027_work_fts_triggers.sql"),
    ),
    (
        "028_work_relations",
        include_str!("../../../../migrations/028_work_relations.sql"),
    ),
];

pub fn run(conn: &mut Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version    TEXT PRIMARY KEY,
            applied_at INTEGER NOT NULL
        );",
    )
    .map_err(db_err("创建 schema_migrations 失败"))?;

    let columns: Vec<String> = conn
        .prepare("PRAGMA table_info(schema_migrations)")
        .map_err(db_err("检查迁移表结构失败"))?
        .query_map([], |row| row.get(1))
        .map_err(db_err("读取迁移表结构失败"))?
        .collect::<Result<_, _>>()
        .map_err(db_err("读取迁移列失败"))?;
    if !columns.iter().any(|column| column == "checksum") {
        conn.execute("ALTER TABLE schema_migrations ADD COLUMN checksum TEXT", [])
            .map_err(db_err("升级迁移表失败"))?;
    }

    let known_versions = MIGRATIONS
        .iter()
        .map(|(version, _)| format!("'{}'", version.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");
    let unknown_sql = format!(
        "SELECT version FROM schema_migrations WHERE version NOT IN ({known_versions}) LIMIT 1"
    );
    let unknown: Option<String> = conn
        .query_row(&unknown_sql, [], |row| row.get(0))
        .optional()
        .map_err(db_err("检查未知迁移失败"))?;
    if let Some(version) = unknown {
        return Err(AppError::new(
            "UNKNOWN_MIGRATION_VERSION",
            haven_common::ErrorKind::Database,
            format!("数据库包含当前版本不认识的迁移 {version}"),
            false,
        ));
    }

    for (version, sql) in MIGRATIONS {
        let checksum = checksum(sql);
        let applied_checksum: Option<Option<String>> = conn
            .query_row(
                "SELECT checksum FROM schema_migrations WHERE version = ?1",
                params![version],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_err("查询迁移状态失败"))?;

        if let Some(applied_checksum) = applied_checksum {
            let applied_checksum = applied_checksum.ok_or_else(|| {
                AppError::new(
                    "MIGRATION_CHECKSUM_UNAVAILABLE",
                    haven_common::ErrorKind::Database,
                    format!("迁移 {version} 缺少校验值，请执行受控升级"),
                    false,
                )
            })?;
            if applied_checksum == checksum {
                continue;
            }
            return Err(AppError::new(
                "MIGRATION_CHECKSUM_MISMATCH",
                haven_common::ErrorKind::Database,
                format!("已应用迁移 {version} 的内容发生变化"),
                false,
            ));
        }

        let tx = conn.transaction().map_err(db_err("开启迁移事务失败"))?;
        tx.execute_batch(sql).map_err(|e| {
            AppError::new(
                "MIGRATION_FAILED",
                haven_common::ErrorKind::Database,
                format!("迁移 {version} 执行失败"),
                false,
            )
            .with_source(e)
        })?;
        tx.execute(
            "INSERT INTO schema_migrations (version, checksum, applied_at) VALUES (?1, ?2, ?3)",
            params![version, checksum, haven_common::UtcMillis::now().0],
        )
        .map_err(db_err("记录迁移版本失败"))?;
        tx.commit().map_err(db_err("提交迁移失败"))?;
    }

    Ok(())
}

fn checksum(sql: &str) -> String {
    let normalized = sql.replace("\r\n", "\n");
    format!("{:x}", Sha256::digest(normalized.as_bytes()))
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
    use rusqlite::Connection;

    fn fresh() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version TEXT PRIMARY KEY,
                applied_at INTEGER NOT NULL
            );",
        )
        .unwrap();
        conn
    }

    /// 构造**真实已应用 001..005 的 legacy DB**：执行 001..005 SQL，并在 schema_migrations
    /// 中按生产 checksum 逻辑登记每个 version/checksum/applied_at（带 checksum 列）。
    /// 供「runner 驱动的 005→006 升级」成功/失败测试使用。
    fn apply_legacy_through_005(conn: &mut Connection) {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version TEXT PRIMARY KEY,
                applied_at INTEGER NOT NULL,
                checksum TEXT
            );",
        )
        .unwrap();
        for (version, sql) in &MIGRATIONS[..5] {
            conn.execute_batch(sql).unwrap();
            conn.execute(
                "INSERT INTO schema_migrations (version, checksum, applied_at) VALUES (?1, ?2, ?3)",
                params![*version, checksum(sql), haven_common::UtcMillis::now().0],
            )
            .unwrap();
        }
    }

    /// 通用 legacy 构造：执行前 `count` 个迁移 SQL 并按生产 checksum 登记
    /// （带 checksum 列的 schema_migrations）；并开启 foreign_keys。
    /// 供 008/009 真实 runner 升级测试使用。
    fn apply_legacy_through(conn: &mut Connection, count: usize) {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version TEXT PRIMARY KEY,
                applied_at INTEGER NOT NULL,
                checksum TEXT
            );",
        )
        .unwrap();
        for (version, sql) in &MIGRATIONS[..count] {
            conn.execute_batch(sql).unwrap();
            conn.execute(
                "INSERT INTO schema_migrations (version, checksum, applied_at) VALUES (?1, ?2, ?3)",
                params![*version, checksum(sql), haven_common::UtcMillis::now().0],
            )
            .unwrap();
        }
        conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
    }

    /// 插入一条 storage_locations 行（参数化，避免字面量假阳性）。
    fn insert_storage_location(conn: &Connection, id: &str, display: &str, root_ref: &str) {
        let now = haven_common::UtcMillis::now().0;
        conn.execute(
            "INSERT INTO storage_locations
                (id, provider_type, display_name, root_ref, credential_ref, status, created_at, updated_at)
             VALUES (?1, 'local', ?2, ?3, NULL, 'connected', ?4, ?4)",
            params![id, display, root_ref, now],
        )
        .unwrap();
    }

    /// 插入 009 前的一整条 resources 历史链（works→edition→media_item→resource）。
    fn insert_pre_009_resource(conn: &Connection) {
        let now = haven_common::UtcMillis::now().0;
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-000000000d01', 'x', 'fiction', 'completed', ?1, ?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-000000000d02', '0196f0d2-0000-7000-8000-000000000d01', 'e', 'movie', ?1, ?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media_items (id, edition_id, media_type, title, status, created_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-000000000d03', '0196f0d2-0000-7000-8000-000000000d02', 'movie', 't', 'available', ?1, ?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO resources
                (id, media_item_id, resource_type, locator_kind, locator_json, availability, created_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-000000000d04', '0196f0d2-0000-7000-8000-000000000d03', 'local_file', 'local_path',
                     '{\"kind\":\"local_path\",\"path\":\"D:/Movies/x.mkv\"}', 'available', ?1, ?1)",
            params![now],
        )
        .unwrap();
    }

    /// 构造 005 状态下的「有效 + 孤儿」版本行（005 无 FK 允许孤儿）。
    fn seed_005_version_rows(conn: &Connection) -> String {
        let work_id = "0196f0d2-0000-7000-8000-000000000aaa";
        let now = haven_common::UtcMillis::now().0;
        conn.execute_batch(&format!(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES ('{work_id}', '孤儿测试', 'fiction', 'completed', {now}, {now});
             INSERT INTO work_favorite_versions (work_id, revision, updated_at)
             VALUES ('{work_id}', 'rev-valid', {now}),
                    ('0196f0d2-0000-7000-8000-00000000ffff', 'rev-orphan', {now});"
        ))
        .unwrap();
        work_id.to_string()
    }

    fn recorded_checksum(conn: &Connection, version: &str) -> Option<String> {
        conn.query_row(
            "SELECT checksum FROM schema_migrations WHERE version = ?1",
            params![version],
            |row| row.get(0),
        )
        .ok()
    }

    /// 阻塞 B（runner 成功路径）：真实 migration runner 从 legacy 005 升级——
    /// run(&mut conn) 应用 006（及后续动态 MIGRATIONS 数量），孤儿行被过滤、有效行保留、
    /// FK 指向 works 且 ON DELETE CASCADE、删 Work 后版本行级联删除；
    /// 005 record/checksum 不变；006 恰好 1 条且 checksum 正确。
    #[test]
    fn legacy_005_to_006_upgrade_via_runner_succeeds() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_legacy_through_005(&mut conn);
        conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
        let work_id = seed_005_version_rows(&conn);

        let five_checksum = recorded_checksum(&conn, "005_favorites_revision").unwrap();
        let five_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = '005_favorites_revision'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        // 真实 runner（非直接 execute_batch 006）。
        run(&mut conn).unwrap();

        // 005 record/checksum 不变。
        assert_eq!(
            recorded_checksum(&conn, "005_favorites_revision").as_deref(),
            Some(five_checksum.as_str()),
            "005 checksum 不得变化"
        );
        let five_after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = '005_favorites_revision'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(five_after, five_count, "005 record 不得重复");

        // 006 恰好 1 条且 checksum 正确。
        let mut six_stmt = conn
            .prepare(
                "SELECT checksum FROM schema_migrations WHERE version = '006_favorite_versions_fk'",
            )
            .unwrap();
        let six_checksums: Vec<String> = six_stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(six_checksums.len(), 1, "006 必须恰好记录 1 条");
        assert_eq!(
            six_checksums[0],
            checksum(include_str!(
                "../../../../migrations/006_favorite_versions_fk.sql"
            )),
            "006 checksum 必须正确"
        );

        // 孤儿行被过滤、有效行保留。
        let valid: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM work_favorite_versions WHERE work_id = ?1",
                params![work_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(valid, 1, "有效版本行必须保留");
        let orphans: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM work_favorite_versions WHERE work_id = '0196f0d2-0000-7000-8000-00000000ffff'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0, "孤儿版本行必须随 006 过滤");

        // FK 指向 works 且 ON DELETE CASCADE。
        // PRAGMA foreign_key_list 列：id, seq, table, from, to, on_update, on_delete, match。
        type FkRow = (i64, i64, String, String, String, String, String, String);
        let mut fk_stmt = conn
            .prepare("PRAGMA foreign_key_list(work_favorite_versions)")
            .unwrap();
        let fks: Vec<FkRow> = fk_stmt
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(
            fks.iter()
                .any(|fk| fk.2 == "works" && fk.4 == "id" && fk.6 == "CASCADE"),
            "work_favorite_versions 必须 FK→works(id) ON DELETE CASCADE，实际 {fks:?}"
        );

        // 删 Work → 版本行级联删除。
        conn.execute("DELETE FROM works WHERE id = ?1", params![work_id])
            .unwrap();
        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM work_favorite_versions WHERE work_id = ?1",
                params![work_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0, "删除 Work 后版本行级联删除");

        // 动态断言当前 MIGRATIONS.len()（未来 Favorites-only clean snapshot 只含 6 项也成立）。
        let applied: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            applied as usize,
            MIGRATIONS.len(),
            "runner 应用全部剩余迁移"
        );
    }

    /// 阻塞 B（runner 失败路径）：legacy 005 库预存在与 006 冲突的
    /// `work_favorite_versions_new` → 真实 runner 的 006 事务失败 →
    /// `MIGRATION_FAILED`；schema_migrations 无 006；**旧 work_favorite_versions 仍在**
    /// （valid + orphan 均未删）；005 record/checksum 不变。失败发生在迁移执行（非准备 SQL）。
    #[test]
    fn legacy_005_to_006_via_runner_conflict_rolls_back() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_legacy_through_005(&mut conn);
        conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
        let work_id = seed_005_version_rows(&conn);

        let five_checksum = recorded_checksum(&conn, "005_favorites_revision").unwrap();

        // 预创建与 006 重建目标冲突的表（模拟半途/并发状态）。
        conn.execute_batch(
            "CREATE TABLE work_favorite_versions_new (
                work_id    TEXT PRIMARY KEY,
                revision   TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            INSERT INTO work_favorite_versions_new (work_id, revision, updated_at)
            VALUES ('pre-existing', 'x', 1);",
        )
        .unwrap();

        let err = run(&mut conn).unwrap_err();
        assert_eq!(
            err.code().as_str(),
            "MIGRATION_FAILED",
            "006 与既有表冲突必须报告 MIGRATION_FAILED"
        );

        // schema_migrations 无 006。
        let six: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = '006_favorite_versions_fk'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(six, 0, "失败的迁移不得记录为已应用");

        // 旧 work_favorite_versions 仍在（valid + orphan 均未删）。
        let valid: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM work_favorite_versions WHERE work_id = ?1",
                params![work_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(valid, 1, "有效版本行必须保留（回滚）");
        let orphan: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM work_favorite_versions WHERE work_id = '0196f0d2-0000-7000-8000-00000000ffff'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphan, 1, "孤儿版本行必须保留（回滚）");

        // 005 record/checksum 不变。
        assert_eq!(
            recorded_checksum(&conn, "005_favorites_revision").as_deref(),
            Some(five_checksum.as_str()),
            "005 checksum 不得变化"
        );
        let five_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = '005_favorites_revision'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(five_count, 1, "005 record 保持 1 条");
    }

    #[test]
    fn runs_all_migrations() {
        let mut conn = fresh();
        run(&mut conn).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count as usize, MIGRATIONS.len());
        let image_cache_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'image_cache_entries'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(image_cache_exists, 1);
        let image_proxy_columns: Vec<String> = conn
            .prepare("PRAGMA table_info(image_proxy)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            image_proxy_columns
                .iter()
                .any(|column| column == "source_id")
        );
        assert!(
            image_proxy_columns
                .iter()
                .any(|column| column == "normalized_host")
        );
    }

    #[test]
    fn is_idempotent() {
        let mut conn = fresh();
        run(&mut conn).unwrap();
        run(&mut conn).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count as usize, MIGRATIONS.len());
    }

    #[test]
    fn legacy_023_upgrades_to_024_resource_preferences_without_data_loss() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_legacy_through(&mut conn, 23);
        conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();

        // Keep a representative global settings row while the new tables are added.
        conn.execute(
            "INSERT INTO settings (section, schema_version, revision, data_json, updated_at)
             VALUES ('reading', 1, 'legacy-reading',
                     '{\"section\":\"reading\",\"fontFamily\":\"serif\",\"fontSize\":\"medium\",\"lineHeight\":\"comfortable\",\"contentWidth\":\"medium\",\"theme\":\"warm\",\"fontWeight\":\"regular\",\"letterSpacing\":\"normal\",\"systemAuto\":true}', 1)",
            [],
        )
        .unwrap();

        run(&mut conn).unwrap();

        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name IN ('edition_preferences', 'media_item_preferences')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 2);
        let revision: String = conn
            .query_row(
                "SELECT revision FROM settings WHERE section = 'reading'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(revision, "legacy-reading");
        assert!(recorded_checksum(&conn, "024_resource_preferences").is_some());
    }

    #[test]
    fn rejects_modified_applied_migration() {
        let mut conn = fresh();
        run(&mut conn).unwrap();
        conn.execute(
            "UPDATE schema_migrations SET checksum = 'tampered' WHERE version = '001_initial'",
            [],
        )
        .unwrap();

        let error = run(&mut conn).unwrap_err();
        assert_eq!(error.code().as_str(), "MIGRATION_CHECKSUM_MISMATCH");
    }

    #[test]
    fn rejects_legacy_applied_migration_without_checksum() {
        let mut conn = fresh();
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES ('001_initial', 1)",
            [],
        )
        .unwrap();

        let error = run(&mut conn).unwrap_err();
        assert_eq!(error.code().as_str(), "MIGRATION_CHECKSUM_UNAVAILABLE");
    }

    #[test]
    fn rejects_unknown_migration_versions() {
        let mut conn = fresh();
        run(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO schema_migrations (version, checksum, applied_at) VALUES ('999_future', 'x', 1)",
            [],
        )
        .unwrap();

        let error = run(&mut conn).unwrap_err();
        assert_eq!(error.code().as_str(), "UNKNOWN_MIGRATION_VERSION");
    }

    /// 迁移 SQL 语义测试（**非 runner 证据**）：直接 execute_batch 执行 006 迁移文件，
    /// 验证「含孤儿版本行的 005 状态」经 006 SQL 正确过滤/重建/FK。
    /// 真实 runner 驱动的升级证据见 `legacy_005_to_006_upgrade_via_runner_succeeds` 与
    /// `legacy_005_to_006_via_runner_conflict_rolls_back`。
    #[test]
    fn upgrade_from_005_filters_orphan_version_rows() {
        let conn = fresh();
        // 应用 001~005（MIGRATIONS 顺序：index 0..=4 为 001~005），不经过 run() 的 checksum 路径。
        for (_, sql) in &MIGRATIONS[..5] {
            conn.execute_batch(sql).unwrap();
        }
        // 开启外键（模拟真实连接 configure），006 建表后 FK 生效。
        conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();

        // 构造 005 后的状态：有效版本行（Work 存在）+ 孤儿版本行（Work 不存在，005 无 FK 允许）。
        let work_id = "0196f0d2-0000-7000-8000-000000000aaa";
        let now = haven_common::UtcMillis::now().0;
        conn.execute_batch(&format!(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES ('{work_id}', '孤儿测试', 'fiction', 'completed', {now}, {now});
             INSERT INTO work_favorite_versions (work_id, revision, updated_at)
             VALUES ('{work_id}', 'rev-valid', {now}),
                    ('0196f0d2-0000-7000-8000-00000000ffff', 'rev-orphan', {now});"
        ))
        .unwrap();

        // 执行 006（真实升级 SQL）。
        conn.execute_batch(include_str!(
            "../../../../migrations/006_favorite_versions_fk.sql"
        ))
        .unwrap();

        // 有效行保留、孤儿行被过滤。
        let valid: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM work_favorite_versions WHERE work_id = ?1",
                rusqlite::params![work_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(valid, 1, "有效版本行必须保留");
        let orphans: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM work_favorite_versions WHERE work_id = '0196f0d2-0000-7000-8000-00000000ffff'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0, "孤儿版本行必须随迁移删除");

        // 删除 Work → 版本行级联删除（FK ON DELETE CASCADE）。
        conn.execute(
            "DELETE FROM works WHERE id = ?1",
            rusqlite::params![work_id],
        )
        .unwrap();
        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM work_favorite_versions WHERE work_id = ?1",
                rusqlite::params![work_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0, "删除 Work 后版本行级联删除");
    }

    /// 008/009 真实 runner 成功：真实已应用 001..007 的 legacy DB → 调用 `run` →
    /// 008 与 009 各恰好 1 条且 checksum 正确；007 record/checksum 不变；
    /// 008 唯一索引拒绝大小写重复 root_ref，**拒绝原因是唯一约束而非 SQL 语法错误**。
    #[test]
    fn legacy_007_to_009_via_runner_succeeds() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_legacy_through(&mut conn, 7);
        insert_storage_location(
            &conn,
            "0196f0d2-0000-7000-8000-000000000b01",
            "A",
            "D:\\Movies\\A",
        );
        insert_storage_location(
            &conn,
            "0196f0d2-0000-7000-8000-000000000b02",
            "B",
            "D:\\Movies\\B",
        );

        let seven_checksum = recorded_checksum(&conn, "007_settings").unwrap();
        let seven_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = '007_settings'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        // 真实 runner（非 execute_batch）。
        run(&mut conn).unwrap();

        // 007 record/checksum 不变。
        assert_eq!(
            recorded_checksum(&conn, "007_settings").as_deref(),
            Some(seven_checksum.as_str()),
            "007 checksum 不得变化"
        );
        let seven_after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = '007_settings'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(seven_after, seven_count, "007 record 不得重复");

        // 008 恰好 1 条且 checksum 正确。
        let eight_checksums: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT checksum FROM schema_migrations WHERE version = '008_storage_root_unique'")
                .unwrap();
            stmt.query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        assert_eq!(eight_checksums.len(), 1, "008 必须恰好记录 1 条");
        assert_eq!(
            eight_checksums[0],
            checksum(include_str!(
                "../../../../migrations/008_storage_root_unique.sql"
            ))
        );

        // 009 恰好 1 条且 checksum 正确。
        let nine_checksums: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT checksum FROM schema_migrations WHERE version = '009_availability_source'")
                .unwrap();
            stmt.query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        assert_eq!(nine_checksums.len(), 1, "009 必须恰好记录 1 条");
        assert_eq!(
            nine_checksums[0],
            checksum(include_str!(
                "../../../../migrations/009_availability_source.sql"
            ))
        );

        // 008 唯一索引：大小写重复 root_ref 被拒绝，且错误是唯一约束（UNIQUE），非语法错误。
        let dup = conn.execute(
            "INSERT INTO storage_locations
                (id, provider_type, display_name, root_ref, credential_ref, status, created_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-000000000b03', 'local', 'C', 'd:\\movies\\a', NULL, 'connected', 1, 1)",
            [],
        );
        let dup_err = dup.expect_err("大小写重复 root_ref 必须被 008 唯一索引拒绝");
        assert!(
            dup_err.to_string().contains("UNIQUE"),
            "拒绝原因必须是唯一约束而非 SQL 语法错误: {dup_err}"
        );
    }

    /// 008 真实 runner 失败回滚：真实 001..007 已登记 + 大小写重复 root_ref →
    /// `run` 返回 `MIGRATION_FAILED`；无 008、无 009 记录；旧重复行均保留；
    /// 007 record/checksum 不变。失败发生在迁移执行（非准备 SQL）。
    #[test]
    fn legacy_007_to_008_via_runner_duplicate_rolls_back() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_legacy_through(&mut conn, 7);
        insert_storage_location(
            &conn,
            "0196f0d2-0000-7000-8000-000000000c01",
            "A",
            "D:\\Movies\\A",
        );
        insert_storage_location(
            &conn,
            "0196f0d2-0000-7000-8000-000000000c02",
            "B",
            "d:\\movies\\a",
        );

        let seven_checksum = recorded_checksum(&conn, "007_settings").unwrap();

        let err = run(&mut conn).unwrap_err();
        assert_eq!(
            err.code().as_str(),
            "MIGRATION_FAILED",
            "008 遇到历史重复行必须报告 MIGRATION_FAILED"
        );

        // 无 008、无 009 记录。
        let eight: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = '008_storage_root_unique'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(eight, 0, "失败的 008 不得记录");
        let nine: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = '009_availability_source'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nine, 0, "009 不得在 008 失败后执行");

        // 旧重复行均保留。
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM storage_locations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 2, "旧重复行必须保留（回滚）");

        // 007 record/checksum 不变。
        assert_eq!(
            recorded_checksum(&conn, "007_settings").as_deref(),
            Some(seven_checksum.as_str()),
            "007 checksum 不得变化"
        );
    }

    /// 009 真实 runner 成功：真实已应用 001..008 的 legacy DB + 009 前资源历史行 →
    /// `run` 应用 009（及后续动态数量）；009 恰好 1 条且 checksum 正确；
    /// 历史行 availability_source 为 unknown；008 record/checksum 不变。
    #[test]
    fn legacy_008_to_009_via_runner_succeeds() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_legacy_through(&mut conn, 8);
        insert_pre_009_resource(&conn);

        let eight_checksum = recorded_checksum(&conn, "008_storage_root_unique").unwrap();
        let eight_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = '008_storage_root_unique'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        // 真实 runner。
        run(&mut conn).unwrap();

        // 009 恰好 1 条且 checksum 正确。
        let nine_checksums: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT checksum FROM schema_migrations WHERE version = '009_availability_source'")
                .unwrap();
            stmt.query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        assert_eq!(nine_checksums.len(), 1, "009 必须恰好记录 1 条");
        assert_eq!(
            nine_checksums[0],
            checksum(include_str!(
                "../../../../migrations/009_availability_source.sql"
            ))
        );

        // 历史资源行 availability_source 归为 unknown。
        let source: String = conn
            .query_row(
                "SELECT availability_source FROM resources WHERE id = '0196f0d2-0000-7000-8000-000000000d04'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(source, "unknown", "历史资源默认 unknown");

        // 008 record/checksum 不变。
        assert_eq!(
            recorded_checksum(&conn, "008_storage_root_unique").as_deref(),
            Some(eight_checksum.as_str()),
            "008 checksum 不得变化"
        );
        let eight_after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = '008_storage_root_unique'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(eight_after, eight_count, "008 record 不得重复");

        // runner 应用到当前全部迁移（动态数量）。
        let applied: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(applied as usize, MIGRATIONS.len());
    }

    /// 009 真实 runner 失败回滚：真实 001..008 已登记 + 预加同名列
    /// （resources.availability_source 已存在）→ 009 的 ALTER 冲突 →
    /// `run` 返回 `MIGRATION_FAILED`；无 009 记录；008 record/checksum 不变；
    /// 原资源行保留。失败发生在迁移执行（确定性 schema 冲突，非准备阶段误判）。
    #[test]
    fn legacy_008_to_009_via_runner_schema_conflict_rolls_back() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_legacy_through(&mut conn, 8);
        insert_pre_009_resource(&conn);

        let eight_checksum = recorded_checksum(&conn, "008_storage_root_unique").unwrap();

        // 预加与 009 目标同名的列（确定性 schema 冲突）。
        conn.execute_batch(
            "ALTER TABLE resources ADD COLUMN availability_source TEXT NOT NULL DEFAULT 'pre';",
        )
        .unwrap();

        let err = run(&mut conn).unwrap_err();
        assert_eq!(
            err.code().as_str(),
            "MIGRATION_FAILED",
            "009 与既有同名列冲突必须报告 MIGRATION_FAILED"
        );

        // 无 009 记录；008 record/checksum 不变。
        let nine: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = '009_availability_source'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nine, 0, "失败的 009 不得记录");
        assert_eq!(
            recorded_checksum(&conn, "008_storage_root_unique").as_deref(),
            Some(eight_checksum.as_str()),
            "008 checksum 不得变化"
        );

        // 原资源行保留（含预加列的值）。
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM resources WHERE id = '0196f0d2-0000-7000-8000-000000000d04'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "原资源行必须保留");
    }

    #[test]
    fn migration_010_creates_query_indexes_and_point_lookup_uses_them() {
        let mut conn = Connection::open_in_memory().unwrap();
        run(&mut conn).unwrap();
        for name in [
            "idx_resources_local_path",
            "idx_resources_storage",
            "idx_progress_work_active",
        ] {
            let found: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                    params![name],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(found, 1, "缺少索引 {name}");
        }
        // 点查必须命中表达式索引：防 json_extract JSON 路径拼写漂移后
        // 静默退化为全表扫描（功能仍正确但 P0-1 的性能修复失效）。
        let mut stmt = conn
            .prepare(
                "EXPLAIN QUERY PLAN SELECT id FROM resources
                 WHERE storage_location_id=?1 AND resource_type='local_file'
                   AND locator_kind='local_path'
                   AND json_extract(locator_json, '$.local_path.path')=?2",
            )
            .unwrap();
        let rows = stmt
            .query_map(params!["s", "p"], |row| {
                let n = row.as_ref().column_count();
                let mut parts = Vec::with_capacity(n);
                for i in 0..n {
                    let v: rusqlite::types::Value = row.get(i)?;
                    parts.push(format!("{v:?}"));
                }
                Ok(parts.join(" "))
            })
            .unwrap();
        let mut plan = String::new();
        for line in rows {
            plan.push_str(&line.unwrap());
        }
        assert!(
            plan.contains("idx_resources_local_path"),
            "local_file 点查未命中表达式索引：{plan}"
        );
    }

    #[test]
    fn legacy_011_upgrades_through_012_and_013_without_checksum_drift() {
        const LEGACY_011_CHECKSUM: &str =
            "a4c3d6d8e62808db62af7133ebcf0737c4991b668b2d99237fef0e524d2b37c7";
        let mut conn = Connection::open_in_memory().unwrap();
        apply_legacy_through(&mut conn, 11);

        let eleven_before = recorded_checksum(&conn, "011_download_tasks").unwrap();
        assert_eq!(eleven_before, LEGACY_011_CHECKSUM);

        run(&mut conn).unwrap();

        assert_eq!(
            recorded_checksum(&conn, "011_download_tasks").as_deref(),
            Some(LEGACY_011_CHECKSUM),
            "升级不得改写已登记的 011 checksum"
        );
        for version in [
            "012_download_transfer_metrics",
            "013_download_offline_resource",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = ?1",
                    params![version],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "{version} 必须恰好应用一次");
        }

        let columns: Vec<(String, i64)> = conn
            .prepare("PRAGMA table_info(download_tasks)")
            .unwrap()
            .query_map([], |row| Ok((row.get(1)?, row.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(
            columns
                .iter()
                .any(|(name, not_null)| name == "offline_resource_id" && *not_null == 0),
            "013 必须新增 nullable offline_resource_id"
        );

        let indexes: Vec<(String, i64)> = conn
            .prepare("PRAGMA index_list(download_tasks)")
            .unwrap()
            .query_map([], |row| Ok((row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(
            indexes.iter().any(|(name, unique)| {
                name == "idx_download_tasks_offline_resource_id" && *unique == 1
            }),
            "013 必须创建 offline_resource_id 唯一索引"
        );

        let foreign_keys: Vec<(String, String, String)> = conn
            .prepare("PRAGMA foreign_key_list(download_tasks)")
            .unwrap()
            .query_map([], |row| Ok((row.get(2)?, row.get(3)?, row.get(6)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(
            foreign_keys.iter().any(|(table, from, on_delete)| {
                table == "resources" && from == "offline_resource_id" && on_delete == "SET NULL"
            }),
            "013 必须建立 Resource FK ON DELETE SET NULL"
        );

        run(&mut conn).unwrap();
        for version in [
            "012_download_transfer_metrics",
            "013_download_offline_resource",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = ?1",
                    params![version],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "重复 run 不得重复应用 {version}");
        }
    }

    #[test]
    fn legacy_020_upgrades_to_021_without_losing_artwork_identity_or_trending_cache() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_legacy_through(&mut conn, 20);
        conn.execute(
            "INSERT INTO image_proxy (id, target_url, created_at)
             VALUES ('legacy-artwork', 'https://img.example.com/poster.jpg', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO trending_board_cache
                (board_id, source_id, payload_json, revision, refreshed_at, expires_at)
             VALUES ('anime', 'douban', '{\"boardId\":\"anime\"}', 'r1', 1, 2)",
            [],
        )
        .unwrap();

        run(&mut conn).unwrap();

        let source_id: Option<String> = conn
            .query_row(
                "SELECT source_id FROM image_proxy WHERE id = 'legacy-artwork'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source_id, None, "既有 artwork 身份必须保留为 legacy 行");
        let cache_table: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'image_cache_entries'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cache_table, 1, "021 必须创建本地 Artwork 文件索引");
        let trending_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM trending_board_cache WHERE board_id = 'anime'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(trending_rows, 1, "020 热榜快照不得在 021 升级中丢失");
        assert!(recorded_checksum(&conn, "021_artwork_cache").is_some());
    }

    #[test]
    fn legacy_021_artwork_sources_are_backfilled_only_for_known_hosts() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_legacy_through(&mut conn, 21);
        conn.execute_batch(
            "INSERT INTO image_proxy (id, target_url, created_at)
             VALUES
                ('legacy-cms10-http', 'http://img.picbf.com/poster-a.jpg', 1),
                ('legacy-cms10-https', 'https://IMG.PICBF.COM/poster-b.jpg', 1),
                ('legacy-opds-www', 'https://www.gutenberg.org/cache/book.png', 1),
                ('legacy-opds-root', 'http://gutenberg.org/cache/book.jpg', 1),
                ('legacy-signed', 'https://img.picbf.com/poster.jpg?signature=old', 1),
                ('legacy-fragment', 'https://www.gutenberg.org/book.png#cover', 1),
                ('legacy-unknown', 'https://images.example.invalid/poster.jpg', 1);",
        )
        .unwrap();

        run(&mut conn).unwrap();

        let rows: Vec<(String, Option<String>, Option<String>)> = conn
            .prepare(
                "SELECT id, source_id, normalized_host FROM image_proxy
                 ORDER BY id",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();

        assert_eq!(
            rows,
            vec![
                (
                    "legacy-cms10-http".to_owned(),
                    Some("cms10".to_owned()),
                    Some("img.picbf.com".to_owned()),
                ),
                (
                    "legacy-cms10-https".to_owned(),
                    Some("cms10".to_owned()),
                    Some("img.picbf.com".to_owned()),
                ),
                ("legacy-fragment".to_owned(), None, None),
                (
                    "legacy-opds-root".to_owned(),
                    Some("opds".to_owned()),
                    Some("gutenberg.org".to_owned()),
                ),
                (
                    "legacy-opds-www".to_owned(),
                    Some("opds".to_owned()),
                    Some("www.gutenberg.org".to_owned()),
                ),
                ("legacy-signed".to_owned(), None, None),
                ("legacy-unknown".to_owned(), None, None,),
            ],
            "022 只能为精确已知 Host 回填来源策略"
        );

        let id: String = conn
            .query_row(
                "SELECT id FROM image_proxy WHERE target_url = 'https://IMG.PICBF.COM/poster-b.jpg'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(id, "legacy-cms10-https", "迁移不得重建 Artwork 身份");
        assert!(recorded_checksum(&conn, "022_artwork_legacy_sources").is_some());
    }
}
