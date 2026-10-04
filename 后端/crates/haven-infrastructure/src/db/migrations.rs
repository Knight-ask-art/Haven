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
    (
        "029_multi_source_work_refs",
        include_str!("../../../../migrations/029_multi_source_work_refs.sql"),
    ),
    (
        "030_edition_profiles",
        include_str!("../../../../migrations/030_edition_profiles.sql"),
    ),
    (
        "031_comic_chapter_and_page_identity",
        include_str!("../../../../migrations/031_comic_chapter_and_page_identity.sql"),
    ),
    (
        "032_comic_progress_migration_snapshots",
        include_str!("../../../../migrations/032_comic_progress_migration_snapshots.sql"),
    ),
    (
        "033_comic_chapter_catalog_refresh",
        include_str!("../../../../migrations/033_comic_chapter_catalog_refresh.sql"),
    ),
    (
        "034_progress_opaque_revision",
        include_str!("../../../../migrations/034_progress_opaque_revision.sql"),
    ),
    (
        "035_comic_page_identity_revision",
        include_str!("../../../../migrations/035_comic_page_identity_revision.sql"),
    ),
    (
        "036_comic_chapter_profile_observation",
        include_str!("../../../../migrations/036_comic_chapter_profile_observation.sql"),
    ),
    (
        "037_comic_progress_subjects",
        include_str!("../../../../migrations/037_comic_progress_subjects.sql"),
    ),
    (
        "038_comic_progress_subject_members",
        include_str!("../../../../migrations/038_comic_progress_subject_members.sql"),
    ),
    (
        "039_comic_catalog_refresh_outcomes",
        include_str!("../../../../migrations/039_comic_catalog_refresh_outcomes.sql"),
    ),
    (
        "040_periodicals",
        include_str!("../../../../migrations/040_periodicals.sql"),
    ),
    (
        "041_periodical_article_content_availability",
        include_str!("../../../../migrations/041_periodical_article_content_availability.sql"),
    ),
    (
        "042_setting_proposals",
        include_str!("../../../../migrations/042_setting_proposals.sql"),
    ),
    (
        "043_agent_action_bindings",
        include_str!("../../../../migrations/043_agent_action_bindings.sql"),
    ),
    (
        "044_agent_approval_tokens",
        include_str!("../../../../migrations/044_agent_approval_tokens.sql"),
    ),
    (
        "045_interface_font_assets",
        include_str!("../../../../migrations/045_interface_font_assets.sql"),
    ),
    (
        "045_appearance_foundation",
        include_str!("../../../../migrations/045_appearance_foundation.sql"),
    ),
    (
        "046_appearance_layout_state",
        include_str!("../../../../migrations/046_appearance_layout_state.sql"),
    ),
    (
        "047_reading_activity",
        include_str!("../../../../migrations/047_reading_activity.sql"),
    ),
    (
        "048_overview_layout",
        include_str!("../../../../migrations/048_overview_layout.sql"),
    ),
    (
        "049_ai_provider_profiles",
        include_str!("../../../../migrations/049_ai_provider_profiles.sql"),
    ),
    (
        "050_agent_skills",
        include_str!("../../../../migrations/050_agent_skills.sql"),
    ),
    (
        "051_source_config_cache",
        include_str!("../../../../migrations/051_source_config_cache.sql"),
    ),
    (
        "052_cloud_drive_storage",
        include_str!("../../../../migrations/052_cloud_drive_storage.sql"),
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
    crate::lower_hex(&Sha256::digest(normalized.as_bytes()))
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

    /// 逐列读出全部设置行，用于断言迁移不改写既有设置事实。
    fn settings_rows(conn: &Connection) -> Vec<(String, i64, String, String, i64)> {
        let mut stmt = conn
            .prepare(
                "SELECT section, schema_version, revision, data_json, updated_at
                 FROM settings ORDER BY section",
            )
            .unwrap();
        stmt.query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
    }

    /// 按应用顺序读出全部迁移记录（version, checksum），用于断言只追加。
    fn recorded_migrations(conn: &Connection) -> Vec<(String, String)> {
        let mut stmt = conn
            .prepare("SELECT version, checksum FROM schema_migrations ORDER BY rowid")
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
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

        let comic_tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name IN (
                     'edition_profiles',
                     'comic_chapter_source_refs',
                     'comic_page_identities',
                     'comic_page_identity_states',
                     'comic_progress_migration_snapshots',
                     'comic_chapter_catalog_states',
                     'comic_progress_subjects',
                     'comic_progress_subject_members',
                     'comic_catalog_refresh_outcomes'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(comic_tables, 9, "漫画身份、主体和刷新结果表必须全部建立");
        let chapter_columns: Vec<String> = conn
            .prepare("PRAGMA table_info(comic_chapter_source_refs)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            chapter_columns
                .iter()
                .any(|column| column == "observed_edition_profile")
        );

        let periodical_tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name IN (
                     'periodicals',
                     'periodical_volumes',
                     'periodical_issues',
                     'periodical_articles'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(periodical_tables, 4, "报刊层级表必须全部建立");
    }

    /// 040 的 schema 语义：逐级 FK、ISSN 部分唯一索引、卷/期 identity_key
    /// 唯一（NULL 不参与比较）、文章来源身份唯一、删除父级级联清理子级。
    #[test]
    fn migration_040_builds_periodical_hierarchy_constraints() {
        let mut conn = Connection::open_in_memory().unwrap();
        run(&mut conn).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
        let now = haven_common::UtcMillis::now().0;
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-000000000e01', '报刊作品', 'standalone', 'completed', ?1, ?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-000000000e02', '0196f0d2-0000-7000-8000-000000000e01', '报刊版本', 'article', ?1, ?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media_items
                (id, edition_id, media_type, title, status, created_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-000000000e03', '0196f0d2-0000-7000-8000-000000000e02', 'article', '文章', 'available', ?1, ?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO periodicals (id, work_id, title, issn_print, issn_electronic, publisher, created_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-000000000e04', '0196f0d2-0000-7000-8000-000000000e01',
                     'Nature Communications', '2041-1723', NULL, NULL, ?1, ?1)",
            params![now],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-000000000e0d', '无 ISSN 作品', 'standalone', 'completed', ?1, ?1)",
            params![now],
        )
        .unwrap();
        let missing_issn = conn.execute(
            "INSERT INTO periodicals (id, work_id, title, issn_print, issn_electronic, publisher, created_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-000000000e0e', '0196f0d2-0000-7000-8000-000000000e0d',
                     '没有稳定身份的期刊', NULL, NULL, NULL, ?1, ?1)",
            params![now],
        );
        assert!(missing_issn.is_err(), "没有 ISSN 的期刊不能进入持久化模型");

        let negative_volume_number = conn.execute(
            "INSERT INTO periodical_volumes
                (id, periodical_id, label, number, year, ordinal, identity_key, created_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-000000000e0f', '0196f0d2-0000-7000-8000-000000000e04',
                     '-1', -1, 2024, 0, 'number:-1000', ?1, ?1)",
            params![now],
        );
        assert!(negative_volume_number.is_err(), "负卷号不能进入持久化模型");

        conn.execute(
            "INSERT INTO periodical_volumes
                (id, periodical_id, label, number, year, ordinal, identity_key, created_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-000000000e05', '0196f0d2-0000-7000-8000-000000000e04',
                     NULL, NULL, NULL, 0, 'unassigned', ?1, ?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO periodical_issues
                (id, volume_id, label, number, publication_date, ordinal, identity_key, created_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-000000000e06', '0196f0d2-0000-7000-8000-000000000e05',
                     '3-4', NULL, '2024-03-15', 0, 'label:3-4', ?1, ?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO periodical_articles
                (id, issue_id, media_item_id, ordinal, title, doi, page_range_start, page_range_end,
                 source_key, remote_article_id, created_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-000000000e07', '0196f0d2-0000-7000-8000-000000000e06',
                     '0196f0d2-0000-7000-8000-000000000e03', 12, '文章标题', '10.1038/x', 'e12345', NULL,
                     'europepmc', 'PMC1', ?1, ?1)",
            params![now],
        )
        .unwrap();

        // 卷身份键唯一：'unassigned' 不能重复建立第二个「来源未给出」的卷。
        let duplicate_volume = conn.execute(
            "INSERT INTO periodical_volumes
                (id, periodical_id, label, number, year, ordinal, identity_key, created_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-000000000e08', '0196f0d2-0000-7000-8000-000000000e04',
                     NULL, NULL, NULL, 1, 'unassigned', 1, 1)",
            [],
        );
        assert!(
            duplicate_volume
                .expect_err("同一期刊不能有两个相同身份键的卷")
                .to_string()
                .contains("UNIQUE")
        );

        // 来源身份唯一：重复 PMCID 不能建立第二篇文章。
        let duplicate_article = conn.execute(
            "INSERT INTO periodical_articles
                (id, issue_id, media_item_id, ordinal, title, doi, page_range_start, page_range_end,
                 source_key, remote_article_id, created_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-000000000e09', '0196f0d2-0000-7000-8000-000000000e06',
                     '0196f0d2-0000-7000-8000-000000000e03', NULL, '另一标题', NULL, NULL, NULL,
                     'europepmc', 'PMC1', 1, 1)",
            [],
        );
        assert!(
            duplicate_article
                .expect_err("同一来源身份不得重复入库")
                .to_string()
                .contains("UNIQUE")
        );

        // MediaItem 唯一：同一 MediaItem 不得被两条文章行绑定，否则阅读、进度与
        // 资源归属会同时属于两个互相矛盾的期刊归属。这里来源身份是新的，因此
        // 只有 media_item_id 唯一约束能拒绝它。
        let duplicate_media_item = conn.execute(
            "INSERT INTO periodical_articles
                (id, issue_id, media_item_id, ordinal, title, doi, page_range_start, page_range_end,
                 source_key, remote_article_id, created_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-000000000e0c', '0196f0d2-0000-7000-8000-000000000e06',
                     '0196f0d2-0000-7000-8000-000000000e03', NULL, '另一来源标题', NULL, NULL, NULL,
                     'europepmc', 'PMC2', 1, 1)",
            [],
        );
        assert!(
            duplicate_media_item
                .expect_err("同一 MediaItem 不得被两条文章行绑定")
                .to_string()
                .contains("periodical_articles.media_item_id"),
            "拒绝原因必须是 media_item_id 的唯一约束"
        );

        // ISSN 部分唯一索引：同一个 print ISSN 不能属于两个期刊。
        let duplicate_issn = conn.execute(
            "INSERT INTO periodicals (id, work_id, title, issn_print, issn_electronic, publisher, created_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-000000000e0a', '0196f0d2-0000-7000-8000-000000000e01',
                     '另一个期刊', '2041-1723', NULL, NULL, 1, 1)",
            [],
        );
        assert!(
            duplicate_issn
                .expect_err("print ISSN 必须唯一")
                .to_string()
                .contains("UNIQUE")
        );

        // 层级 FK：卷必须指向存在的期刊。
        let orphan_volume = conn.execute(
            "INSERT INTO periodical_volumes
                (id, periodical_id, label, number, year, ordinal, identity_key, created_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-000000000e0b', '0196f0d2-0000-7000-8000-00000000dead',
                     NULL, 1, NULL, 0, 'number:1000', 1, 1)",
            [],
        );
        assert!(orphan_volume.is_err(), "孤儿卷必须被外键拒绝");

        // 删除期刊级联清理卷/期；文章绑定 MediaItem，删除文章行不删除内容。
        conn.execute(
            "DELETE FROM periodicals WHERE id = '0196f0d2-0000-7000-8000-000000000e04'",
            [],
        )
        .unwrap();
        for table in [
            "periodical_volumes",
            "periodical_issues",
            "periodical_articles",
        ] {
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "删除期刊后 {table} 必须级联清理");
        }
        let media_items: i64 = conn
            .query_row("SELECT COUNT(*) FROM media_items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(media_items, 1, "级联不得删除文章内容本身");
    }

    /// 039 → 040 真实 runner 升级：既有内容不受影响，040 恰好应用一次。
    #[test]
    fn legacy_039_upgrades_to_040_without_touching_existing_content() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_legacy_through(&mut conn, 39);
        let now = haven_common::UtcMillis::now().0;
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-000000000f01', '旧作品', 'standalone', 'completed', ?1, ?1)",
            params![now],
        )
        .unwrap();
        let before = recorded_checksum(&conn, "039_comic_catalog_refresh_outcomes").unwrap();

        run(&mut conn).unwrap();

        let applied: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = '040_periodicals'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(applied, 1, "040 必须恰好应用一次");
        assert_eq!(
            recorded_checksum(&conn, "039_comic_catalog_refresh_outcomes").as_deref(),
            Some(before.as_str()),
            "039 的注册内容不得变化"
        );
        let works: i64 = conn
            .query_row("SELECT COUNT(*) FROM works", [], |row| row.get(0))
            .unwrap();
        assert_eq!(works, 1, "升级不得触碰既有作品");
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(total as usize, MIGRATIONS.len());
    }

    #[test]
    fn migration_034_backfills_opaque_progress_revisions() {
        let mut conn = Connection::open_in_memory().unwrap();
        // 该测试需要在 034 尚未应用时插入旧 Progress，避免 034 已经
        // 运行后再断言 backfill 失去意义；035 位于其后。
        apply_legacy_through(&mut conn, 33);
        let work_id = "0196f0d2-0000-7000-8000-00000000f401";
        let edition_id = "0196f0d2-0000-7000-8000-00000000f402";
        let media_item_id = "0196f0d2-0000-7000-8000-00000000f403";
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES (?1, '旧进度作品', 'fiction', 'completed', 1, 1)",
            params![work_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
             VALUES (?1, ?2, '漫画版', 'comic', 1, 1)",
            params![edition_id, work_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media_items
                (id, edition_id, media_type, title, category, chapter, page_count,
                 status, created_at, updated_at)
             VALUES (?1, ?2, 'comic', '第 1 话', 'comic', 1, 4, 'available', 1, 1)",
            params![media_item_id, edition_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO progress
                (id, work_id, edition_id, media_item_id, locator_json, locator_version,
                 completion, percentage, last_active_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-00000000f404', ?1, ?2, ?3, '{}', 1,
                     'in_progress', 0.25, 1234, 1234)",
            params![work_id, edition_id, media_item_id],
        )
        .unwrap();

        run(&mut conn).unwrap();
        let (revision, updated_at): (String, i64) = conn
            .query_row(
                "SELECT revision, updated_at FROM progress WHERE media_item_id = ?1",
                params![media_item_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(revision.starts_with("legacy-"));
        assert_ne!(revision, updated_at.to_string());
        assert_eq!(updated_at, 1234, "迁移不得修改展示时间");
    }

    #[test]
    fn migration_035_backfills_page_identity_revisions_without_rewriting_pages() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_legacy_through(&mut conn, 34);
        let work_id = "0196f0d2-0000-7000-8000-00000000f411";
        let edition_id = "0196f0d2-0000-7000-8000-00000000f412";
        let media_item_id = "0196f0d2-0000-7000-8000-00000000f413";
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES (?1, '旧页面观察作品', 'fiction', 'completed', 1, 1)",
            params![work_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
             VALUES (?1, ?2, '漫画版', 'comic', 1, 1)",
            params![edition_id, work_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media_items
                (id, edition_id, media_type, title, category, chapter, page_count,
                 status, created_at, updated_at)
             VALUES (?1, ?2, 'comic', '第 1 话', 'comic', 1, 2, 'available', 1, 1)",
            params![media_item_id, edition_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO comic_page_identities
                (media_item_id, page_index, stable_key, fingerprint, updated_at)
             VALUES (?1, 0, 'legacy-page-0', NULL, 4321),
                    (?1, 1, NULL, 'legacy-fingerprint-1', 4322)",
            params![media_item_id],
        )
        .unwrap();

        run(&mut conn).unwrap();
        let (revision, updated_at): (String, i64) = conn
            .query_row(
                "SELECT revision, updated_at
                 FROM comic_page_identity_states WHERE media_item_id = ?1",
                params![media_item_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(revision.starts_with("legacy-"));
        assert_eq!(updated_at, 4322);
        let pages: Vec<(i64, Option<String>, Option<String>)> = conn
            .prepare(
                "SELECT page_index, stable_key, fingerprint
                 FROM comic_page_identities WHERE media_item_id = ?1 ORDER BY page_index",
            )
            .unwrap()
            .query_map(params![media_item_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            pages,
            vec![
                (0, Some("legacy-page-0".into()), None),
                (1, None, Some("legacy-fingerprint-1".into())),
            ]
        );
    }

    #[test]
    fn migration_036_adds_source_profile_observation_without_rewriting_legacy_rows() {
        let mut conn = Connection::open_in_memory().unwrap();
        // Build a legacy chapter-source row before 036. Its profile observation
        // must remain NULL so Repository reads can use the historical Edition
        // projection without pretending it was a source-level observation.
        apply_legacy_through(&mut conn, 35);
        let work_id = "0196f0d2-0000-7000-8000-00000000f421";
        let edition_id = "0196f0d2-0000-7000-8000-00000000f422";
        let media_item_id = "0196f0d2-0000-7000-8000-00000000f423";
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES (?1, '旧来源画像作品', 'fiction', 'completed', 1, 1)",
            params![work_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
             VALUES (?1, ?2, '漫画版', 'comic', 1, 1)",
            params![edition_id, work_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media_items
                (id, edition_id, media_type, title, category, chapter, page_count,
                 status, created_at, updated_at)
             VALUES (?1, ?2, 'comic', '第 1 话', 'comic', 1, 2, 'available', 1, 1)",
            params![media_item_id, edition_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO comic_chapter_source_refs
                (source_key, remote_work_id, remote_chapter_id, media_item_id,
                 chapter_number, page_count, updated_at)
             VALUES ('legacy-source', 'legacy-work', 'legacy-chapter', ?1, 1, 2, 7)",
            params![media_item_id],
        )
        .unwrap();

        run(&mut conn).unwrap();
        let observed: Option<String> = conn
            .query_row(
                "SELECT observed_edition_profile
                 FROM comic_chapter_source_refs
                 WHERE source_key = 'legacy-source'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(observed.is_none(), "036 不得伪造旧来源的历史画像观察");
    }

    #[test]
    fn multi_source_work_refs_allow_many_sources_per_work() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_legacy_through(&mut conn, 28);
        let now = haven_common::UtcMillis::now().0;
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-000000000a01', '作品 A', 'fiction', 'completed', ?1, ?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES ('0196f0d2-0000-7000-8000-000000000a02', '作品 B', 'fiction', 'completed', ?1, ?1)",
            params![now],
        )
        .unwrap();
        // Legacy 014 只能先放一条引用；029 必须保留它并解除 work_id 单主键限制。
        conn.execute(
            "INSERT INTO work_source_refs (provider, external_id, work_id)
             VALUES ('legacy-source', 'legacy-work', '0196f0d2-0000-7000-8000-000000000a01')",
            [],
        )
        .unwrap();

        run(&mut conn).unwrap();

        conn.execute(
            "INSERT INTO work_source_refs (provider, external_id, work_id)
             VALUES ('second-source', 'second-work', '0196f0d2-0000-7000-8000-000000000a01')",
            [],
        )
        .unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM work_source_refs WHERE work_id = '0196f0d2-0000-7000-8000-000000000a01'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2, "一个 Work 必须保留多个来源引用");

        let conflict = conn.execute(
            "INSERT INTO work_source_refs (provider, external_id, work_id)
             VALUES ('legacy-source', 'legacy-work', '0196f0d2-0000-7000-8000-000000000a02')",
            [],
        );
        assert!(
            conflict
                .expect_err("同一来源远端作品不得绑定两个 Work")
                .to_string()
                .contains("UNIQUE"),
            "唯一约束必须落在来源身份，而不是 work_id"
        );

        conn.execute(
            "DELETE FROM works WHERE id = '0196f0d2-0000-7000-8000-000000000a01'",
            [],
        )
        .unwrap();
        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM work_source_refs WHERE work_id = '0196f0d2-0000-7000-8000-000000000a01'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0, "删除 Work 仍应级联清理全部来源引用");
    }

    #[test]
    fn legacy_028_upgrade_preserves_source_ref_and_builds_comic_schema() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_legacy_through(&mut conn, 28);
        let now = haven_common::UtcMillis::now().0;
        let work_id = "0196f0d2-0000-7000-8000-000000000b01";
        let edition_id = "0196f0d2-0000-7000-8000-000000000b02";
        let media_item_id = "0196f0d2-0000-7000-8000-000000000b03";
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES (?1, '迁移漫画作品', 'fiction', 'completed', ?2, ?2)",
            params![work_id, now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO editions (id, work_id, title, edition_type, language, created_at, updated_at)
             VALUES (?1, ?2, '中文漫画版', 'comic', 'zh-cn', ?3, ?3)",
            params![edition_id, work_id, now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media_items
                (id, edition_id, media_type, title, category, chapter, page_count,
                 status, created_at, updated_at)
             VALUES (?1, ?2, 'comic', '第 1 话', 'comic', 1.0, 3, 'available', ?3, ?3)",
            params![media_item_id, edition_id, now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO work_source_refs (provider, external_id, work_id)
             VALUES ('legacy-source', 'legacy-manga', ?1)",
            params![work_id],
        )
        .unwrap();

        run(&mut conn).unwrap();

        let stored_work: String = conn
            .query_row(
                "SELECT work_id FROM work_source_refs
                 WHERE provider = 'legacy-source' AND external_id = 'legacy-manga'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_work, work_id, "029 不得丢失旧来源引用");

        for table in [
            "edition_profiles",
            "comic_chapter_source_refs",
            "comic_page_identities",
            "comic_page_identity_states",
            "comic_progress_migration_snapshots",
            "comic_chapter_catalog_states",
        ] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    params![table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "缺少迁移表 {table}");
        }

        let work_id_primary_key: i64 = conn
            .query_row(
                "SELECT pk FROM pragma_table_info('work_source_refs') WHERE name = 'work_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(work_id_primary_key, 0, "029 必须移除 work_id 单列主键");
        let schema_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(schema_count as usize, MIGRATIONS.len());
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

    /// 041 为 040 时代已有的文章行保守补上 Provider 正文观察。
    ///
    /// 迁移前写入的行不可能被追溯成任何「已观察到的正文状态」，因此只能是
    /// `unknown`——既不是 `metadata_only`（那是对来源的断言），也不是 `full_text`。
    #[test]
    fn legacy_periodical_articles_gain_a_conservative_provider_observation() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_legacy_through(&mut conn, 40);
        // 这里只验证 ALTER TABLE 对既有行的取值，不构造完整归属链。
        conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
        conn.execute(
            "INSERT INTO periodical_articles
                (id, issue_id, media_item_id, ordinal, title, doi,
                 page_range_start, page_range_end, source_key, remote_article_id,
                 created_at, updated_at)
             VALUES ('legacy-article', 'legacy-issue', 'legacy-item', 1, '迁移前的文章',
                     NULL, NULL, NULL, 'europepmc', 'PMC-legacy', 1, 1)",
            [],
        )
        .unwrap();

        run(&mut conn).unwrap();

        let observed: String = conn
            .query_row(
                "SELECT provider_content_availability FROM periodical_articles
                 WHERE id = 'legacy-article'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            observed, "unknown",
            "没有 Provider 观察的旧行只能保守为 unknown"
        );

        // 列是闭合枚举：三态之外的取值写不进去。
        assert!(
            conn.execute(
                "UPDATE periodical_articles SET provider_content_availability = 'readable'
                 WHERE id = 'legacy-article'",
                [],
            )
            .is_err(),
            "闭合枚举之外的可用性取值必须被拒绝"
        );
        assert!(
            conn.execute(
                "UPDATE periodical_articles SET provider_content_availability = NULL
                 WHERE id = 'legacy-article'",
                [],
            )
            .is_err(),
            "正文观察是 NOT NULL：不能把缺失写成 NULL"
        );
    }

    /// 042 建立提案与回执两张表，并把作用域形状、状态闭合、changed 0/1、
    /// digest 长度与时间关系全部收敛成数据库层约束；回执只能对应真实存在的提案，
    /// 且与它 1:1。
    #[test]
    fn setting_proposal_tables_are_closed_and_related() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_legacy_through(&mut conn, 42);
        assert!(
            recorded_checksum(&conn, "042_setting_proposals").is_some(),
            "042 必须由真实 runner 记录"
        );

        let now = haven_common::UtcMillis::now().0;
        let digest = "a".repeat(64);

        // 一条形状完整、状态 pending 的 global 提案。
        let insert_proposal = |conn: &Connection,
                               id: &str,
                               scope: &str,
                               section: Option<&str>,
                               edition_id: Option<&str>,
                               media_item_id: Option<&str>,
                               digest: &str,
                               status: &str,
                               created_at: i64,
                               expires_at: i64| {
            conn.execute(
                "INSERT INTO setting_proposals
                    (id, scope, section, edition_id, media_item_id, payload_json, digest, status,
                     created_at, expires_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, '{}', ?6, ?7, ?8, ?9, ?8)",
                params![
                    id,
                    scope,
                    section,
                    edition_id,
                    media_item_id,
                    digest,
                    status,
                    created_at,
                    expires_at
                ],
            )
        };
        insert_proposal(
            &conn,
            "p-global",
            "global",
            Some("reading"),
            None,
            None,
            &digest,
            "pending",
            now,
            now + 1_000,
        )
        .unwrap();

        // status 是闭合集合。
        assert!(
            insert_proposal(
                &conn,
                "p-status",
                "global",
                Some("reading"),
                None,
                None,
                &digest,
                "confirmed",
                now,
                now + 1_000,
            )
            .is_err(),
            "闭合集合之外的提案状态必须被拒绝"
        );
        // scope 也是闭合集合，且形状与冗余 target 列绑定。
        assert!(
            insert_proposal(
                &conn,
                "p-scope",
                "workspace",
                None,
                None,
                None,
                &digest,
                "pending",
                now,
                now + 1_000,
            )
            .is_err(),
            "未知 scope 必须被拒绝"
        );
        assert!(
            insert_proposal(
                &conn,
                "p-global-no-section",
                "global",
                None,
                None,
                None,
                &digest,
                "pending",
                now,
                now + 1_000,
            )
            .is_err(),
            "global 提案必须携带 section"
        );
        assert!(
            insert_proposal(
                &conn,
                "p-edition-with-media",
                "edition",
                None,
                Some("e1"),
                Some("m1"),
                &digest,
                "pending",
                now,
                now + 1_000,
            )
            .is_err(),
            "edition 提案不允许携带 media_item_id"
        );
        assert!(
            insert_proposal(
                &conn,
                "p-media-item-no-edition",
                "media_item",
                None,
                None,
                Some("m1"),
                &digest,
                "pending",
                now,
                now + 1_000,
            )
            .is_err(),
            "media_item 提案必须携带所属 edition"
        );
        // digest 长度与时间关系。
        assert!(
            insert_proposal(
                &conn,
                "p-short-digest",
                "global",
                Some("reading"),
                None,
                None,
                "abc",
                "pending",
                now,
                now + 1_000,
            )
            .is_err(),
            "digest 必须是 64 位十六进制长度"
        );
        assert!(
            insert_proposal(
                &conn,
                "p-expiry",
                "global",
                Some("reading"),
                None,
                None,
                &digest,
                "pending",
                now,
                now,
            )
            .is_err(),
            "过期时间必须晚于创建时间"
        );

        // 回执：只能对应真实存在的提案。
        let insert_receipt = |conn: &Connection,
                              id: &str,
                              proposal_id: &str,
                              changed: i64,
                              applied_revision: Option<&str>| {
            conn.execute(
                "INSERT INTO setting_change_receipts
                    (id, proposal_id, proposal_digest, scope, section, edition_id, media_item_id,
                     before_json, after_json, applied_revision, changed, provenance_json, applied_at)
                 VALUES (?1, ?2, ?3, 'global', 'reading', NULL, NULL, '{}', '{}', ?4, ?5, '{}', ?6)",
                params![id, proposal_id, digest, applied_revision, changed, now],
            )
        };
        assert!(
            insert_receipt(&conn, "r-orphan", "p-missing", 0, None).is_err(),
            "回执不允许指向不存在的提案"
        );
        insert_receipt(&conn, "r-1", "p-global", 1, Some("set-1")).unwrap();
        assert!(
            insert_receipt(&conn, "r-2", "p-global", 0, None).is_err(),
            "一条提案最多一张回执"
        );
        assert!(
            insert_receipt(&conn, "r-3", "p-global", 2, Some("set-1")).is_err(),
            "changed 只能是 0/1"
        );
        assert!(
            insert_receipt(&conn, "r-4", "p-global", 1, None).is_err(),
            "changed=1 必须写出新的 applied_revision"
        );

        // 回执是 append-only 审计事实：有回执的提案不能被删除，避免历史被级联擦除。
        assert!(
            conn.execute("DELETE FROM setting_proposals WHERE id = 'p-global'", [])
                .is_err(),
            "已有回执的提案删除必须被外键保护"
        );
        let receipts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM setting_change_receipts WHERE proposal_id = 'p-global'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(receipts, 1, "删除失败时回执必须保留");

        // 目标列刻意不建外键：删除目标不能让审计事实消失，也不能反向阻塞删除。
        assert!(
            conn.execute(
                "INSERT INTO setting_proposals
                    (id, scope, section, edition_id, media_item_id, payload_json, digest, status,
                     created_at, expires_at, updated_at)
                 VALUES ('p-dangling-edition', 'edition', NULL, '0196f0d2-0000-7000-8000-0000000000ff',
                         NULL, '{}', ?1, 'pending', ?2, ?3, ?2)",
                params![digest, now, now + 1_000],
            )
            .is_ok(),
            "目标列不建外键：目标被删除后提案仍要能被读取并返回稳定的 target-not-found"
        );
    }

    #[test]
    fn agent_action_binding_table_is_closed_and_restricts_parent_deletion() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_legacy_through(&mut conn, 43);
        assert!(
            recorded_checksum(&conn, "043_agent_action_bindings").is_some(),
            "043 必须由真实 runner 记录"
        );

        let now = haven_common::UtcMillis::now().0;
        let digest = "a".repeat(64);
        conn.execute(
            "INSERT INTO setting_proposals
                (id, scope, section, edition_id, media_item_id, payload_json, digest, status,
                 created_at, expires_at, updated_at)
             VALUES ('p-agent-binding', 'global', 'reading', NULL, NULL, '{}', ?1, 'pending', ?2, ?3, ?2)",
            params![digest, now, now + 1_000],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_action_bindings
                (proposal_id, session_id, request_id, context_snapshot_id, context_hash,
                 subject_json, action_kind, created_at, expires_at, approval_state, updated_at)
             VALUES ('p-agent-binding', 'session-1', 'request-1', 'snapshot-1', ?1,
                     '{}', 'settings_proposal', ?2, ?3, 'pending', ?2)",
            params![digest, now, now + 1_000],
        )
        .unwrap();

        assert!(
            conn.execute(
                "UPDATE agent_action_bindings SET context_hash = 'short' WHERE proposal_id = 'p-agent-binding'",
                [],
            )
            .is_err(),
            "context_hash 必须保持 SHA-256 摘要长度"
        );
        assert!(
            conn.execute(
                "UPDATE agent_action_bindings SET action_kind = 'filesystem_write' WHERE proposal_id = 'p-agent-binding'",
                [],
            )
            .is_err(),
            "动作类型必须是服务端闭合集合"
        );
        assert!(
            conn.execute(
                "UPDATE agent_action_bindings SET approval_state = 'confirmed' WHERE proposal_id = 'p-agent-binding'",
                [],
            )
            .is_err(),
            "审批状态必须是服务端闭合集合"
        );
        assert!(
            conn.execute(
                "UPDATE agent_action_bindings SET expires_at = created_at WHERE proposal_id = 'p-agent-binding'",
                [],
            )
            .is_err(),
            "绑定过期时间必须晚于创建时间"
        );
        assert!(
            conn.execute(
                "DELETE FROM setting_proposals WHERE id = 'p-agent-binding'",
                [],
            )
            .is_err(),
            "有 Agent 绑定的提案不能被删除"
        );
        let binding_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_action_bindings WHERE proposal_id = 'p-agent-binding'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(binding_count, 1);
    }

    #[test]
    fn migration_044_adds_closed_approval_token_columns_and_preserves_043_rows() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_legacy_through(&mut conn, 43);

        let now = haven_common::UtcMillis::now().0;
        let digest = "a".repeat(64);
        conn.execute(
            "INSERT INTO setting_proposals
                (id, scope, section, edition_id, media_item_id, payload_json, digest, status,
                 created_at, expires_at, updated_at)
             VALUES ('p-044-legacy', 'global', 'reading', NULL, NULL, '{}', ?1, 'pending', ?2, ?3, ?2)",
            params![digest, now, now + 1_000],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_action_bindings
                (proposal_id, session_id, request_id, context_snapshot_id, context_hash,
                 subject_json, action_kind, created_at, expires_at, approval_state, updated_at)
             VALUES ('p-044-legacy', 'session-044', 'request-044', 'snapshot-044', ?1,
                     '{}', 'settings_proposal', ?2, ?3, 'pending', ?2)",
            params![digest, now, now + 1_000],
        )
        .unwrap();

        run(&mut conn).unwrap();
        assert!(recorded_checksum(&conn, "044_agent_approval_tokens").is_some());
        let legacy_token_columns: (Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT approval_token_hash, approval_token_issued_at
                 FROM agent_action_bindings WHERE proposal_id = 'p-044-legacy'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(legacy_token_columns, (None, None));

        conn.execute(
            "INSERT INTO setting_proposals
                (id, scope, section, edition_id, media_item_id, payload_json, digest, status,
                 created_at, expires_at, updated_at)
             VALUES ('p-044-token', 'global', 'reading', NULL, NULL, '{}', ?1, 'pending', ?2, ?3, ?2)",
            params![digest, now, now + 1_000],
        )
        .unwrap();
        let valid_hash = "b".repeat(64);
        conn.execute(
            "INSERT INTO agent_action_bindings
                (proposal_id, session_id, request_id, context_snapshot_id, context_hash,
                 subject_json, action_kind, created_at, expires_at, approval_state,
                 approval_token_hash, approval_token_issued_at, updated_at)
             VALUES ('p-044-token', 'session-044', 'request-044', 'snapshot-044', ?1,
                     '{}', 'settings_proposal', ?2, ?3, 'pending', ?4, ?2, ?2)",
            params![digest, now, now + 1_000, valid_hash],
        )
        .unwrap();

        // hash 只能是 64 位小写十六进制，且 hash / issued_at 必须成对存在。
        assert!(
            conn.execute(
                "UPDATE agent_action_bindings
                 SET approval_token_hash = NULL WHERE proposal_id = 'p-044-token'",
                [],
            )
            .is_err()
        );
        assert!(
            conn.execute(
                "UPDATE agent_action_bindings
                 SET approval_token_hash = ?1 WHERE proposal_id = 'p-044-token'",
                params!["A".repeat(64)],
            )
            .is_err()
        );
        assert!(
            conn.execute(
                "UPDATE agent_action_bindings
                 SET approval_token_hash = ?1 WHERE proposal_id = 'p-044-token'",
                params![format!("{}g", "b".repeat(63))],
            )
            .is_err()
        );
        assert!(
            conn.execute(
                "UPDATE agent_action_bindings
                 SET approval_token_issued_at = ?1 WHERE proposal_id = 'p-044-token'",
                params![now + 1_000],
            )
            .is_err()
        );

        // 终态必须清除 token 材料；清除后迁移到终态才允许。
        assert!(
            conn.execute(
                "UPDATE agent_action_bindings
                 SET approval_state = 'applied' WHERE proposal_id = 'p-044-token'",
                [],
            )
            .is_err()
        );
        conn.execute(
            "UPDATE agent_action_bindings
             SET approval_state = 'applied', approval_token_hash = NULL,
                 approval_token_issued_at = NULL
             WHERE proposal_id = 'p-044-token'",
            [],
        )
        .unwrap();
        let terminal_token_columns: (Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT approval_token_hash, approval_token_issued_at
                 FROM agent_action_bindings WHERE proposal_id = 'p-044-token'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(terminal_token_columns, (None, None));
    }

    /// 审计保留语义：删除目标（媒体条目/版本）既不能被提案或回执反向阻塞，
    /// 也不能把提案与回执连带擦掉——历史必须留着，apply 只回报稳定的
    /// `SETTING_PROPOSAL_TARGET_NOT_FOUND`。
    #[test]
    fn setting_proposal_audit_rows_survive_target_deletion() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_legacy_through(&mut conn, 42);

        let now = haven_common::UtcMillis::now().0;
        let digest = "b".repeat(64);
        let work_id = "0196f0d2-0000-7000-8000-00000000000a";
        let edition_id = "0196f0d2-0000-7000-8000-00000000000b";
        let media_item_id = "0196f0d2-0000-7000-8000-00000000000c";
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES (?1, '审计保留', 'fiction', 'completed', ?2, ?2)",
            params![work_id, now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
             VALUES (?1, ?2, '审计保留版本', 'book', ?3, ?3)",
            params![edition_id, work_id, now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media_items
                (id, edition_id, media_type, title, status, created_at, updated_at)
             VALUES (?1, ?2, 'book', '审计保留资源', 'available', ?3, ?3)",
            params![media_item_id, edition_id, now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO setting_proposals
                (id, scope, section, edition_id, media_item_id, payload_json, digest, status,
                 created_at, expires_at, updated_at)
             VALUES ('p-1', 'media_item', NULL, ?1, ?2, '{}', ?3, 'applied', ?4, ?5, ?4)",
            params![edition_id, media_item_id, digest, now, now + 1_000],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO setting_change_receipts
                (id, proposal_id, proposal_digest, scope, section, edition_id, media_item_id,
                 before_json, after_json, applied_revision, changed, provenance_json, applied_at)
             VALUES ('r-1', 'p-1', ?1, 'media_item', NULL, ?2, ?3, '{}', '{}', 'pref-media-1', 1,
                     '{}', ?4)",
            params![digest, edition_id, media_item_id, now],
        )
        .unwrap();

        assert_eq!(
            conn.execute(
                "DELETE FROM media_items WHERE id = ?1",
                params![media_item_id]
            )
            .unwrap(),
            1,
            "媒体条目必须能被删除：提案/回执的存在不得反向阻塞业务删除"
        );
        assert_eq!(
            conn.execute("DELETE FROM editions WHERE id = ?1", params![edition_id])
                .unwrap(),
            1,
            "版本必须能被删除"
        );

        for table in ["setting_proposals", "setting_change_receipts"] {
            let remaining: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(remaining, 1, "删除目标后 {table} 的审计事实必须保留");
        }
    }

    #[test]
    fn applies_ai_migrations_after_an_existing_048_database() {
        let mut conn = Connection::open_in_memory().unwrap();
        // 旧库停在 048：按版本名定位切点，而不是 `len() - 2`——后者在每次追加迁移后
        // 都会悄悄改变「旧库停在哪一版」，让断言的语义随追加漂移。
        let split = MIGRATIONS
            .iter()
            .position(|(version, _)| *version == "048_overview_layout")
            .expect("迁移列表必须包含 048_overview_layout")
            + 1;
        apply_legacy_through(&mut conn, split);
        let before = recorded_migrations(&conn);
        assert_eq!(
            before.last().map(|(version, _)| version.as_str()),
            Some("048_overview_layout"),
        );

        run(&mut conn).unwrap();

        let after = recorded_migrations(&conn);
        assert_eq!(
            &after[..before.len()],
            before.as_slice(),
            "已有 001-048 迁移记录必须原样保留",
        );
        assert_eq!(after.len(), before.len() + 4);
        assert_eq!(
            after[before.len()..]
                .iter()
                .map(|(version, _)| version.as_str())
                .collect::<Vec<_>>(),
            [
                "049_ai_provider_profiles",
                "050_agent_skills",
                "051_source_config_cache",
                "052_cloud_drive_storage",
            ],
        );

        for table in ["ai_provider_profiles", "agent_skill_states"] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "升级后必须创建 {table}");
        }
    }

    /// 045 建立外观 Foundation 的两张规范化表：资产元数据按种类分档设上限，资产 ID
    /// 只接受规范小写 UUID（永不接受路径或 URL），首页模块的集合/档位/坐标/唯一性
    /// 全部落在数据库层；既有 settings 行逐列不变（只追加迁移，不改写 001..044）。
    #[test]
    fn migration_045_builds_appearance_foundation_without_touching_settings() {
        // 045 的**发布形态** checksum（SHA-256 小写十六进制摘要）。与 011 的
        // `LEGACY_011_CHECKSUM` 同一条约定：写成字面量，而不是
        // `checksum(include_str!("...045..."))`——后者与被测对象同源，SQL 一旦被改写，
        // 期望值会跟着一起变，测试永远是绿的，等于没有守住「已发布的迁移不得改写」。
        // 字面量则相反：改 045 就必须显式改这个常量，而那正是「需要受控升级」的信号
        // （已落库的库会因为 checksum 变化报 `MIGRATION_CHECKSUM_MISMATCH`）。
        // 重新计算（runner 的 `checksum` 先把 CRLF 归一化成 LF，所以这里也必须归一化）：
        //   tr -d '\r' < 后端/migrations/045_appearance_foundation.sql | sha256sum
        const LEGACY_045_CHECKSUM: &str =
            "b2a774d3d8b5995d9454651405711c9f142ee695880ebe80042ab70cba1e5e6d";

        let mut conn = Connection::open_in_memory().unwrap();
        apply_legacy_through(&mut conn, 44);
        assert!(
            recorded_checksum(&conn, "044_agent_approval_tokens").is_some(),
            "045 之前必须有真实的 001..044 已应用记录"
        );

        // 代表性旧设置行：当前 appearance 形状，以及 b60661e 之前的五字段 reading 形状。
        conn.execute(
            "INSERT INTO settings (section, schema_version, revision, data_json, updated_at)
             VALUES ('appearance', 1, 'legacy-appearance',
                     '{\"section\":\"appearance\",\"theme\":\"system\",\"density\":\"comfortable\",\"sidebar\":\"auto\",\"reduceMotion\":false}', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings (section, schema_version, revision, data_json, updated_at)
             VALUES ('reading', 1, 'legacy-reading',
                     '{\"section\":\"reading\",\"fontFamily\":\"kai\",\"fontSize\":\"large\",\"lineHeight\":\"airy\",\"contentWidth\":\"wide\",\"theme\":\"dark\"}', 2)",
            [],
        )
        .unwrap();
        let checksum_044 = recorded_checksum(&conn, "044_agent_approval_tokens").unwrap();
        let settings_before = settings_rows(&conn);
        let migrations_before = recorded_migrations(&conn);

        run(&mut conn).unwrap();

        // 045 恰好一次，且 checksum 必须等于**发布形态**的字面量（断言见函数开头）。
        let applied_checksums: Vec<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT checksum FROM schema_migrations
                     WHERE version = '045_appearance_foundation'",
                )
                .unwrap();
            stmt.query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        assert_eq!(applied_checksums.len(), 1, "045 必须恰好记录 1 条");
        assert_eq!(
            applied_checksums[0], LEGACY_045_CHECKSUM,
            "045 checksum 必须与发布形态一致（不一致说明 045 的 SQL 被改写，需要受控升级）"
        );

        // append-only：044 的注册内容与既有设置行都不变。
        assert_eq!(
            recorded_checksum(&conn, "044_agent_approval_tokens").as_deref(),
            Some(checksum_044.as_str()),
            "045 不得改写 044 的注册内容"
        );
        assert_eq!(
            settings_rows(&conn),
            settings_before,
            "045 不得改写任何既有设置行"
        );

        // 只追加：既有迁移记录逐行不变；新 AI 迁移使用 049、050，不能插入已发布的 045-048。
        let migrations_after = recorded_migrations(&conn);
        assert_eq!(
            &migrations_after[..migrations_before.len()],
            migrations_before.as_slice(),
            "既有迁移记录必须逐行不变（只追加，不改写已发布迁移）"
        );
        assert_eq!(
            migrations_after.len(),
            migrations_before.len() + 9,
            "只允许追加 045-048 外观/总览迁移、049、050 AI 迁移、051 来源配置缓存与 052 云盘存储"
        );
        assert_eq!(
            migrations_after.last().map(|(version, _)| version.as_str()),
            Some("052_cloud_drive_storage")
        );
        for expected in [
            "045_interface_font_assets",
            "045_appearance_foundation",
            "046_appearance_layout_state",
            "047_reading_activity",
            "048_overview_layout",
            "049_ai_provider_profiles",
            "050_agent_skills",
            "051_source_config_cache",
            "052_cloud_drive_storage",
        ] {
            assert!(
                recorded_checksum(&conn, expected).is_some(),
                "缺少迁移记录 {expected}"
            );
        }

        for table in [
            "appearance_assets",
            "appearance_home_modules",
            "appearance_home_layout_meta",
            "appearance_overview_modules",
            "appearance_overview_layout_meta",
        ] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    params![table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "缺少外观表 {table}");
        }
        let asset_index: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index' AND name = 'idx_appearance_assets_kind_state'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(asset_index, 1, "资产读取路径索引必须建立");
        // 首页模块的排序键由 `UNIQUE (sort_order)` 自带的索引承担，不得再声明同列的
        // `idx_appearance_home_modules_order`：那不会更安全，只会多一份写入/维护成本。
        let duplicated_order_index: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index' AND name = 'idx_appearance_home_modules_order'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            duplicated_order_index, 0,
            "sort_order 的唯一约束已经提供同列索引，不得再建重复索引"
        );

        let now = haven_common::UtcMillis::now().0;
        let insert_asset = |conn: &Connection,
                            id: &str,
                            kind: &str,
                            state: &str,
                            byte_size: i64,
                            display_name: Option<&str>| {
            conn.execute(
                "INSERT INTO appearance_assets
                    (id, kind, validation_state, byte_size, display_name, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![id, kind, state, byte_size, display_name, now],
            )
        };

        let font_max = haven_domain::MAX_FONT_BYTES as i64;
        let static_max = haven_domain::MAX_STATIC_WALLPAPER_BYTES as i64;
        let dynamic_max = haven_domain::MAX_DYNAMIC_WALLPAPER_BYTES as i64;
        let display_name_max = haven_domain::MAX_ASSET_DISPLAY_NAME_CHARS;

        insert_asset(
            &conn,
            "0196f0d2-0000-7000-8000-0000000a0001",
            "font",
            "validated",
            font_max,
            Some("思源宋体"),
        )
        .unwrap();

        // 种类与校验状态是闭合集合。
        assert!(
            insert_asset(
                &conn,
                "0196f0d2-0000-7000-8000-0000000a0002",
                "video",
                "validated",
                1,
                None
            )
            .is_err(),
            "未知资产种类必须拒绝"
        );
        assert!(
            insert_asset(
                &conn,
                "0196f0d2-0000-7000-8000-0000000a0002",
                "font",
                "unknown",
                1,
                None
            )
            .is_err(),
            "未知校验状态必须拒绝"
        );
        // 体积必须为正，且不得超过该种类上限（上限与领域常量一一对应）。
        assert!(
            insert_asset(
                &conn,
                "0196f0d2-0000-7000-8000-0000000a0002",
                "font",
                "pending",
                0,
                None
            )
            .is_err(),
            "零字节资产必须拒绝"
        );
        assert!(
            insert_asset(
                &conn,
                "0196f0d2-0000-7000-8000-0000000a0002",
                "font",
                "pending",
                -1,
                None
            )
            .is_err(),
            "负体积必须拒绝"
        );
        assert!(
            insert_asset(
                &conn,
                "0196f0d2-0000-7000-8000-0000000a0002",
                "font",
                "pending",
                font_max + 1,
                None
            )
            .is_err(),
            "字体超出上限必须拒绝"
        );
        assert!(
            insert_asset(
                &conn,
                "0196f0d2-0000-7000-8000-0000000a0002",
                "static_wallpaper",
                "pending",
                static_max + 1,
                None
            )
            .is_err(),
            "静态壁纸不得使用动态壁纸的上限"
        );
        insert_asset(
            &conn,
            "0196f0d2-0000-7000-8000-0000000a0002",
            "static_wallpaper",
            "pending",
            static_max,
            None,
        )
        .unwrap();
        // 动态壁纸的上限本身可接受，但 max+1 必须拒绝：上限是边界，不是「更大就都行」。
        assert!(
            insert_asset(
                &conn,
                "0196f0d2-0000-7000-8000-0000000a0003",
                "dynamic_wallpaper",
                "pending",
                dynamic_max + 1,
                None
            )
            .is_err(),
            "动态壁纸超出上限必须拒绝"
        );
        insert_asset(
            &conn,
            "0196f0d2-0000-7000-8000-0000000a0003",
            "dynamic_wallpaper",
            "validated",
            dynamic_max,
            None,
        )
        .unwrap();

        // 资产 ID：不是路径或 URL，也不接受大写/无连字符 UUID；同一 ID 只能登记一次。
        assert!(
            insert_asset(&conn, "C:/assets/font.ttf", "font", "pending", 1, None).is_err(),
            "资产 ID 不得是路径"
        );
        assert!(
            insert_asset(
                &conn,
                "https://example.com/font.ttf",
                "font",
                "pending",
                1,
                None
            )
            .is_err(),
            "资产 ID 不得是 URL"
        );
        assert!(
            insert_asset(
                &conn,
                "0196F0D2-0000-7000-8000-0000000A0004",
                "font",
                "pending",
                1,
                None
            )
            .is_err(),
            "大写 UUID 不是规范文本身份"
        );
        assert!(
            insert_asset(
                &conn,
                "0196f0d20000700080000000a0004",
                "font",
                "pending",
                1,
                None
            )
            .is_err(),
            "无连字符 UUID 不是规范文本身份"
        );
        assert!(
            insert_asset(
                &conn,
                "0196f0d2-0000-7000-8000-0000000a0001",
                "font",
                "pending",
                1,
                None
            )
            .is_err(),
            "同一资产 ID 不得重复登记"
        );
        // SQLite 不会为 TEXT 主键隐式加 NOT NULL：NULL 资产 ID 必须被显式约束拒绝。
        assert!(
            conn.execute(
                "INSERT INTO appearance_assets
                    (id, kind, validation_state, byte_size, display_name, created_at, updated_at)
                 VALUES (NULL, 'font', 'pending', 1, NULL, 1, 1)",
                [],
            )
            .is_err(),
            "资产 ID 不得为空"
        );

        // 展示名：NULL 合法；未修剪、超长都不合法。
        let max_len_name = "名".repeat(display_name_max);
        insert_asset(
            &conn,
            "0196f0d2-0000-7000-8000-0000000a0005",
            "font",
            "pending",
            1,
            Some(max_len_name.as_str()),
        )
        .unwrap();
        assert!(
            insert_asset(
                &conn,
                "0196f0d2-0000-7000-8000-0000000a0006",
                "font",
                "pending",
                1,
                Some(" 名字 ")
            )
            .is_err(),
            "未修剪的展示名不得入库"
        );
        assert!(
            insert_asset(
                &conn,
                "0196f0d2-0000-7000-8000-0000000a0006",
                "font",
                "pending",
                1,
                Some("")
            )
            .is_err(),
            "空展示名不得入库"
        );
        let too_long_name = "n".repeat(display_name_max + 1);
        assert!(
            insert_asset(
                &conn,
                "0196f0d2-0000-7000-8000-0000000a0006",
                "font",
                "pending",
                1,
                Some(too_long_name.as_str())
            )
            .is_err(),
            "展示名超长必须拒绝"
        );
        assert!(
            conn.execute(
                "INSERT INTO appearance_assets
                    (id, kind, validation_state, byte_size, display_name, created_at, updated_at)
                 VALUES ('0196f0d2-0000-7000-8000-0000000a0007', 'font', 'pending', 1, NULL, 10, 9)",
                [],
            )
            .is_err(),
            "updated_at 不得早于 created_at"
        );

        let insert_module = |conn: &Connection,
                             module: &str,
                             size: &str,
                             row: i64,
                             column: i64,
                             order: i64,
                             version: i64| {
            conn.execute(
                "INSERT INTO appearance_home_modules
                    (module_id, size, row_index, column_index, sort_order, schema_version, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![module, size, row, column, order, version, now],
            )
        };

        let grid_rows = haven_domain::HOME_LAYOUT_GRID_ROWS as i64;
        let grid_columns = haven_domain::HOME_LAYOUT_GRID_COLUMNS as i64;
        let max_modules = haven_domain::HOME_LAYOUT_MAX_MODULES as i64;

        insert_module(&conn, "continue", "small", 0, 0, 0, 1).unwrap();

        // 闭合集合：未知模块（含下划线写法）与未知档位都进不来；schema 版本同样闭合。
        assert!(
            insert_module(&conn, "trending", "small", 3, 0, 3, 1).is_err(),
            "未知模块 ID 必须拒绝"
        );
        assert!(
            insert_module(&conn, "shelf_favorites", "small", 3, 0, 3, 1).is_err(),
            "下划线写法不是合法模块 ID"
        );
        assert!(
            insert_module(&conn, "recently_added", "huge", 3, 0, 3, 1).is_err(),
            "未知档位必须拒绝"
        );
        assert!(
            insert_module(&conn, "recently_added", "small", 3, 0, 3, 2).is_err(),
            "未知 schema 版本必须拒绝"
        );
        // 坐标与排序有界。
        assert!(
            insert_module(&conn, "recently_added", "small", grid_rows, 0, 3, 1).is_err(),
            "越界行坐标必须拒绝"
        );
        assert!(
            insert_module(&conn, "recently_added", "small", 3, grid_columns, 3, 1).is_err(),
            "越界列坐标必须拒绝"
        );
        assert!(
            insert_module(&conn, "recently_added", "small", 3, 0, max_modules, 1).is_err(),
            "越界排序必须拒绝"
        );
        // 档位跨度不得越出右侧边界。
        assert!(
            insert_module(
                &conn,
                "shelf-favorites",
                "medium",
                4,
                grid_columns - 1,
                4,
                1
            )
            .is_err(),
            "medium 不得越出右侧边界"
        );
        assert!(
            insert_module(&conn, "shelf-favorites", "large", 5, 1, 5, 1).is_err(),
            "large 不得从非零列开始"
        );
        insert_module(&conn, "recently_added", "small", 1, grid_columns - 1, 1, 1).unwrap();
        insert_module(&conn, "shelf-favorites", "large", 2, 0, 2, 1).unwrap();

        // 模块唯一：同一模块不得出现在第二个位置（位置本身是空闲的）。
        assert!(
            insert_module(&conn, "continue", "small", 6, 0, 6, 1)
                .expect_err("同一首页模块不得登记两次")
                .to_string()
                .contains("UNIQUE"),
            "拒绝原因必须是模块唯一约束"
        );
        // 占格重叠：换一个模块也不能压在已占用的格子上（continue 占着 (0,0)）。
        // 起始格相同只是重叠的一种情形；按档位跨度展开的真实判断见专门的测试。
        conn.execute(
            "DELETE FROM appearance_home_modules WHERE module_id = 'recently_added'",
            [],
        )
        .unwrap();
        assert!(
            insert_module(&conn, "recently_added", "medium", 0, 0, 1, 1)
                .expect_err("占格重叠的模块不得登记")
                .to_string()
                .contains("must not overlap"),
            "拒绝原因必须是占格重叠不变量（BEFORE 触发器先于唯一性检查执行）"
        );

        let layout_rows: Vec<(String, String, i64, i64, i64)> = conn
            .prepare(
                "SELECT module_id, size, row_index, column_index, sort_order
                 FROM appearance_home_modules ORDER BY module_id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            layout_rows,
            vec![
                ("continue".to_owned(), "small".to_owned(), 0, 0, 0),
                ("shelf-favorites".to_owned(), "large".to_owned(), 2, 0, 2),
            ],
            "被拒绝的写入不得留下任何行"
        );

        // 只追加：重复 run 不重复应用，也不改写设置行。
        run(&mut conn).unwrap();
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            total as usize,
            MIGRATIONS.len(),
            "重复 run 不得重复应用 045"
        );
        assert_eq!(
            settings_rows(&conn),
            settings_before,
            "重复 run 也不得改写既有设置行"
        );
    }

    /// 045 的 `display_name` CHECK 必须与 haven-domain 的 `bounded_display_name`
    /// 对同一组输入给出同一结论：首尾空白按 Unicode White_Space 全集修剪（与 Rust
    /// `str::trim` 同一集合，SQLite 默认的 `trim(X)` 只去半角空格），控制字符覆盖
    /// 0x00..0x1f 与 0x7f（与 `has_opaque_control_character` 同一集合），
    /// 格式/双向控制字符覆盖 18 个码位（与 `has_display_name_format_control` 同一集合）。
    /// 领域侧对应的钉住测试是 `display_name_trims_exactly_unicode_white_space`、
    /// `display_name_rejects_c0_control_characters_and_del` 与
    /// `display_name_rejects_format_and_bidi_control_characters`。
    #[test]
    fn migration_045_display_name_semantics_match_the_domain() {
        let mut conn = Connection::open_in_memory().unwrap();
        run(&mut conn).unwrap();

        let now = haven_common::UtcMillis::now().0;
        let mut next_id = 0u32;
        let mut insert_name = |conn: &Connection, name: Option<&str>| {
            next_id += 1;
            let id = format!("0196f0d2-0000-7000-8000-{next_id:012x}");
            conn.execute(
                "INSERT INTO appearance_assets
                    (id, kind, validation_state, byte_size, display_name, created_at, updated_at)
                 VALUES (?1, 'font', 'pending', 1, ?2, ?3, ?3)",
                params![id, name, now],
            )
        };
        // 同一串文本也过一遍领域：两端要么都收、要么都拒，测试才有「同集合」的证据。
        let domain_reads = |raw: &str| {
            haven_domain::AppearanceAssetMetadata::new(
                "0196f0d2-0000-7000-8000-0000000a0001"
                    .parse::<haven_domain::AppearanceAssetId>()
                    .unwrap(),
                haven_domain::AppearanceAssetKind::Font,
                haven_domain::AssetValidationState::Pending,
                1,
                Some(raw.to_owned()),
            )
        };

        // Unicode White_Space 全集（与 haven-domain 的 UNICODE_WHITE_SPACE 逐项对应）。
        const UNICODE_WHITE_SPACE: [u32; 25] = [
            0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x20, 0x85, 0xa0, 0x1680, 0x2000, 0x2001, 0x2002, 0x2003,
            0x2004, 0x2005, 0x2006, 0x2007, 0x2008, 0x2009, 0x200a, 0x2028, 0x2029, 0x202f, 0x205f,
            0x3000,
        ];
        // 格式/双向控制字符：与领域 `has_display_name_format_control` 逐项一致的 18 个
        // 码位（零宽字符、双向嵌入/隔离、行/段分隔符、BOM）。它们不打印任何字形，却能让
        // 两个不同的展示名在界面上渲染成同一串字，因此与 C0/DEL 一样任何位置都拒绝。
        const FORMAT_CONTROLS: [u32; 18] = [
            0x200b, 0x200c, 0x200d, 0x200e, 0x200f, // 零宽与双向标记
            0x2028, 0x2029, // 行分隔符 / 段分隔符
            0x202a, 0x202b, 0x202c, 0x202d, 0x202e, // 双向嵌入/覆盖
            0x2060, // 无宽不换行
            0x2066, 0x2067, 0x2068, 0x2069, // 双向隔离
            0xfeff, // BOM / 零宽不换行
        ];

        for code_point in UNICODE_WHITE_SPACE {
            let character = char::from_u32(code_point).unwrap();
            // 首尾空白 = 未修剪，库侧只接受修剪后的形态。
            assert!(
                insert_name(
                    &conn,
                    Some(format!("{character}思源宋体{character}").as_str())
                )
                .is_err(),
                "U+{code_point:04X} 作为首尾空白必须拒绝（落库只接受修剪后的形态）"
            );
            let interior = format!("思源{character}宋体");
            if code_point <= 0x1f {
                // 0x09..0x0d 同时属于 C0：它们在任何位置都被控制字符检查拒绝。
                assert!(
                    insert_name(&conn, Some(interior.as_str())).is_err(),
                    "U+{code_point:04X} 是 C0，出现在中间同样必须拒绝"
                );
            } else if FORMAT_CONTROLS.contains(&code_point) {
                // 0x2028/0x2029 同时属于格式字符：首尾只可能被 trim 掉（上面那条断言），
                // 出现在中间则按格式字符拒绝——两端都只收修剪后的形态，结论一致。
                assert!(
                    insert_name(&conn, Some(interior.as_str())).is_err(),
                    "U+{code_point:04X} 是格式字符，出现在中间必须拒绝"
                );
                assert!(
                    domain_reads(&interior).is_err(),
                    "领域必须与 SQL 同集合地拒绝中间的 U+{code_point:04X}"
                );
            } else {
                assert!(
                    insert_name(&conn, Some(interior.as_str())).is_ok(),
                    "U+{code_point:04X} 只算首尾空白，中间位置不得被修剪"
                );
                assert!(
                    domain_reads(&interior).is_ok(),
                    "U+{code_point:04X} 在中间位置两端都必须照常接受"
                );
            }
        }

        // 与空白或控制字符「长得像」但不在任何拒绝清单里的码位：两端都必须原样保留
        // （清单是精确的，不是「0x20xx 一律拒绝」）。
        for code_point in [0x00ad, 0x180e, 0x2065, 0xfe00] {
            let character = char::from_u32(code_point).unwrap();
            let raw = format!("{character}名{character}");
            assert!(
                insert_name(&conn, Some(raw.as_str())).is_ok(),
                "U+{code_point:04X} 不在任何拒绝清单里，不得被拒绝"
            );
            assert_eq!(
                domain_reads(&raw)
                    .unwrap_or_else(|e| panic!("领域同样不得拒绝 U+{code_point:04X}: {e}"))
                    .display_name
                    .as_deref(),
                Some(raw.as_str()),
                "U+{code_point:04X} 必须原样保留"
            );
        }

        // 控制字符：C0（0x00..0x1f）与 DEL（0x7f）在任何位置都拒绝。
        // 0x00 由 `instr(display_name, char(0))` 覆盖（GLOB 的字符类承载不了 NUL）。
        for code_point in (0x00u32..=0x1f).chain(std::iter::once(0x7f)) {
            let character = char::from_u32(code_point).unwrap();
            for raw in [
                format!("名{character}字"),
                format!("{character}名"),
                format!("名{character}"),
            ] {
                assert!(
                    insert_name(&conn, Some(raw.as_str())).is_err(),
                    "U+{code_point:04X} 不得入库：{raw:?}"
                );
            }
        }

        // 格式/双向控制字符逐项过两端。
        for code_point in FORMAT_CONTROLS {
            let character = char::from_u32(code_point).unwrap();
            // 中间位置：两端都拒绝（修剪救不了它）。
            let interior = format!("名{character}字");
            assert!(
                insert_name(&conn, Some(interior.as_str())).is_err(),
                "U+{code_point:04X} 出现在中间时必须拒绝：{interior:?}"
            );
            assert!(
                domain_reads(&interior).is_err(),
                "领域必须与 SQL 同集合地拒绝中间的 U+{code_point:04X}"
            );
            // 首尾位置：SQL 只收修剪后的形态，所以原样形态一律拒绝；领域对同时是
            // White_Space 的 0x2028/0x2029 只是修剪（`"\u{2028}名"` → `"名"`），
            // 对其余 16 个码位才整段拒绝。
            for edge in [format!("{character}名"), format!("名{character}")] {
                assert!(
                    insert_name(&conn, Some(edge.as_str())).is_err(),
                    "未修剪的形态不得入库：{edge:?}"
                );
                if character.is_whitespace() {
                    assert_eq!(
                        domain_reads(&edge)
                            .unwrap_or_else(|e| panic!("U+{code_point:04X} 在首尾只是被修剪: {e}"))
                            .display_name
                            .as_deref(),
                        Some("名"),
                        "U+{code_point:04X} 同时是 White_Space，首尾只被修剪"
                    );
                } else {
                    assert!(
                        domain_reads(&edge).is_err(),
                        "U+{code_point:04X} 在首尾同样必须拒绝：{edge:?}"
                    );
                }
            }
        }

        // 长度按码位计数（与领域的 `chars().count()` 一致，不是字节数）。
        assert!(insert_name(&conn, Some("名".repeat(120).as_str())).is_ok());
        assert!(
            insert_name(&conn, Some("名".repeat(121).as_str())).is_err(),
            "超出 120 个码位必须拒绝"
        );
        let long_emoji = "😀".repeat(120);
        assert_eq!("😀".chars().count(), 1);
        assert!(
            insert_name(&conn, Some(long_emoji.as_str())).is_ok(),
            "多字节码位必须按码位计数，不能按字节数拒绝"
        );
        assert!(
            insert_name(&conn, Some("😀".repeat(121).as_str())).is_err(),
            "121 个多字节码位必须拒绝"
        );

        // 库能存下的每个规范形态，领域都必须原样读回：不能出现「库里有、域读不了」
        // （域在写入前已修剪，因此未修剪的形态只可能被 SQL 拒绝，不会造成漂移）。
        let canonical: [&str; 6] = [
            "思源宋体",
            "思源 宋体",
            "名\u{00a0}别 名",
            "名\u{2065}字",
            "混合 Latin 名称.txt",
            long_emoji.as_str(),
        ];
        for (index, value) in canonical.iter().enumerate() {
            let id = format!("0196f0d2-0000-7000-8000-{:012x}", 0xf00 + index);
            conn.execute(
                "INSERT INTO appearance_assets
                    (id, kind, validation_state, byte_size, display_name, created_at, updated_at)
                 VALUES (?1, 'font', 'pending', 1, ?2, ?3, ?3)",
                params![id, value, now],
            )
            .unwrap_or_else(|e| panic!("规范展示名必须入库 {value:?}: {e}"));
            let stored: String = conn
                .query_row(
                    "SELECT display_name FROM appearance_assets WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(stored, *value, "落库文本不得被 SQLite 静默改写");
            assert_eq!(
                domain_reads(&stored)
                    .unwrap_or_else(|e| panic!("领域必须能读回库里的展示名 {value:?}: {e}"))
                    .display_name
                    .as_deref(),
                Some(*value),
                "落库形态必须是领域修剪后的不动点"
            );
        }
    }

    /// 045 的 `UNIQUE (sort_order)`：并列排序号不得入库，否则领域侧承诺的稳定顺序
    /// 在持久化后不再成立。领域侧对应的钉住测试是 `home_layout_rejects_duplicate_order`。
    #[test]
    fn migration_045_rejects_duplicate_home_module_order() {
        let mut conn = Connection::open_in_memory().unwrap();
        run(&mut conn).unwrap();

        let now = haven_common::UtcMillis::now().0;
        let insert_module = |conn: &Connection,
                             module: &str,
                             size: &str,
                             row: i64,
                             column: i64,
                             order: i64| {
            conn.execute(
                    "INSERT INTO appearance_home_modules
                        (module_id, size, row_index, column_index, sort_order, schema_version, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
                    params![module, size, row, column, order, now],
                )
        };

        insert_module(&conn, "continue", "small", 0, 0, 0).unwrap();
        insert_module(&conn, "recently_added", "small", 1, 0, 1).unwrap();

        // 模块与网格位置都是空闲的，唯一冲突只可能来自 sort_order。
        assert!(
            insert_module(&conn, "shelf-favorites", "small", 5, 0, 1)
                .expect_err("并列 sort_order 必须拒绝")
                .to_string()
                .contains("sort_order"),
            "拒绝原因必须是 sort_order 唯一约束"
        );
        // 换一个空闲位置、重复另一个既有排序号：同样拒绝（位置不是拒绝原因）。
        assert!(
            insert_module(&conn, "shelf-favorites", "small", 6, 0, 0)
                .expect_err("并列 sort_order 必须拒绝")
                .to_string()
                .contains("sort_order")
        );

        // 唯一排序号照常登记，读取顺序与 sort_order 一致。
        insert_module(&conn, "shelf-favorites", "medium", 2, 0, 2).unwrap();
        let orders: Vec<i64> = {
            let mut stmt = conn
                .prepare("SELECT sort_order FROM appearance_home_modules ORDER BY sort_order")
                .unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        assert_eq!(orders, vec![0, 1, 2], "合法布局的顺序必须是全序");

        // 被拒绝的写入不得留下任何行。
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM appearance_home_modules", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 3, "被拒绝的写入不得留下任何行");
    }

    /// 045 的占格重叠不变量：起始格不同也可能压在同一个格子上，所以拒绝必须按档位跨度
    /// 展开后的**实际占格**判断，而不是只比较起始格。INSERT 与 UPDATE 两条写入路径都要
    /// 守住，否则「移动模块 / 改档位」就能绕过它。领域侧对应
    /// `home_layout_rejects_span_expanded_cell_overlap` 与
    /// `placement_occupied_cells_expand_by_size_span`。
    #[test]
    fn migration_045_rejects_span_expanded_home_module_overlap() {
        let mut conn = Connection::open_in_memory().unwrap();
        run(&mut conn).unwrap();

        let now = haven_common::UtcMillis::now().0;
        let insert_module = |conn: &Connection,
                             module: &str,
                             size: &str,
                             row: i64,
                             column: i64,
                             order: i64| {
            conn.execute(
                "INSERT INTO appearance_home_modules
                    (module_id, size, row_index, column_index, sort_order, schema_version, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
                params![module, size, row, column, order, now],
            )
        };
        let cells_of = |conn: &Connection, module: &str| -> (i64, i64, String) {
            conn.query_row(
                "SELECT row_index, column_index, size FROM appearance_home_modules
                 WHERE module_id = ?1",
                params![module],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap()
        };

        // medium 从第 1 列开始占 (0,0)(0,1)：它的起始格是 (0,0)，第 2 列也属于它。
        insert_module(&conn, "continue", "medium", 0, 0, 0).unwrap();
        // 起始格不同（(0,1) 对 (0,0)），但第 2 列同时属于两者。
        assert!(
            insert_module(&conn, "recently_added", "small", 0, 1, 1)
                .expect_err("起始格不同但占格重叠的模块不得登记")
                .to_string()
                .contains("must not overlap"),
            "拒绝原因必须是占格重叠不变量"
        );
        // 换成不重叠的位置照常登记（只比较起始格的写法会在这里放过 (0,1)，所以这条不能少）。
        insert_module(&conn, "recently_added", "small", 1, 1, 1).unwrap();
        insert_module(&conn, "shelf-favorites", "large", 2, 0, 2).unwrap();

        // UPDATE 路径：把模块挪到别人的格子上同样拒绝。
        assert!(
            conn.execute(
                "UPDATE appearance_home_modules SET row_index = 0
                 WHERE module_id = 'recently_added'",
                [],
            )
            .expect_err("移动模块不得压到别的模块的格子上")
            .to_string()
            .contains("must not overlap"),
            "拒绝原因必须是占格重叠不变量"
        );
        assert_eq!(
            cells_of(&conn, "recently_added"),
            (1, 1, "small".to_owned()),
            "被拒绝的移动不得留下任何字段改动"
        );

        // 档位变更会扩大占格：medium 从第 3 列开始占 (0,2)(0,3)，与同行的 small 相邻而不
        // 重叠，必须照常成功；换成同段 medium 则完全重叠，必须拒绝。
        conn.execute(
            "UPDATE appearance_home_modules SET size = 'medium', column_index = 2
             WHERE module_id = 'recently_added'",
            [],
        )
        .unwrap();
        assert_eq!(
            cells_of(&conn, "recently_added"),
            (1, 2, "medium".to_owned())
        );
        assert!(
            conn.execute(
                "UPDATE appearance_home_modules SET row_index = 1, column_index = 2
                 WHERE module_id = 'continue'",
                [],
            )
            .expect_err("同段 medium 不得压在同一格上")
            .to_string()
            .contains("must not overlap")
        );

        // 被拒绝的写入不得留下任何行或字段改动。
        let rows: Vec<(String, String, i64, i64, i64)> = conn
            .prepare(
                "SELECT module_id, size, row_index, column_index, sort_order
                 FROM appearance_home_modules ORDER BY module_id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![
                ("continue".to_owned(), "medium".to_owned(), 0, 0, 0),
                ("recently_added".to_owned(), "medium".to_owned(), 1, 2, 1),
                ("shelf-favorites".to_owned(), "large".to_owned(), 2, 0, 2),
            ],
            "被拒绝的写入不得留下任何行"
        );
    }

    /// 048 的占格重叠不变量：与 045 同形，但网格换成**三列**、large 的跨度换成 3。
    ///
    /// 这条测试同时是「总览与首页不是同一张表」的证据：同样的 `large` 在首页占 4 列、
    /// 在总览占 3 列，`column_index` 的合法上界也因此不同（2 而不是 3）。领域侧对应
    /// `overview_layout_rejects_duplicates_positions_and_bad_versions` 与
    /// `overview_placement_occupied_cells_expand_by_size_span`。
    #[test]
    fn migration_048_rejects_span_expanded_overview_module_overlap() {
        let mut conn = Connection::open_in_memory().unwrap();
        run(&mut conn).unwrap();

        let now = haven_common::UtcMillis::now().0;
        let insert_module = |conn: &Connection,
                             module: &str,
                             size: &str,
                             row: i64,
                             column: i64,
                             order: i64| {
            conn.execute(
                "INSERT INTO appearance_overview_modules
                    (module_id, size, row_index, column_index, sort_order, schema_version, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
                params![module, size, row, column, order, now],
            )
        };
        let cells_of = |conn: &Connection, module: &str| -> (i64, i64, String) {
            conn.query_row(
                "SELECT row_index, column_index, size FROM appearance_overview_modules
                 WHERE module_id = ?1",
                params![module],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap()
        };

        // medium 从第 1 列开始占 (0,0)(0,1)：起始格是 (0,0)，第 2 列也属于它。
        insert_module(&conn, "preferences", "medium", 0, 0, 0).unwrap();
        // 起始格不同（(0,1) 对 (0,0)），但第 2 列同时属于两者。
        assert!(
            insert_module(&conn, "metrics", "small", 0, 1, 1)
                .expect_err("起始格不同但占格重叠的模块不得登记")
                .to_string()
                .contains("must not overlap"),
            "拒绝原因必须是占格重叠不变量"
        );
        // 换成第 3 列（三列网格的最后一列）就不重叠，必须照常登记。
        insert_module(&conn, "metrics", "small", 0, 2, 1).unwrap();
        // large 占满整行三列。
        insert_module(&conn, "reading-minutes", "large", 1, 0, 2).unwrap();
        // 同一行再放一个模块必然与 large 重叠，哪怕起始格是最后一列。
        assert!(
            insert_module(&conn, "type-share", "small", 1, 2, 3)
                .expect_err("large 占满整行，同一行的任何模块都与它重叠")
                .to_string()
                .contains("must not overlap")
        );

        // 越界与闭合集合的负向断言都用**尚未登记**的 `type-share` 做探针，避免主键冲突
        // 掩盖真正的拒绝原因。
        // 三列网格里 medium 从第 3 列开始会越出右侧边界（首页的四列网格允许它）。
        assert!(
            insert_module(&conn, "type-share", "medium", 2, 2, 3).is_err(),
            "medium 在第 3 列的跨度越出三列网格"
        );
        assert!(
            insert_module(&conn, "type-share", "small", 12, 0, 3).is_err(),
            "行坐标必须小于 12"
        );
        // 首页的模块 ID 不得进入总览布局：两套闭合集合互不通用。
        assert!(
            insert_module(&conn, "shelf-favorites", "small", 2, 0, 3).is_err(),
            "首页模块 ID 不得登记到总览布局"
        );
        // 下划线写法不是合法总览模块 ID（连字符才是）。
        assert!(insert_module(&conn, "type_share", "small", 2, 0, 3).is_err());
        // 并列排序号必须拒绝（稳定顺序是库里的既成事实，不是读取时的巧合）。
        assert!(
            insert_module(&conn, "type-share", "small", 2, 0, 2).is_err(),
            "并列 sort_order 必须拒绝"
        );

        insert_module(&conn, "type-share", "small", 2, 2, 3).unwrap();
        insert_module(&conn, "reading-heatmap", "large", 3, 0, 4).unwrap();

        // UPDATE 路径：把模块挪到别人的格子上同样拒绝。
        assert!(
            conn.execute(
                "UPDATE appearance_overview_modules SET row_index = 1
                 WHERE module_id = 'metrics'",
                [],
            )
            .expect_err("移动模块不得压到别的模块的格子上")
            .to_string()
            .contains("must not overlap")
        );
        assert_eq!(
            cells_of(&conn, "metrics"),
            (0, 2, "small".to_owned()),
            "被拒绝的移动不得留下任何字段改动"
        );

        // 档位变更会扩大占格：把第 3 行的 small 挪到同行的第 1 列并扩成 medium，占 (2,0)(2,1)，
        // 与第 3 列的空格相邻而不重叠，必须照常成功（证明这条不变量不是「同行只能有一个模块」）。
        conn.execute(
            "UPDATE appearance_overview_modules SET size = 'medium', column_index = 0
             WHERE module_id = 'type-share'",
            [],
        )
        .unwrap();
        assert_eq!(cells_of(&conn, "type-share"), (2, 0, "medium".to_owned()));
        // 扩到越界则必须拒绝：同一行的第 2 列起放 medium 会超出三列网格。
        assert!(
            conn.execute(
                "UPDATE appearance_overview_modules SET column_index = 2
                 WHERE module_id = 'type-share'",
                [],
            )
            .is_err()
        );

        // 「从未保存过」与「显式保存了空布局」由 meta 行区分（与 046 同形）。
        let meta_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM appearance_overview_layout_meta",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(meta_rows, 0, "只写模块行不得产生 meta 行");

        // 被拒绝的写入不得留下任何行。
        let modules: Vec<String> = conn
            .prepare("SELECT module_id FROM appearance_overview_modules ORDER BY sort_order")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            modules,
            vec![
                "preferences".to_owned(),
                "metrics".to_owned(),
                "reading-minutes".to_owned(),
                "type-share".to_owned(),
                "reading-heatmap".to_owned(),
            ]
        );
    }
}
