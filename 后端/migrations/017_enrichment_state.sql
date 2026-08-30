-- 017_enrichment_state: Enrichment 流水线状态（契约 §36.8）。
-- 每个 Work 至多一条；匹配失败不回滚扫描，保留原始名并标 failed。
-- status 为闭合枚举：pending | enriched | failed。
CREATE TABLE IF NOT EXISTS enrichment_state (
    work_id    TEXT PRIMARY KEY,
    status     TEXT NOT NULL,
    source_id  TEXT,
    error      TEXT,
    updated_at INTEGER NOT NULL
);
