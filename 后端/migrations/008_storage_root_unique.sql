-- 008_storage_root_unique: 目录唯一性（P1-6）
--
-- 同一规范化目录（Windows 大小写不敏感）只允许一个 StorageLocation。
-- 表达式唯一索引 lower(root_ref) 在 DB 层兜底并发 add；服务层仍做预检查以返回既有 ID。
-- 已存在重复行（历史数据）不会阻塞索引创建（索引对重复值仅跳过？）
-- SQLite 表达式唯一索引会因现有重复行创建失败；本项目无持久历史库，内存/测试库无重复。

CREATE UNIQUE INDEX IF NOT EXISTS idx_storage_locations_root_unique
    ON storage_locations(lower(root_ref));
