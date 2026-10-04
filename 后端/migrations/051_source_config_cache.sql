-- 051_source_config_cache.sql
--
-- 来源「最后一次成功获取的原始配置」缓存。
--
-- 为什么不是设置行：来源的启用状态、端点与凭据引用是**设置域**事实，存在
-- `settings` 表（section=`sources`）里；而一份 TVBox / FongMi 配置的原文有 8 MiB
-- 上限、可能含站点端点、header 与凭据，属于**可丢弃的缓存**。把两者放进同一份
-- JSON 会让每次读写设置都要搬运整份配置，也会让「配置原文」顺带出现在任何导出
-- 设置行的路径上。因此这里单独一张窄表：按 source_id 一源一行。
--
-- 只保存事实，不保存解释：
-- - body 是**已经过解析器校验**的原始字节（UTF-8 JSON）。保存原文而不是解析结果，
--   是因为解析结构（站点/直播/解析器分类）会随解析器版本演进，而「当时那份配置」
--   只有一份。重放解析永远可以从 body 得到当时的结构。
-- - 表里刻意没有 URL、没有 header、没有凭据列：端点与凭据引用属于来源身份，仍在
--   设置域；缓存只认 source_id。
-- - 外键不指向来源表：来源身份存在 settings JSON 里（不是表），而且删除来源时必须
--   能**独立**清掉这行缓存——见 SourceRegistryService::remove_custom_source 的删除顺序。
CREATE TABLE source_config_cache (
    source_id  TEXT PRIMARY KEY
        CHECK (length(source_id) BETWEEN 1 AND 64),
    body       BLOB NOT NULL
        CHECK (length(body) > 0),
    fetched_at INTEGER NOT NULL
);
