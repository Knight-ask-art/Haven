//! 存储位置的**应用内内容清理**权威实现：purge 算法 + 可用性覆盖规则。
//!
//! 边界：
//! - 这里的 SQL 是「移除位置 / 位置失效」的唯一权威定义：本地位置（`SqliteStorageUoW`）
//!   与云盘目录（云盘 Repository 的 remove）都调用同一份实现，不允许复制第二套等价
//!   SQL，也不允许任何一方改变规则。
//! - 只清理**应用内索引**：该位置的 Resource、仅由该位置派生的孤儿内容链
//!   （media_items → editions → works）与其用户状态（progress / markers / favorites /
//!   history_entries）。用户原始文件（本地文件或远端 Provider 文件）绝不删除；其他位置
//!   仍引用的共享内容完整保留。
//! - `*_on_conn` 只接收 `&Connection`，由调用方保证已在同一事务（或同一把 db 锁）内。

use rusqlite::Connection;

use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::enums::{Availability, AvailabilitySource};
use haven_domain::ids::StorageLocationId;

use crate::db::repos::enum_to_db_str;

/// R-MAIN-09D：purge 中间表唯一内部名（明确 temp schema；DROP 用同一定义，避免散落字符串）。
const PURGE_TEMP_TABLE: &str = "temp._haven_storage_purge_media_ids";

/// 事务**边界外**（同一 db mutex guard 内）清理 purge 中间表的语句。SQLite 的 TEMP DDL
/// 是事务性的：事务回滚会撤销本次 CREATE，却会恢复**事务开始前已存在**的旧表，因此每个
/// 走 purge 的调用方都必须在事务开始前先清一次。
pub(crate) const PURGE_TEMP_DROP_SQL: &str =
    "DROP TABLE IF EXISTS temp._haven_storage_purge_media_ids";

fn tx_err(msg: &'static str, e: rusqlite::Error) -> AppError {
    AppError::new("DATABASE_ERROR", ErrorKind::Database, msg, true).with_source(e)
}

pub(crate) fn delete_resources_on_conn(
    conn: &Connection,
    storage_location_id: StorageLocationId,
) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM resources WHERE storage_location_id = ?1",
        rusqlite::params![storage_location_id.to_string()],
    )
    .map_err(|e| tx_err("删除位置索引资源失败", e))?;
    Ok(())
}

/// R-MAIN-08 覆盖规则（位置失效 / 无效化）的**唯一**实现：
/// 只覆盖 a) 当前 `availability = 'available'` 的行（即使 `availability_source = 'user'`，
/// 位置不可达时有效可用性也必须失效）；或 b) `availability_source = 'storage'` 的行
/// （重复标记 / 状态迁移收敛）。user 显式标记的不可用状态（SourceUnavailable /
/// TemporarilyUnavailable / Unknown / 自身 Missing）与未知状态一律原样保留。
pub(crate) fn set_resources_availability_on_conn(
    conn: &Connection,
    storage_location_id: StorageLocationId,
    availability: Availability,
    source: AvailabilitySource,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE resources SET availability = ?1, availability_source = ?2, updated_at = ?3
         WHERE storage_location_id = ?4
           AND (availability = 'available' OR availability_source = 'storage')",
        rusqlite::params![
            enum_to_db_str(&availability)?,
            enum_to_db_str(&source)?,
            UtcMillis::now().0,
            storage_location_id.to_string(),
        ],
    )
    .map_err(|e| tx_err("批量标记资源可用性失败", e))?;
    Ok(())
}

/// 删除某存储位置的全部应用内痕迹（INTEGRATION-SLICE-001「选错目录」缺口）：
/// 该位置的 Resource，以及**仅由该位置派生**的孤儿内容链（media_items → editions →
/// works）与其用户状态（progress / markers / favorites / history_entries，均为 RESTRICT
/// 须先行删除）。其他位置仍引用的共享内容（共享 edition / work / media_item）完整保留；
/// `work_favorite_versions` 随 works 外键 CASCADE。孤儿判定经连接级临时表传递
/// （无 IN 参数上限问题）。绝不删除用户原始文件（本地或远端）。
pub(crate) fn purge_location_content(
    conn: &Connection,
    storage_location_id: StorageLocationId,
) -> Result<(), AppError> {
    let loc = storage_location_id.to_string();
    let exec = |sql: &str| -> Result<(), AppError> {
        conn.execute(sql, [])
            .map_err(|e| tx_err("位置内容清理失败", e))?;
        Ok(())
    };

    // R-MAIN-09C/09D Important 1：TEMP 中间表用唯一内部名并**明确限定 temp schema**，
    // 避免共享 SQLite 连接上跨事务残留、以及未限定 DROP 误伤 main 同名对象。
    // 事务开始前的防御性清理由调用方（`SqliteStorageUoW::run` / `with_cloud_purge_tx`）
    // 在事务边界外完成，因此此处不再需要开头 DROP；成功后仍保留 DROP 作为局部自清理。
    let purge_temp = PURGE_TEMP_TABLE;
    exec(
        "CREATE TEMP TABLE _haven_storage_purge_media_ids(
             stage INTEGER NOT NULL, id TEXT NOT NULL, parent TEXT, depth INTEGER NOT NULL DEFAULT 0)",
    )?;
    conn.execute(
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

    delete_resources_on_conn(conn, storage_location_id)?;

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

    let max_depth: Option<i64> = conn
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
    exec(PURGE_TEMP_DROP_SQL)?;
    Ok(())
}
