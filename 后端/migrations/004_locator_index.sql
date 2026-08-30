-- 004_locator_index: 本地文件定位索引（BE-AUDIT 注意项 C 清偿）
--
-- 1. find_resource_by_local_path 之前按 resource_type/locator_kind 全表枚举再内存过滤；
--    此索引让枚举路径从"全表"收敛到"本地文件子集"。
-- 2. locator_json 是 JSON 文本，路径匹配仍按现有语义在内存完成（SQL 端 JSON 转义
--    可靠性优先），本迁移只收敛候选集，不改变匹配逻辑。

CREATE INDEX IF NOT EXISTS idx_resources_local_file
    ON resources(resource_type, locator_kind);
