-- 010_query_indexes: 查询路径索引（审查批次 P0-1 / P1-3 / P2-2 清偿）
--
-- 1. idx_resources_local_path：local_file 资源按 (storage_location_id, locator path)
--    点查的表达式索引。Locator 外部标签枚举序列化为 {"local_path":{"path":...}}，
--    json_extract 为确定性函数，可入表达式索引；此前 find_resource_by_local_path
--    逐文件全表枚举 + 内存 JSON 反序列化比对（O(N²) 且放大全局连接锁持有时间）。
-- 2. idx_resources_storage：disconnect/rebind/remove 按 storage_location_id 的
--    批量资源操作（mark_unavailable_by_storage / delete_by_storage）此前全表扫描。
-- 3. idx_progress_work_active：library_list 的 LastActive 排序聚合子查询
--    （SELECT work_id, MAX(last_active_at) ... GROUP BY work_id）支撑索引。

CREATE INDEX IF NOT EXISTS idx_resources_local_path
    ON resources(storage_location_id, json_extract(locator_json, '$.local_path.path'));

CREATE INDEX IF NOT EXISTS idx_resources_storage
    ON resources(storage_location_id);

CREATE INDEX IF NOT EXISTS idx_progress_work_active
    ON progress(work_id, last_active_at);
