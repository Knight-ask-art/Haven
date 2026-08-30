-- 014_work_source_refs: 来源作品去重引用（契约 §36.1 去重键的持久化事实）。
--
-- - (provider, external_id) 二元组唯一：同一来源同一外部条目至多映射一个 Work。
-- - work 删除级联清理引用；引用不携带任何端点 URL 或凭据。
CREATE TABLE IF NOT EXISTS work_source_refs (
    provider     TEXT NOT NULL,
    external_id  TEXT NOT NULL,
    work_id      TEXT NOT NULL PRIMARY KEY REFERENCES works(id) ON DELETE CASCADE,
    UNIQUE (provider, external_id)
);

CREATE INDEX IF NOT EXISTS idx_work_source_refs_lookup
    ON work_source_refs (provider, external_id);
