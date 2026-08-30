//! Sqlite Unit of Work（BE-APP-001 事务编排）。
//!
//! "检查 + 写入"等跨 Repository 操作在单一 SQLite 事务内执行：
//! - begin：unchecked_transaction（业务闭包在调用线程同步执行）。
//! - 闭包返回 Ok → commit；Err → 自动 rollback（tx Drop 时回滚）。
//! - 闭包内不执行异步 IO，只做同步数据库操作。

use std::sync::Arc;

use rusqlite::Transaction;

use haven_common::AppError;
use haven_domain::entities::FavoriteTarget;
use haven_domain::ids::WorkId;

use crate::db::Db;
use haven_application::services::ports::{FavoriteState, FavoriteTxPorts, UnitOfWork};

/// R-MAIN-09D：purge 中间表唯一内部名（明确 temp schema；DROP 用同一定义，避免散落字符串）。
const PURGE_TEMP_TABLE: &str = "temp._haven_storage_purge_media_ids";
const PURGE_TEMP_DROP_SQL: &str = "DROP TABLE IF EXISTS temp._haven_storage_purge_media_ids";

/// SQLite 版 UnitOfWork。
pub struct SqliteUnitOfWork {
    db: Arc<Db>,
}

impl SqliteUnitOfWork {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

impl UnitOfWork for SqliteUnitOfWork {
    fn run_favorite(
        &self,
        f: &dyn Fn(&dyn FavoriteTxPorts) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        let guard = self.db.lock();
        let tx = guard
            .unchecked_transaction()
            .map_err(|e| tx_err("开启事务失败", e))?;
        let scope = SqliteFavoriteTx { tx: &tx };
        match f(&scope) {
            Ok(()) => tx.commit().map_err(|e| tx_err("提交事务失败", e)),
            Err(e) => {
                // tx Drop 时自动 rollback
                Err(e)
            }
        }
    }
}

struct SqliteFavoriteTx<'a> {
    tx: &'a Transaction<'a>,
}

fn target_columns(target: &FavoriteTarget) -> (Option<String>, Option<String>, Option<String>) {
    match target {
        FavoriteTarget::Work(id) => (Some(id.to_string()), None, None),
        FavoriteTarget::Edition(id) => (None, Some(id.to_string()), None),
        FavoriteTarget::MediaItem(id) => (None, None, Some(id.to_string())),
    }
}

fn clear_sql_and_param(target: &FavoriteTarget) -> (&'static str, String) {
    match target {
        FavoriteTarget::Work(id) => ("DELETE FROM favorites WHERE work_id = ?1", id.to_string()),
        FavoriteTarget::Edition(id) => (
            "DELETE FROM favorites WHERE edition_id = ?1",
            id.to_string(),
        ),
        FavoriteTarget::MediaItem(id) => (
            "DELETE FROM favorites WHERE media_item_id = ?1",
            id.to_string(),
        ),
    }
}

impl FavoriteTxPorts for SqliteFavoriteTx<'_> {
    fn work_exists(&self, work_id: WorkId) -> Result<bool, AppError> {
        let exists: i64 = self
            .tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM works WHERE id = ?1)",
                rusqlite::params![work_id.to_string()],
                |r| r.get(0),
            )
            .map_err(|e| tx_err("查询作品失败", e))?;
        Ok(exists > 0)
    }

    fn favorite_state(&self, target: &FavoriteTarget) -> Result<Option<FavoriteState>, AppError> {
        let (work, edition, media_item) = target_columns(target);
        // favorites 行存在 → active；版本行提供 revision（005 迁移：状态版本持久化）。
        let active: bool = self
            .tx
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM favorites
                    WHERE (?1 IS NOT NULL AND work_id = ?1)
                       OR (?2 IS NOT NULL AND edition_id = ?2)
                       OR (?3 IS NOT NULL AND media_item_id = ?3)
                 )",
                rusqlite::params![work, edition, media_item],
                |r| r.get::<_, i64>(0),
            )
            .map(|v| v > 0)
            .map_err(|e| tx_err("查询收藏状态失败", e))?;

        let revision = match &work {
            Some(id) => self
                .tx
                .query_row(
                    "SELECT revision FROM work_favorite_versions WHERE work_id = ?1",
                    rusqlite::params![id],
                    |r| r.get::<_, String>(0),
                )
                .optional()
                .map_err(|e| tx_err("查询收藏版本失败", e))?,
            None => None,
        };

        if !active && revision.is_none() {
            return Ok(None);
        }
        Ok(Some(FavoriteState { active, revision }))
    }

    fn apply_favorite(
        &self,
        target: &FavoriteTarget,
        on: bool,
        revision: &str,
    ) -> Result<(), AppError> {
        if on {
            // 互斥表达：先清同类 target 再插入（与 SqliteFavoriteRepository 同语义）。
            let (clear, param) = clear_sql_and_param(target);
            self.tx
                .execute(clear, rusqlite::params![param])
                .map_err(|e| tx_err("清除收藏失败", e))?;
            let (work, edition, media_item) = target_columns(target);
            self.tx
                .execute(
                    "INSERT INTO favorites (work_id, edition_id, media_item_id, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![work, edition, media_item, haven_common::UtcMillis::now().0],
                )
                .map_err(|e| tx_err("收藏写入失败", e))?;
        } else {
            let (sql, param) = clear_sql_and_param(target);
            self.tx
                .execute(sql, rusqlite::params![param])
                .map_err(|e| tx_err("取消收藏失败", e))?;
        }

        // 状态版本持久化（005 迁移：work_favorite_versions），取消收藏也保留版本行，
        // 使"重复取消"返回相同 revision（状态版本语义，R-FAV-001）。
        if let FavoriteTarget::Work(id) = target {
            self.tx
                .execute(
                    "INSERT INTO work_favorite_versions (work_id, revision, updated_at)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(work_id) DO UPDATE SET revision = excluded.revision, updated_at = excluded.updated_at",
                    rusqlite::params![id.to_string(), revision, haven_common::UtcMillis::now().0],
                )
                .map_err(|e| tx_err("收藏版本写入失败", e))?;
        }
        Ok(())
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

/// rusqlite 0.31+ 的 OptionalExtension 路径。
trait OptionalExt {
    fn optional(self) -> Result<Option<String>, rusqlite::Error>;
}
impl OptionalExt for Result<String, rusqlite::Error> {
    fn optional(self) -> Result<Option<String>, rusqlite::Error> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// SQLite 版存储位置 Unit of Work（P0-5：位置状态与 Resource 更新原子提交）。
pub struct SqliteStorageUoW {
    db: Arc<Db>,
}

impl SqliteStorageUoW {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

impl haven_application::services::storage_location::StorageLocationUoW for SqliteStorageUoW {
    fn run(
        &self,
        f: &dyn Fn(
            &dyn haven_application::services::storage_location::StorageTxPorts,
        ) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        // R-MAIN-09D：TEMP 中间表的防御性清理放在**事务边界外**、同一 db mutex guard 内，
        // 避免"事务内 DROP+CREATE 后业务失败回滚使旧 TEMP 表复活"。SQLite TEMP DDL 是
        // 事务性的：事务回滚会撤销本次 CREATE，但也会恢复**事务开始前已存在**的旧表；
        // 因此在事务开始前把旧表清走，事务失败回滚后就不会有旧表残留。
        let guard = self.db.lock();
        guard
            .execute_batch(PURGE_TEMP_DROP_SQL)
            .map_err(|e| tx_err("清理 purge 临时表失败", e))?;

        let tx = guard
            .unchecked_transaction()
            .map_err(|e| tx_err("开启存储事务失败", e))?;
        let scope = SqliteStorageTx { tx: &tx };
        let result = match f(&scope) {
            Ok(()) => tx.commit().map_err(|e| tx_err("提交存储事务失败", e)),
            Err(e) => {
                // 显式结束事务（drop → rollback），随后在 guard 内做事务外清理。
                drop(tx);
                Err(e)
            }
        };
        // 事务已结束（commit consume 或 drop），仍持同一 guard 清理（不持有 Transaction、
        // 不释放 guard）。
        let cleanup = guard
            .execute_batch(PURGE_TEMP_DROP_SQL)
            .map_err(|e| tx_err("清理 purge 临时表失败", e));

        // 错误优先级：主操作 Err 优先返回原错误（cleanup 失败不得把失败变成成功）；
        // 主操作 Ok 但 cleanup 失败 → 返回 DATABASE_ERROR。
        match (result, cleanup) {
            (Err(e), _) => Err(e),
            (Ok(()), Err(e)) => Err(e),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    /// 事务外短读（R-MAIN-03）：FS 探测前的初始快照，不持有长事务。
    fn read_location(
        &self,
        id: haven_domain::ids::StorageLocationId,
    ) -> Result<Option<haven_domain::entities::StorageLocation>, AppError> {
        use crate::db::repos::storage_location::row_to_storage_location;
        let guard = self.db.lock();
        let mut stmt = guard
            .prepare(
                "SELECT id, provider_type, display_name, root_ref, credential_ref, status, created_at, updated_at
                 FROM storage_locations WHERE id = ?1",
            )
            .map_err(|e| tx_err("查询存储位置失败", e))?;
        let mut rows = stmt
            .query_map(rusqlite::params![id.to_string()], row_to_storage_location)
            .map_err(|e| tx_err("查询存储位置失败", e))?;
        rows.next()
            .transpose()
            .map_err(|e| tx_err("查询存储位置失败", e))
    }
}

struct SqliteStorageTx<'a> {
    tx: &'a Transaction<'a>,
}

impl haven_application::services::storage_location::StorageTxPorts for SqliteStorageTx<'_> {
    fn load_location(
        &self,
        id: haven_domain::ids::StorageLocationId,
    ) -> Result<Option<haven_domain::entities::StorageLocation>, AppError> {
        use crate::db::repos::storage_location::row_to_storage_location;
        let mut stmt = self
            .tx
            .prepare(
                "SELECT id, provider_type, display_name, root_ref, credential_ref, status, created_at, updated_at
                 FROM storage_locations WHERE id = ?1",
            )
            .map_err(|e| tx_err("查询存储位置失败", e))?;
        let mut rows = stmt
            .query_map(rusqlite::params![id.to_string()], row_to_storage_location)
            .map_err(|e| tx_err("查询存储位置失败", e))?;
        rows.next()
            .transpose()
            .map_err(|e| tx_err("查询存储位置失败", e))
    }

    fn load_all(&self) -> Result<Vec<haven_domain::entities::StorageLocation>, AppError> {
        use crate::db::repos::storage_location::row_to_storage_location;
        let mut stmt = self
            .tx
            .prepare(
                "SELECT id, provider_type, display_name, root_ref, credential_ref, status, created_at, updated_at
                 FROM storage_locations ORDER BY created_at",
            )
            .map_err(|e| tx_err("查询存储位置列表失败", e))?;
        let rows = stmt
            .query_map([], row_to_storage_location)
            .map_err(|e| tx_err("查询存储位置列表失败", e))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| tx_err("查询存储位置列表失败", e))
    }

    fn save_location(
        &self,
        location: &haven_domain::entities::StorageLocation,
    ) -> Result<(), AppError> {
        use crate::db::repos::enum_to_db_str;
        self.tx
            .execute(
                "INSERT INTO storage_locations
                    (id, provider_type, display_name, root_ref, credential_ref, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(id) DO UPDATE SET
                     provider_type = excluded.provider_type,
                     display_name = excluded.display_name,
                     root_ref = excluded.root_ref,
                     credential_ref = excluded.credential_ref,
                     status = excluded.status,
                     updated_at = excluded.updated_at",
                rusqlite::params![
                    location.id.to_string(),
                    enum_to_db_str(&location.provider_type)?,
                    location.display_name,
                    location.root_ref,
                    location
                        .credential_ref
                        .as_ref()
                        .map(|r| r.as_str().to_owned()),
                    enum_to_db_str(&location.status)?,
                    location.created_at.0,
                    location.updated_at.0,
                ],
            )
            .map_err(|e| tx_err("保存存储位置失败", e))?;
        Ok(())
    }

    fn set_resources_availability(
        &self,
        storage_location_id: haven_domain::ids::StorageLocationId,
        availability: haven_domain::enums::Availability,
        source: haven_domain::enums::AvailabilitySource,
    ) -> Result<(), AppError> {
        // R-MAIN-08 覆盖规则（位置失效/无效化）只允许：
        //   a) 当前 availability='available' 的资源（即便 source=user，位置不可达时有效可用性必须失效）；或
        //   b) availability_source='storage' 的资源（重复/状态迁移收敛）。
        // 不得覆盖 source=user 且当前为 SourceUnavailable/TemporarilyUnavailable/Unknown/自身 Missing。
        self.tx
            .execute(
                "UPDATE resources SET availability = ?1, availability_source = ?2, updated_at = ?3
                 WHERE storage_location_id = ?4
                   AND (availability = 'available' OR availability_source = 'storage')",
                rusqlite::params![
                    serde_json::to_string(&availability)
                        .map_err(|e| {
                            tx_err(
                                "序列化可用性失败",
                                rusqlite::Error::ToSqlConversionFailure(Box::new(e)),
                            )
                        })?
                        .trim_matches('"'),
                    serde_json::to_string(&source)
                        .map_err(|e| {
                            tx_err(
                                "序列化可用性来源失败",
                                rusqlite::Error::ToSqlConversionFailure(Box::new(e)),
                            )
                        })?
                        .trim_matches('"'),
                    haven_common::UtcMillis::now().0,
                    storage_location_id.to_string()
                ],
            )
            .map_err(|e| tx_err("批量标记资源可用性失败", e))?;
        Ok(())
    }

    /// 读取某存储位置下的全部 Resource（rebind rebase 用；失败回滚整个事务）。
    /// `ORDER BY id` 保证确定性顺序（R-MAIN-08A 混合回滚测试依赖：先处理较小 ID 的有效
    /// locator，再遇到较大 ID 的非法 locator → 已保存的第一个也必须回滚）。
    fn load_resources(
        &self,
        storage_location_id: haven_domain::ids::StorageLocationId,
    ) -> Result<Vec<haven_domain::entities::Resource>, AppError> {
        let mut stmt = self
            .tx
            .prepare("SELECT * FROM resources WHERE storage_location_id = ?1 ORDER BY id")
            .map_err(|e| tx_err("查询位置资源失败", e))?;
        let rows = stmt
            .query_map(
                rusqlite::params![storage_location_id.to_string()],
                crate::db::repos::resource::row_to_resource,
            )
            .map_err(|e| tx_err("查询位置资源失败", e))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| tx_err("解析位置资源失败", e))
    }

    /// 保存 Resource（事务内；rebind rebase 原子提交用）。
    fn save_resource(&self, resource: &haven_domain::entities::Resource) -> Result<(), AppError> {
        crate::db::repos::resource::save_on_conn(self.tx, resource)
    }

    fn delete_resources(
        &self,
        storage_location_id: haven_domain::ids::StorageLocationId,
    ) -> Result<(), AppError> {
        self.tx
            .execute(
                "DELETE FROM resources WHERE storage_location_id = ?1",
                rusqlite::params![storage_location_id.to_string()],
            )
            .map_err(|e| tx_err("删除位置索引资源失败", e))?;
        Ok(())
    }

    /// INTEGRATION-SLICE-001 真机验收发现（「选错目录」缺口）：remove 只删
    /// resources + storage_locations 会留下孤儿 works/editions/media_items，
    /// 媒体库仍显示已移除位置扫出的内容。
    ///
    /// 本方法在**同一事务**内：删除该位置 Resource 后，级联清理**仅由该位置派生**
    /// 的孤儿内容链（media_items → editions → works）及其用户状态
    /// （progress / markers / favorites / history_entries，均为 RESTRICT 须先行删除）。
    /// 其他位置仍引用的内容（共享 edition/work）完整保留；`work_favorite_versions`
    /// 随 works 外键 CASCADE。孤儿判定经连接级临时表传递（无 IN 参数上限问题）。
    fn purge_location_content(
        &self,
        storage_location_id: haven_domain::ids::StorageLocationId,
    ) -> Result<(), AppError> {
        let loc = storage_location_id.to_string();
        let exec = |sql: &str| {
            self.tx
                .execute(sql, [])
                .map_err(|e| tx_err("位置内容清理失败", e))?;
            Ok::<(), AppError>(())
        };

        // R-MAIN-09C/09D Important 1：TEMP 中间表用唯一内部名并**明确限定 temp schema**，
        // 避免共享 SQLite 连接上跨事务残留、以及未限定 DROP 误伤 main 同名对象。
        // 事务开始前的防御性清理由 `SqliteStorageUoW::run` 在事务边界外（同一 guard）完成，
        // 因此此处不再需要开头 DROP；成功后仍保留 DROP 作为局部自清理。
        let purge_temp = PURGE_TEMP_TABLE;
        exec(
            "CREATE TEMP TABLE _haven_storage_purge_media_ids(
                 stage INTEGER NOT NULL, id TEXT NOT NULL, parent TEXT, depth INTEGER NOT NULL DEFAULT 0)",
        )?;
        self.tx
            .execute(
                "INSERT INTO temp._haven_storage_purge_media_ids(stage, id, parent, depth)
                 SELECT DISTINCT 0, media_item_id, NULL, 0 FROM resources
                 WHERE storage_location_id = ?1",
                rusqlite::params![&loc],
            )
            .map_err(|e| tx_err("位置内容清理失败", e))?;

        // 下载任务先行清理：source_resource_id（RESTRICT）与 target_storage_id
        // （RESTRICT）指向被移除位置的任务已失去内容源/目标，随位置一并删除。
        // offline_resource_id 为 ON DELETE SET NULL，不构成阻塞。
        exec(&format!(
            "DELETE FROM download_tasks
             WHERE source_resource_id IN (SELECT id FROM resources WHERE storage_location_id = '{loc}')
                OR target_storage_id = '{loc}'"
        ))?;

        self.delete_resources(storage_location_id)?;

        // stage 1：从已删资源的 media_item 沿 parent_id 向上闭包，记录深度。
        // 删除时按 depth 升序（child first），满足 parent_id ON DELETE RESTRICT；仍被其他资源或
        // 子节点引用的候选会在实际 DELETE 条件中保留，从而支持共享层级。
        exec(&format!(
            "INSERT INTO {purge_temp}(stage, id, parent, depth)
             WITH RECURSIVE candidates(id, depth, path) AS (
                 SELECT id, 0, ',' || id || ',' FROM {purge_temp} WHERE stage = 0
                 UNION ALL
                 SELECT m.parent_id, c.depth + 1, c.path || m.parent_id || ','
                 FROM media_items m
                 JOIN candidates c ON m.id = c.id
                 WHERE m.parent_id IS NOT NULL
                   AND instr(c.path, ',' || m.parent_id || ',') = 0
             )
             SELECT 1, m.id, m.edition_id, MAX(c.depth)
             FROM media_items m
             JOIN candidates c ON c.id = m.id
             WHERE NOT EXISTS (SELECT 1 FROM resources r WHERE r.media_item_id = m.id)
             GROUP BY m.id, m.edition_id"
        ))?;

        let max_depth: Option<i64> = self
            .tx
            .query_row(
                &format!("SELECT MAX(depth) FROM {purge_temp} WHERE stage = 1"),
                [],
                |row| row.get(0),
            )
            .map_err(|e| tx_err("计算层级清理深度失败", e))?;
        if let Some(max_depth) = max_depth {
            let mut depth = 0;
            while depth <= max_depth {
                // 只有本轮确实可删的叶节点才允许先清用户状态。候选父条目若仍有
                // 另一个非候选/共享子节点，必须连同 progress/history/marker/favorite
                // 一起保留，不能因为它出现在祖先闭包里就提前丢用户数据。
                exec(&format!("DELETE FROM {purge_temp} WHERE stage = 4"))?;
                let mark_deletable = format!(
                    "INSERT INTO {purge_temp}(stage, id, parent, depth)
                     SELECT DISTINCT 4, m.id, m.edition_id, {depth}
                     FROM media_items m
                     JOIN {purge_temp} p ON p.stage = 1 AND p.id = m.id AND p.depth = {depth}
                     WHERE NOT EXISTS (SELECT 1 FROM resources r WHERE r.media_item_id = m.id)
                       AND NOT EXISTS (SELECT 1 FROM media_items child WHERE child.parent_id = m.id)"
                );
                exec(&mark_deletable)?;
                exec(&format!(
                    "DELETE FROM history_entries WHERE media_item_id IN (SELECT id FROM {purge_temp} WHERE stage = 4)"
                ))?;
                exec(&format!(
                    "DELETE FROM progress WHERE media_item_id IN (SELECT id FROM {purge_temp} WHERE stage = 4)"
                ))?;
                exec(&format!(
                    "DELETE FROM markers WHERE media_item_id IN (SELECT id FROM {purge_temp} WHERE stage = 4)"
                ))?;
                exec(&format!(
                    "DELETE FROM favorites WHERE media_item_id IN (SELECT id FROM {purge_temp} WHERE stage = 4)"
                ))?;
                exec(&format!(
                    "DELETE FROM download_tasks WHERE media_item_id IN (SELECT id FROM {purge_temp} WHERE stage = 4)"
                ))?;
                exec(&format!(
                    "DELETE FROM media_items WHERE id IN (SELECT id FROM {purge_temp} WHERE stage = 4)"
                ))?;
                depth += 1;
            }
        }

        // stage 2：孤儿 edition（media_item 已删，NOT EXISTS 即孤儿；共享 edition 保留）。
        exec(&format!(
            "INSERT INTO {purge_temp}(stage, id, parent, depth)
             SELECT DISTINCT 2, e.id, e.work_id, 0 FROM editions e
             WHERE e.id IN (SELECT parent FROM {purge_temp} WHERE stage = 1 AND parent IS NOT NULL)
               AND NOT EXISTS (SELECT 1 FROM media_items m WHERE m.edition_id = e.id)"
        ))?;
        exec(&format!(
            "DELETE FROM favorites WHERE edition_id IN (SELECT id FROM {purge_temp} WHERE stage = 2)"
        ))?;
        exec(&format!(
            "DELETE FROM download_tasks WHERE edition_id IN (SELECT id FROM {purge_temp} WHERE stage = 2)"
        ))?;
        exec(&format!(
            "DELETE FROM editions WHERE id IN (SELECT id FROM {purge_temp} WHERE stage = 2)"
        ))?;

        // stage 3：孤儿 work（edition 已删；共享 work 保留）。
        exec(&format!(
            "INSERT INTO {purge_temp}(stage, id, parent, depth)
             SELECT DISTINCT 3, w.id, NULL, 0 FROM works w
             WHERE w.id IN (SELECT parent FROM {purge_temp} WHERE stage = 2 AND parent IS NOT NULL)
               AND NOT EXISTS (SELECT 1 FROM editions e WHERE e.work_id = w.id)"
        ))?;
        exec(&format!(
            "DELETE FROM favorites WHERE work_id IN (SELECT id FROM {purge_temp} WHERE stage = 3)"
        ))?;
        exec(&format!(
            "DELETE FROM download_tasks WHERE work_id IN (SELECT id FROM {purge_temp} WHERE stage = 3)"
        ))?;
        // work_favorite_versions 随 works 外键 CASCADE。
        exec(&format!(
            "DELETE FROM works WHERE id IN (SELECT id FROM {purge_temp} WHERE stage = 3)"
        ))?;
        // 成功收尾：必须 DROP，不只 DELETE（释放连接级 TEMP 表，避免跨事务残留）。
        exec("DROP TABLE IF EXISTS temp._haven_storage_purge_media_ids")?;
        Ok(())
    }

    fn delete_location(&self, id: haven_domain::ids::StorageLocationId) -> Result<bool, AppError> {
        let affected = self
            .tx
            .execute(
                "DELETE FROM storage_locations WHERE id = ?1",
                rusqlite::params![id.to_string()],
            )
            .map_err(|e| tx_err("删除存储位置失败", e))?;
        Ok(affected > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repos::SqliteRepositories;
    use haven_application::services::favorite::FavoriteService;
    use haven_application::services::storage_location::StorageLocationService;

    /// 插入合法 works 行（与 repos/mod.rs 既有测试同格式）。
    fn insert_work(db: &Arc<Db>) -> haven_domain::ids::WorkId {
        use haven_domain::ids::WorkId;
        let work_id = WorkId::new();
        let now = haven_common::UtcMillis::now().0;
        db.lock()
            .execute(
                "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
                 VALUES (?1, '回滚测试', 'fiction', 'completed', ?2, ?2)",
                rusqlite::params![work_id.to_string(), now],
            )
            .unwrap();
        work_id
    }

    /// 阻塞 A：真实 SQLite Favorite UoW 写失败回滚证据——`apply_favorite` 先写 favorites
    /// 再写 work_favorite_versions；版本写入被 trigger 拒绝时，**整个事务必须回滚**：
    /// favorites 与 work_favorite_versions 均为 0，works 仍为 1。
    /// 使用真实 SqliteRepositories + SqliteUnitOfWork + FavoriteService（非手写 tx / mock）。
    #[tokio::test]
    async fn favorite_version_write_failure_rolls_back_whole_txn() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let work_id = insert_work(&db);

        // 注入确定性失败：任何对 work_favorite_versions 的 INSERT 都被 trigger 拒绝。
        db.lock()
            .execute_batch(
                "CREATE TRIGGER fail_favorite_version_insert
                 BEFORE INSERT ON work_favorite_versions
                 BEGIN
                     SELECT RAISE(ABORT, 'injected favorite version failure');
                 END;",
            )
            .unwrap();

        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        let svc = FavoriteService::new(repos, Arc::new(SqliteUnitOfWork::new(db.clone())));

        let err = svc.set_with_outcome(work_id, true).await.unwrap_err();
        assert_eq!(
            err.code().as_str(),
            "DATABASE_ERROR",
            "版本写失败必须映射为 DATABASE_ERROR"
        );

        let count = |table: &str| -> i64 {
            db.lock()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(count("favorites"), 0, "favorites 已写但必须随事务回滚");
        assert_eq!(
            count("work_favorite_versions"),
            0,
            "版本写入失败，行不得残留"
        );
        assert_eq!(count("works"), 1, "works 不受影响");
    }

    /// 以 Repository 的正式写入路径建立一条仅属于指定位置的内容链。失败注入测试
    /// 放在此内部模块，才能在不扩大 `Db::lock` 产品可见性的前提下安装 SQLite trigger。
    async fn seed_location_content(
        repos: &SqliteRepositories,
        storage_location_id: haven_domain::ids::StorageLocationId,
    ) -> (
        haven_domain::ids::WorkId,
        haven_domain::ids::EditionId,
        haven_domain::ids::MediaItemId,
    ) {
        use haven_domain::contracts::{
            EditionRepository, MediaItemRepository, ResourceRepository, WorkRepository,
        };
        use haven_domain::entities::{Edition, MediaItem, Resource, ResourceLocator, Work};
        use haven_domain::enums::{
            Availability, AvailabilitySource, MediaItemStatus, MediaType, ResourceType, WorkStatus,
            WorkType,
        };

        let work_id = haven_domain::ids::WorkId::new();
        let edition_id = haven_domain::ids::EditionId::new();
        let media_item_id = haven_domain::ids::MediaItemId::new();
        let now = haven_common::UtcMillis::now();
        repos
            .work
            .save(&Work {
                id: work_id,
                canonical_title: "purge 回滚测试".into(),
                original_title: None,
                sort_title: None,
                description: None,
                work_type: WorkType::Standalone,
                release_year: None,
                language: None,
                director: None,
                actor: None,
                status: WorkStatus::Unknown,
                rating_value: None,
                rating_scale: None,
                artwork: Default::default(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        repos
            .edition
            .save(&Edition {
                id: edition_id,
                work_id,
                title: "purge 回滚版本".into(),
                subtitle: None,
                edition_type: MediaType::Movie,
                release_date: None,
                language: None,
                region: None,
                publisher_or_studio: None,
                description: None,
                artwork: Default::default(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        repos
            .media_item
            .save(&MediaItem {
                id: media_item_id,
                edition_id,
                parent_id: None,
                media_type: MediaType::Movie,
                title: "purge 回滚条目".into(),
                index: haven_domain::entities::MediaIndex::Movie,
                duration_ms: None,
                page_count: None,
                chapter_count: None,
                published_at: None,
                status: MediaItemStatus::Available,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        repos
            .resource
            .save(&Resource {
                id: haven_domain::ids::ResourceId::new(),
                media_item_id,
                resource_type: ResourceType::LocalFile,
                source_id: None,
                storage_location_id: Some(storage_location_id),
                locator: ResourceLocator::LocalPath {
                    path: "D:\\purge-test\\item.mkv".into(),
                },
                mime_type: None,
                size: None,
                hash: None,
                availability: Availability::Available,
                availability_source: AvailabilitySource::Unknown,
                modified_ms: None,
                fingerprint_first: None,
                fingerprint_last: None,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        (work_id, edition_id, media_item_id)
    }

    /// 构造 work + edition（复用；供层级/顺序测试）。
    async fn seed_work_and_edition(
        repos: &SqliteRepositories,
    ) -> (haven_domain::ids::WorkId, haven_domain::ids::EditionId) {
        use haven_domain::contracts::{EditionRepository, WorkRepository};
        use haven_domain::entities::{Edition, Work};
        use haven_domain::enums::{MediaType, WorkStatus, WorkType};

        let work_id = haven_domain::ids::WorkId::new();
        let edition_id = haven_domain::ids::EditionId::new();
        let now = haven_common::UtcMillis::now();
        repos
            .work
            .save(&Work {
                id: work_id,
                canonical_title: "层级作品".into(),
                original_title: None,
                sort_title: None,
                description: None,
                work_type: WorkType::Standalone,
                release_year: None,
                language: None,
                director: None,
                actor: None,
                status: WorkStatus::Unknown,
                rating_value: None,
                rating_scale: None,
                artwork: Default::default(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        repos
            .edition
            .save(&Edition {
                id: edition_id,
                work_id,
                title: "层级版本".into(),
                subtitle: None,
                edition_type: MediaType::Movie,
                release_date: None,
                language: None,
                region: None,
                publisher_or_studio: None,
                description: None,
                artwork: Default::default(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        (work_id, edition_id)
    }

    /// 构造一条 `parent_id` 链：返回 `chain[0]`=最深层子、`chain[depth]`=根；
    /// 每个 `chain[i]` 的 parent 为 `chain[i+1]`（`chain[depth]` parent=None）。
    /// `depth=0` 表示单层。供 child-first 顺序/层级测试复用。
    async fn seed_media_chain(
        repos: &SqliteRepositories,
        edition_id: haven_domain::ids::EditionId,
        depth: usize,
    ) -> Vec<haven_domain::ids::MediaItemId> {
        use haven_domain::contracts::MediaItemRepository;
        use haven_domain::entities::{MediaIndex, MediaItem};
        use haven_domain::enums::{MediaItemStatus, MediaType};

        let mut ids = Vec::new();
        for _ in 0..=depth {
            ids.push(haven_domain::ids::MediaItemId::new());
        }
        let now = haven_common::UtcMillis::now();
        // 从根插入（parent 先存在），再逐层向下，避免 parent_id FK 指向未插入行。
        for i in (0..=depth).rev() {
            let id = ids[i];
            let parent = if i < depth { Some(ids[i + 1]) } else { None };
            repos
                .media_item
                .save(&MediaItem {
                    id,
                    edition_id,
                    parent_id: parent,
                    media_type: MediaType::Episode,
                    title: format!("层级条目{depth}-{i}"),
                    index: MediaIndex::Episode {
                        season: None,
                        episode: 1,
                    },
                    duration_ms: None,
                    page_count: None,
                    chapter_count: None,
                    published_at: None,
                    status: MediaItemStatus::Available,
                    created_at: now,
                    updated_at: now,
                })
                .await
                .unwrap();
        }
        ids
    }

    /// 给 media_item 挂一个位置资源（复用）。
    async fn attach_resource(
        repos: &SqliteRepositories,
        media_item_id: haven_domain::ids::MediaItemId,
        storage_location_id: haven_domain::ids::StorageLocationId,
    ) {
        use haven_domain::contracts::ResourceRepository;
        use haven_domain::entities::{Resource, ResourceLocator};
        use haven_domain::enums::{Availability, AvailabilitySource, ResourceType};

        let now = haven_common::UtcMillis::now();
        repos
            .resource
            .save(&Resource {
                id: haven_domain::ids::ResourceId::new(),
                media_item_id,
                resource_type: ResourceType::LocalFile,
                source_id: None,
                storage_location_id: Some(storage_location_id),
                locator: ResourceLocator::LocalPath {
                    path: "series/child.mkv".into(),
                },
                mime_type: None,
                size: None,
                hash: None,
                availability: Availability::Available,
                availability_source: AvailabilitySource::Unknown,
                modified_ms: None,
                fingerprint_first: None,
                fingerprint_last: None,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
    }

    /// R-STORAGE-PURGE-03（实战回归）：位置下资源被 download_tasks 引用
    /// （source_resource_id RESTRICT）时，remove 曾因外键失败报"删除位置索引资源失败"。
    /// 现契约：指向该位置资源的下载任务随位置一并清理；其他位置的任务保留。
    #[tokio::test]
    async fn storage_remove_purges_download_tasks_referencing_removed_resources() {
        use haven_domain::contracts::ResourceRepository;
        use haven_domain::entities::{Resource, ResourceLocator};
        use haven_domain::enums::{Availability, AvailabilitySource, ResourceType};

        let dir = tempfile::TempDir::new().unwrap();
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = SqliteRepositories::new(db.clone());
        let storage = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
        let location_id = storage
            .add_local("下载源库".into(), dir.path())
            .await
            .unwrap();
        let (_work_id, edition_id) = seed_work_and_edition(&repos).await;
        let media_item_id = seed_media_chain(&repos, edition_id, 0).await.remove(0);

        let now = haven_common::UtcMillis::now();
        let resource_id = haven_domain::ids::ResourceId::new();
        repos
            .resource
            .save(&Resource {
                id: resource_id,
                media_item_id,
                resource_type: ResourceType::LocalFile,
                source_id: None,
                storage_location_id: Some(location_id),
                locator: ResourceLocator::LocalPath {
                    path: "a.mkv".into(),
                },
                mime_type: None,
                size: None,
                hash: None,
                availability: Availability::Available,
                availability_source: AvailabilitySource::Unknown,
                modified_ms: None,
                fingerprint_first: None,
                fingerprint_last: None,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();

        // 直接 SQL 建下载任务（DownloadRepository 端口不含裸建任务入口）。
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO download_tasks (id, work_id, edition_id, media_item_id, source_resource_id, target_storage_id, state, created_at, updated_at)
                 VALUES ('dt-1', ?1, ?2, ?3, ?4, ?5, 'completed', ?6, ?6)",
                rusqlite::params![
                    _work_id.to_string(),
                    edition_id.to_string(),
                    media_item_id.to_string(),
                    resource_id.to_string(),
                    location_id.to_string(),
                    now.0,
                ],
            )
            .unwrap();
        }

        // 修复前：此处因 FK RESTRICT 失败（"删除位置索引资源失败"）。
        storage.remove(location_id).await.unwrap();

        let tasks: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM download_tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tasks, 0, "引用被移除位置资源的下载任务必须一并清理");
        for table in [
            "resources",
            "media_items",
            "editions",
            "works",
            "storage_locations",
        ] {
            let count: i64 = db
                .lock()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "{table} 不得残留");
        }
    }
    /// 安装 TEMP 顺序日志（记录 media_items 实际 DELETE 顺序；temp schema 与日志表同域）。
    fn install_purge_order_log(db: &Db) {
        let conn = db.lock();
        conn.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS _purge_order(seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT);
             DROP TRIGGER IF EXISTS purge_order_log;
             CREATE TEMP TRIGGER purge_order_log BEFORE DELETE ON media_items
             BEGIN
                 INSERT INTO _purge_order(id) VALUES (OLD.id);
             END;",
        )
        .unwrap();
    }

    /// 读取并按删除顺序返回 media_items 的 id 列表（随后清理 instrumentation）。
    fn read_purge_order(db: &Db) -> Vec<String> {
        let conn = db.lock();
        let mut stmt = conn
            .prepare("SELECT id FROM _purge_order ORDER BY seq")
            .unwrap();
        let out: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        conn.execute_batch(
            "DROP TRIGGER IF EXISTS purge_order_log;
             DROP TABLE IF EXISTS _purge_order;",
        )
        .unwrap();
        out
    }

    /// purge TEMP 中间表是否残留在连接级 temp schema。
    fn purge_temp_leaked(db: &Db) -> bool {
        let leaked: i64 = db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM temp.sqlite_temp_master WHERE name = '_haven_storage_purge_media_ids'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        leaked > 0
    }

    fn table_count(db: &Db, table: &str) -> i64 {
        db.lock()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    /// 预置一个"旧" purge TEMP 表（不同 schema + 一行），模拟事务开始前已存在的残留，
    /// 用于证明事务外预清理能清走、事务失败回滚不会复活旧表。
    fn seed_old_purge_temp(db: &Db) {
        let conn = db.lock();
        conn.execute_batch(
            "DROP TABLE IF EXISTS temp._haven_storage_purge_media_ids;
             CREATE TEMP TABLE _haven_storage_purge_media_ids(old_marker TEXT);
             INSERT INTO _haven_storage_purge_media_ids(old_marker) VALUES ('old-row');",
        )
        .unwrap();
    }

    /// `purge_location_content` 的中段失败注入：stage 1/2 的资源、用户状态与内容
    /// 已实际尝试删除后，在 `editions` 删除处强制失败。必须回滚整个 remove 事务，
    /// 随后移除 trigger 的重试必须完整成功。
    #[tokio::test]
    async fn storage_remove_rolls_back_all_purge_stages_after_mid_transaction_failure() {
        use haven_domain::contracts::FavoriteRepository;
        use haven_domain::entities::FavoriteTarget;
        use rusqlite::params;

        let dir = tempfile::TempDir::new().unwrap();
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        let storage = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
        let location_id = storage.add_local("库".into(), dir.path()).await.unwrap();
        let (work_id, edition_id, media_item_id) = seed_location_content(&repos, location_id).await;

        // Work 收藏经正式 FavoriteService 产生版本行；edition/media 收藏保证 stage 1/2
        // 的 `DELETE FROM favorites` 在 trigger 触发前都已走过真实 SQL。
        let favorite_service =
            FavoriteService::new(repos.clone(), Arc::new(SqliteUnitOfWork::new(db.clone())));
        favorite_service
            .set_with_outcome(work_id, true)
            .await
            .unwrap();
        repos
            .favorite
            .set(&FavoriteTarget::Edition(edition_id))
            .await
            .unwrap();
        repos
            .favorite
            .set(&FavoriteTarget::MediaItem(media_item_id))
            .await
            .unwrap();

        let now = haven_common::UtcMillis::now().0;
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO progress (id, work_id, edition_id, media_item_id, locator_json, locator_version, completion, percentage, last_active_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, 'in_progress', 0.25, ?6, ?6)",
                params![
                    "purge-progress",
                    work_id.to_string(),
                    edition_id.to_string(),
                    media_item_id.to_string(),
                    r#"{\"kind\":\"local_path\",\"path\":\"test.mkv\"}"#,
                    now,
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO markers (id, work_id, edition_id, media_item_id, locator_json, marker_type, title, excerpt, note, preview, created_at, updated_at, deleted_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'bookmark', NULL, NULL, NULL, NULL, ?6, ?6, NULL)",
                params![
                    "purge-marker",
                    work_id.to_string(),
                    edition_id.to_string(),
                    media_item_id.to_string(),
                    r#"{\"kind\":\"local_path\",\"path\":\"test.mkv\"}"#,
                    now,
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO history_entries (id, media_item_id, work_id, edition_id, locator_json, started_at, last_active_at, completed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, NULL)",
                params![
                    "purge-history",
                    media_item_id.to_string(),
                    work_id.to_string(),
                    edition_id.to_string(),
                    r#"{\"kind\":\"local_path\",\"path\":\"test.mkv\"}"#,
                    now,
                ],
            )
            .unwrap();
            conn.execute_batch(
                "CREATE TRIGGER fail_purge_editions
                 BEFORE DELETE ON editions
                 BEGIN
                     SELECT RAISE(ABORT, 'injected purge failure');
                 END;",
            )
            .unwrap();
        }

        let err = storage.remove(location_id).await.unwrap_err();
        assert_eq!(err.code().as_str(), "DATABASE_ERROR");

        let count = |table: &str| -> i64 {
            db.lock()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap()
        };
        for (table, expected) in [
            ("storage_locations", 1),
            ("resources", 1),
            ("media_items", 1),
            ("editions", 1),
            ("works", 1),
            ("progress", 1),
            ("markers", 1),
            ("history_entries", 1),
            ("favorites", 3),
            ("work_favorite_versions", 1),
        ] {
            assert_eq!(count(table), expected, "{table} 必须在失败后完整回滚");
        }
        {
            let conn = db.lock();
            let mut statement = conn.prepare("PRAGMA foreign_key_check").unwrap();
            let mut rows = statement.query([]).unwrap();
            assert!(rows.next().unwrap().is_none(), "回滚后不得有 FK 损坏");
            let integrity: String = conn
                .query_row("PRAGMA integrity_check", [], |row| row.get(0))
                .unwrap();
            assert_eq!(integrity, "ok");
            conn.execute_batch("DROP TRIGGER fail_purge_editions")
                .unwrap();
        }

        storage.remove(location_id).await.unwrap();
        for table in [
            "storage_locations",
            "resources",
            "media_items",
            "editions",
            "works",
            "progress",
            "markers",
            "history_entries",
            "favorites",
            "work_favorite_versions",
        ] {
            assert_eq!(count(table), 0, "{table} 必须在重试后清空");
        }
    }

    /// R-STORAGE-PURGE-02：三层 grandparent→parent→child，资源只挂最深层 child；
    /// remove 后 resources/media_items/editions/works 全清，且删除顺序必须 child-first
    /// （depth 0→1→2），经 `BEFORE DELETE ON media_items` 顺序日志明确验证。
    #[tokio::test]
    async fn storage_remove_purges_child_then_parent_media_items() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = SqliteRepositories::new(db.clone());
        let storage = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
        let location_id = storage
            .add_local("层级库".into(), dir.path())
            .await
            .unwrap();
        let (_work_id, edition_id) = seed_work_and_edition(&repos).await;

        // 三层链：chain[0]=child（最深）、chain[1]=parent、chain[2]=grandparent（根）。
        let chain = seed_media_chain(&repos, edition_id, 2).await;
        assert_eq!(chain.len(), 3);
        attach_resource(&repos, chain[0], location_id).await;

        // 记录 media_items 实际删除顺序（连接级临时表 + **TEMP trigger**：trigger 与
        // _purge_order 同属 temp schema，避免普通 main trigger 无法访问连接级 TEMP 表）。
        {
            let conn = db.lock();
            conn.execute_batch(
                "CREATE TEMP TABLE _purge_order(seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT);
                 CREATE TEMP TRIGGER purge_order_log BEFORE DELETE ON media_items
                 BEGIN
                     INSERT INTO _purge_order(id) VALUES (OLD.id);
                 END;",
            )
            .unwrap();
        }

        storage.remove(location_id).await.unwrap();

        // 删除顺序：depth0(child)→depth1(parent)→depth2(grandparent)。
        {
            let conn = db.lock();
            let mut stmt = conn
                .prepare("SELECT id FROM _purge_order ORDER BY seq")
                .unwrap();
            let deleted: Vec<String> = stmt
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(
                deleted,
                vec![
                    chain[0].to_string(),
                    chain[1].to_string(),
                    chain[2].to_string(),
                ],
                "purge 必须 child-first：child→parent→grandparent"
            );
        }

        // 清理 instrumentation。
        {
            let conn = db.lock();
            conn.execute_batch(
                "DROP TRIGGER IF EXISTS purge_order_log;
                 DROP TABLE IF EXISTS _purge_order;",
            )
            .unwrap();
        }

        for table in ["resources", "media_items", "editions", "works"] {
            let count: i64 = db
                .lock()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "层级清理后 {table} 不得残留");
        }
    }

    /// R-STORAGE-PURGE-02：最终阶段回滚——purge 全阶段已执行后，`storage_locations`
    /// 的 `BEFORE DELETE` trigger 强制失败，`delete_location` 失败 → 整个 remove 事务回滚：
    /// storage_locations/resources/media_items/editions/works 仍各 1；foreign_key_check 无行、
    /// integrity_check=ok；移除 trigger 后重试成功并全清。
    #[tokio::test]
    async fn storage_remove_rolls_back_when_final_delete_location_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        let storage = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
        let location_id = storage
            .add_local("回滚库".into(), dir.path())
            .await
            .unwrap();
        seed_location_content(&repos, location_id).await;

        {
            let conn = db.lock();
            conn.execute_batch(
                "CREATE TRIGGER fail_delete_location
                 BEFORE DELETE ON storage_locations
                 BEGIN
                     SELECT RAISE(ABORT, 'injected delete-location failure');
                 END;",
            )
            .unwrap();
        }

        let err = storage.remove(location_id).await.unwrap_err();
        assert_eq!(err.code().as_str(), "DATABASE_ERROR");

        let count = |table: &str| -> i64 {
            db.lock()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap()
        };
        for table in [
            "storage_locations",
            "resources",
            "media_items",
            "editions",
            "works",
        ] {
            assert_eq!(count(table), 1, "{table} 必须在最终阶段失败后完整回滚");
        }
        {
            let conn = db.lock();
            let mut statement = conn.prepare("PRAGMA foreign_key_check").unwrap();
            let mut rows = statement.query([]).unwrap();
            assert!(rows.next().unwrap().is_none(), "回滚后不得有 FK 损坏");
            let integrity: String = conn
                .query_row("PRAGMA integrity_check", [], |row| row.get(0))
                .unwrap();
            assert_eq!(integrity, "ok");
            conn.execute_batch("DROP TRIGGER fail_delete_location")
                .unwrap();
        }

        storage.remove(location_id).await.unwrap();
        for table in [
            "storage_locations",
            "resources",
            "media_items",
            "editions",
            "works",
        ] {
            assert_eq!(count(table), 0, "{table} 必须在重试后清空");
        }
    }

    /// R-STORAGE-PURGE-02：无资源的 location remove 成功，不误删无关 work/content；
    /// 原始文件不被 remove 删除（sentinel 保留）。
    #[tokio::test]
    async fn storage_remove_empty_location_keeps_unrelated_content_and_files() {
        use haven_domain::contracts::WorkRepository;
        use haven_domain::entities::Work;
        use haven_domain::enums::{WorkStatus, WorkType};

        let media_dir = tempfile::TempDir::new().unwrap();
        let sentinel = media_dir.path().join("movie.mkv");
        std::fs::write(&sentinel, b"original-bytes").unwrap();

        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        let storage = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
        let location_id = storage
            .add_local("空库".into(), media_dir.path())
            .await
            .unwrap();

        // 无关 work（不挂该位置资源）。
        let now = haven_common::UtcMillis::now();
        let unrelated = haven_domain::ids::WorkId::new();
        repos
            .work
            .save(&Work {
                id: unrelated,
                canonical_title: "无关作品".into(),
                original_title: None,
                sort_title: None,
                description: None,
                work_type: WorkType::Standalone,
                release_year: None,
                language: None,
                director: None,
                actor: None,
                status: WorkStatus::Unknown,
                rating_value: None,
                rating_scale: None,
                artwork: Default::default(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();

        storage.remove(location_id).await.unwrap();

        let locations: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM storage_locations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(locations, 0, "空位置 remove 后位置删除");
        let works = repos.work.list(10, 0).await.unwrap();
        assert_eq!(works.len(), 1, "无资源位置 remove 不得误删无关 work");
        assert_eq!(works[0].id, unrelated);
        assert!(
            sentinel.exists(),
            "remove 绝对不得删除用户原始媒体文件（sentinel 必须保留）"
        );
    }

    #[tokio::test]
    async fn storage_remove_preserves_shared_parent_and_its_user_state() {
        let removed_dir = tempfile::TempDir::new().unwrap();
        let retained_dir = tempfile::TempDir::new().unwrap();
        let db = Arc::new(Db::open_in_memory().unwrap());
        let storage = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
        let removed_location = storage
            .add_local("待移除层级库".into(), removed_dir.path())
            .await
            .unwrap();
        let retained_location = storage
            .add_local("共享层级库".into(), retained_dir.path())
            .await
            .unwrap();

        let work_id = haven_domain::ids::WorkId::new().to_string();
        let edition_id = haven_domain::ids::EditionId::new().to_string();
        let parent_id = haven_domain::ids::MediaItemId::new().to_string();
        let removed_child_id = haven_domain::ids::MediaItemId::new().to_string();
        let retained_child_id = haven_domain::ids::MediaItemId::new().to_string();
        let now = haven_common::UtcMillis::now().0;
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
                 VALUES (?1, '共享父条目', 'fiction', 'unknown', ?2, ?2)",
                rusqlite::params![&work_id, now],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
                 VALUES (?1, ?2, '共享版本', 'series', ?3, ?3)",
                rusqlite::params![&edition_id, &work_id, now],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO media_items
                    (id, edition_id, parent_id, media_type, title, category, status, created_at, updated_at)
                 VALUES (?1, ?2, NULL, 'series', '父条目', 'video', 'available', ?3, ?3)",
                rusqlite::params![&parent_id, &edition_id, now],
            )
            .unwrap();
            for (id, title) in [
                (&removed_child_id, "待移除子条目"),
                (&retained_child_id, "保留子条目"),
            ] {
                conn.execute(
                    "INSERT INTO media_items
                        (id, edition_id, parent_id, media_type, title, category, status, created_at, updated_at)
                     VALUES (?1, ?2, ?3, 'episode', ?4, 'video', 'available', ?5, ?5)",
                    rusqlite::params![id, &edition_id, &parent_id, title, now],
                )
                .unwrap();
            }
            for (item_id, location_id, path) in [
                (&removed_child_id, removed_location, "removed/episode.mkv"),
                (
                    &retained_child_id,
                    retained_location,
                    "retained/episode.mkv",
                ),
            ] {
                conn.execute(
                    "INSERT INTO resources
                        (id, media_item_id, resource_type, storage_location_id, locator_kind,
                         locator_json, availability, created_at, updated_at)
                     VALUES (?1, ?2, 'local_file', ?3, 'local_path', ?4, 'available', ?5, ?5)",
                    rusqlite::params![
                        haven_domain::ids::ResourceId::new().to_string(),
                        item_id,
                        location_id.to_string(),
                        format!(r#"{{"kind":"local_path","path":"{path}"}}"#),
                        now,
                    ],
                )
                .unwrap();
            }

            conn.execute(
                "INSERT INTO favorites (media_item_id, created_at) VALUES (?1, ?2)",
                rusqlite::params![&parent_id, now],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO progress
                    (id, work_id, edition_id, media_item_id, locator_json, locator_version,
                     completion, percentage, last_active_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, '{}', 1, 'in_progress', 0.5, ?5, ?5)",
                rusqlite::params![
                    haven_domain::ids::ProgressId::new().to_string(),
                    &work_id,
                    &edition_id,
                    &parent_id,
                    now,
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO markers
                    (id, work_id, edition_id, media_item_id, locator_json, marker_type, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, '{}', 'bookmark', ?5, ?5)",
                rusqlite::params![
                    haven_domain::ids::MarkerId::new().to_string(),
                    &work_id,
                    &edition_id,
                    &parent_id,
                    now,
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO history_entries
                    (id, media_item_id, work_id, edition_id, locator_json, started_at, last_active_at)
                 VALUES (?1, ?2, ?3, ?4, '{}', ?5, ?5)",
                rusqlite::params![
                    haven_domain::ids::HistoryEntryId::new().to_string(),
                    &parent_id,
                    &work_id,
                    &edition_id,
                    now,
                ],
            )
            .unwrap();
        }

        storage.remove(removed_location).await.unwrap();

        let conn = db.lock();
        let exists = |table: &str, column: &str, id: &str| -> i64 {
            conn.query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?1"),
                rusqlite::params![id],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(exists("media_items", "id", &removed_child_id), 0);
        assert_eq!(exists("media_items", "id", &parent_id), 1);
        assert_eq!(exists("media_items", "id", &retained_child_id), 1);
        assert_eq!(
            exists(
                "resources",
                "storage_location_id",
                &retained_location.to_string()
            ),
            1
        );
        for table in ["favorites", "progress", "markers", "history_entries"] {
            assert_eq!(
                exists(table, "media_item_id", &parent_id),
                1,
                "共享父条目保留时不得提前删除 {table}"
            );
        }
        assert_eq!(exists("works", "id", &work_id), 1);
        assert_eq!(exists("editions", "id", &edition_id), 1);
    }

    /// R-MAIN-09C Important 1：purge TEMP 中间表（`temp._haven_storage_purge_media_ids`）
    /// 在成功路径**必须 DROP**、在任一失败回滚路径**不得泄漏**；失败后原业务数据
    /// 全回滚、移除 trigger 重试成功且成功后同样无泄漏。
    /// R-MAIN-09D 强化：两条路径都在 remove **之前**预置一个同名**旧** TEMP 表
    /// （不同 schema + 一行数据）——事务外预清理必须把它清走，事务失败回滚不会复活旧表。
    #[tokio::test]
    async fn purge_temp_table_no_leak_on_success_and_failure() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        let storage = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));

        // 成功路径：起点已有旧 TEMP 表（old_marker 列 + 一行）→ remove 前被清走，
        // remove 成功、成功后无任何 TEMP 残留。
        let location_a = storage.add_local("A".into(), dir.path()).await.unwrap();
        seed_location_content(&repos, location_a).await;
        seed_old_purge_temp(&db);
        storage.remove(location_a).await.unwrap();
        assert!(
            !purge_temp_leaked(&db),
            "成功 remove 后 TEMP 中间表必须 DROP（旧表也不得残留）"
        );

        // 失败路径：起点同样预置旧 TEMP 表 + storage_locations trigger 使最终删除失败。
        let location_b = storage.add_local("B".into(), dir.path()).await.unwrap();
        seed_location_content(&repos, location_b).await;
        {
            let conn = db.lock();
            conn.execute_batch(
                "CREATE TRIGGER fail_delete_location_2
                 BEFORE DELETE ON storage_locations
                 BEGIN
                     SELECT RAISE(ABORT, 'injected delete-location failure');
                 END;",
            )
            .unwrap();
        }
        seed_old_purge_temp(&db);
        let err = storage.remove(location_b).await.unwrap_err();
        assert_eq!(err.code().as_str(), "DATABASE_ERROR");
        assert!(
            !purge_temp_leaked(&db),
            "失败回滚后 TEMP 中间表不得泄漏（预置旧表也不得复活）"
        );
        for table in [
            "storage_locations",
            "resources",
            "media_items",
            "editions",
            "works",
        ] {
            assert_eq!(table_count(&db, table), 1, "{table} 必须完整回滚");
        }

        // 移除 trigger 重试：成功且成功后无泄漏。
        db.lock()
            .execute_batch("DROP TRIGGER fail_delete_location_2")
            .unwrap();
        storage.remove(location_b).await.unwrap();
        for table in [
            "storage_locations",
            "resources",
            "media_items",
            "editions",
            "works",
        ] {
            assert_eq!(table_count(&db, table), 0, "{table} 重试后必须清空");
        }
        assert!(!purge_temp_leaked(&db), "重试成功后 TEMP 中间表不得泄漏");
    }

    /// R-MAIN-09C Important 2：parent_id **成环**时 purge 必须终止（path guard）、
    /// remove 按明确安全语义成功（位置/资源删除），**绝不误删环内节点/用户状态**，
    /// FK/integrity 通过且 TEMP 表不泄漏。
    #[tokio::test]
    async fn purge_terminates_on_parent_cycle_and_preserves_nodes() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = SqliteRepositories::new(db.clone());
        let storage = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
        let location_id = storage.add_local("环库".into(), dir.path()).await.unwrap();
        let (_work_id, edition_id) = seed_work_and_edition(&repos).await;

        // 链 child→parent→root，然后 UPDATE root.parent_id = child 形成环
        // child→parent→root→child。
        let chain = seed_media_chain(&repos, edition_id, 2).await;
        let (child, parent, root) = (chain[0], chain[1], chain[2]);
        {
            let conn = db.lock();
            conn.execute(
                "UPDATE media_items SET parent_id = ?1 WHERE id = ?2",
                rusqlite::params![child.to_string(), root.to_string()],
            )
            .unwrap();
        }
        attach_resource(&repos, child, location_id).await;

        // remove 必须成功（位置 + 资源删除），环内节点全部保留。
        storage.remove(location_id).await.unwrap();
        assert_eq!(table_count(&db, "storage_locations"), 0);
        assert_eq!(table_count(&db, "resources"), 0);
        for (id, label) in [(child, "child"), (parent, "parent"), (root, "root")] {
            let n: i64 = db
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM media_items WHERE id = ?1",
                    rusqlite::params![id.to_string()],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "环内 {label} 节点必须保留（不得误删）");
        }
        assert_eq!(table_count(&db, "editions"), 1, "edition 必须保留");
        assert_eq!(table_count(&db, "works"), 1, "work 必须保留");

        // FK / integrity / TEMP 不泄漏。
        {
            let conn = db.lock();
            let mut stmt = conn.prepare("PRAGMA foreign_key_check").unwrap();
            let mut rows = stmt.query([]).unwrap();
            assert!(rows.next().unwrap().is_none(), "环保留后不得有 FK 损坏");
            let integrity: String = conn
                .query_row("PRAGMA integrity_check", [], |row| row.get(0))
                .unwrap();
            assert_eq!(integrity, "ok");
        }
        assert!(!purge_temp_leaked(&db), "环路径不得泄漏 TEMP 中间表");
    }

    /// R-MAIN-09C Important 2：复杂图——direct child 与更深 grandchild 链汇聚同一祖先，
    /// 祖先经不同 depth 到达；验证 MAX(depth)/child-first 顺序与独占节点正确删除（全清）。
    #[tokio::test]
    async fn purge_multi_depth_convergence_clears_exclusive_nodes() {
        use haven_domain::contracts::MediaItemRepository;

        let dir = tempfile::TempDir::new().unwrap();
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = SqliteRepositories::new(db.clone());
        let storage = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
        let location_id = storage
            .add_local("汇聚焦".into(), dir.path())
            .await
            .unwrap();
        let (_work_id, edition_id) = seed_work_and_edition(&repos).await;

        // 图：root；D(direct, parent=root)；P(parent=root)；G(parent=P)。
        let d = haven_domain::ids::MediaItemId::new();
        // 用 seed_media_chain 建 G→P→root 链（chain=[g,p,root]），再单独建 D(parent=root)。
        let chain = seed_media_chain(&repos, edition_id, 2).await;
        let (g, p, root) = (chain[0], chain[1], chain[2]);
        let now = haven_common::UtcMillis::now();
        repos
            .media_item
            .save(&haven_domain::entities::MediaItem {
                id: d,
                edition_id,
                parent_id: Some(root),
                media_type: haven_domain::enums::MediaType::Episode,
                title: "direct-child".into(),
                index: haven_domain::entities::MediaIndex::Episode {
                    season: None,
                    episode: 1,
                },
                duration_ms: None,
                page_count: None,
                chapter_count: None,
                published_at: None,
                status: haven_domain::enums::MediaItemStatus::Available,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        // 资源挂 direct child 与最深 grandchild。
        attach_resource(&repos, d, location_id).await;
        attach_resource(&repos, g, location_id).await;

        install_purge_order_log(&db);
        storage.remove(location_id).await.unwrap();
        let deleted = read_purge_order(&db);

        // child-first：{d,g}（depth0）先于 p（depth1），p 先于 root（depth2）。
        assert_eq!(deleted.len(), 4, "独占四节点全部删除");
        let (first, rest) = deleted.split_at(2);
        let first_set: std::collections::HashSet<&String> = first.iter().collect();
        assert_eq!(
            first_set,
            std::collections::HashSet::from([&d.to_string(), &g.to_string()]),
            "depth0 必须先删 direct child 与 grandchild（内部顺序不限）"
        );
        assert_eq!(rest[0], p.to_string(), "depth1 后删 p");
        assert_eq!(rest[1], root.to_string(), "depth2 最后删 root");

        for table in ["resources", "media_items", "editions", "works"] {
            assert_eq!(table_count(&db, table), 0, "{table} 必须全清");
        }
        assert!(!purge_temp_leaked(&db), "独占清理不得泄漏 TEMP 中间表");
    }

    /// R-MAIN-09C Important 2：多路径汇聚且 **共享分支**（资源同时挂 G、D、P）——
    /// 仅独占叶（G、D）删除；共享节点 P 与祖先 root 及其用户状态保留。
    #[tokio::test]
    async fn purge_multi_depth_convergence_preserves_shared_branch() {
        use haven_domain::contracts::MediaItemRepository;

        let dir = tempfile::TempDir::new().unwrap();
        let dir2 = tempfile::TempDir::new().unwrap();
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = SqliteRepositories::new(db.clone());
        let storage = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
        let location_a = storage
            .add_local("共享汇聚焦".into(), dir.path())
            .await
            .unwrap();
        // 外部位置 B：资源引用 P 的另一个子节点 X → P 的"子引用"来自位置 B（外部），
        // remove(A) 不得删除 P 及其祖先 root。
        let location_b = storage
            .add_local("外部库".into(), dir2.path())
            .await
            .unwrap();
        let (_work_id, edition_id) = seed_work_and_edition(&repos).await;

        let chain = seed_media_chain(&repos, edition_id, 2).await;
        let (g, p, root) = (chain[0], chain[1], chain[2]);
        let now = haven_common::UtcMillis::now();
        let d = haven_domain::ids::MediaItemId::new();
        let x = haven_domain::ids::MediaItemId::new();
        repos
            .media_item
            .save(&haven_domain::entities::MediaItem {
                id: d,
                edition_id,
                parent_id: Some(root),
                media_type: haven_domain::enums::MediaType::Episode,
                title: "direct-child".into(),
                index: haven_domain::entities::MediaIndex::Episode {
                    season: None,
                    episode: 1,
                },
                duration_ms: None,
                page_count: None,
                chapter_count: None,
                published_at: None,
                status: haven_domain::enums::MediaItemStatus::Available,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        repos
            .media_item
            .save(&haven_domain::entities::MediaItem {
                id: x,
                edition_id,
                parent_id: Some(p),
                media_type: haven_domain::enums::MediaType::Episode,
                title: "external-child".into(),
                index: haven_domain::entities::MediaIndex::Episode {
                    season: None,
                    episode: 1,
                },
                duration_ms: None,
                page_count: None,
                chapter_count: None,
                published_at: None,
                status: haven_domain::enums::MediaItemStatus::Available,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        // 位置 A 资源：G（最深 grandchild）与 D（direct child）——独占叶。
        attach_resource(&repos, g, location_a).await;
        attach_resource(&repos, d, location_a).await;
        // 外部位置 B 资源：X（P 的子）——P 的"外部共享 child"。
        attach_resource(&repos, x, location_b).await;

        // remove(A)：仅删除 A 的独占叶 G、D；P 因外部 child X 保留，root 因 P 保留。
        storage.remove(location_a).await.unwrap();
        assert_eq!(table_count(&db, "resources"), 1, "仅剩位置 B 的资源");
        assert_eq!(
            table_count(&db, "media_items"),
            3,
            "G、D 删除；P、root、X 保留"
        );
        for id in [g, d] {
            let n: i64 = db
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM media_items WHERE id = ?1",
                    rusqlite::params![id.to_string()],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 0, "独占叶必须删除");
        }
        for id in [p, root, x] {
            let n: i64 = db
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM media_items WHERE id = ?1",
                    rusqlite::params![id.to_string()],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "共享节点、祖先与外部子必须保留");
        }
        assert_eq!(table_count(&db, "editions"), 1, "共享分支 edition 保留");
        assert_eq!(table_count(&db, "works"), 1, "共享分支 work 保留");
        {
            let conn = db.lock();
            let mut stmt = conn.prepare("PRAGMA foreign_key_check").unwrap();
            let mut rows = stmt.query([]).unwrap();
            assert!(rows.next().unwrap().is_none(), "共享保留后不得有 FK 损坏");
        }
        assert!(!purge_temp_leaked(&db), "共享分支不得泄漏 TEMP 中间表");
    }
}
