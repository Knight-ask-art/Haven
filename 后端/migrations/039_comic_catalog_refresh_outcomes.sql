-- 039_comic_catalog_refresh_outcomes: append-only 的漫画目录刷新结果收据。
--
-- 结果仅包含来源 opaque identity、generation 和有界观察事实；不保存 URL、Cookie、
-- grant、请求头、本地路径或其他运行时授权。
CREATE TABLE comic_catalog_refresh_outcomes (
    id                          TEXT PRIMARY KEY,
    work_id                     TEXT NOT NULL REFERENCES works(id) ON DELETE RESTRICT,
    source_key                  TEXT NOT NULL,
    remote_work_id              TEXT NOT NULL,
    status                      TEXT NOT NULL CHECK (status IN (
                                    'never_synced', 'succeeded', 'temporarily_unavailable',
                                    'external_only', 'unknown', 'refresh_failed', 'truncated'
                                )),
    generation_before           INTEGER NOT NULL CHECK (generation_before >= 0),
    generation_after            INTEGER CHECK (generation_after IS NULL OR generation_after >= 0),
    observed_from               TEXT,
    observed_to                 TEXT,
    truncated                   INTEGER NOT NULL CHECK (truncated IN (0, 1)),
    retained_previous_catalog   INTEGER NOT NULL CHECK (retained_previous_catalog IN (0, 1)),
    error_code                  TEXT,
    observed_at                 INTEGER NOT NULL,
    CHECK (length(trim(source_key)) > 0),
    CHECK (length(trim(remote_work_id)) > 0)
);

CREATE INDEX idx_comic_catalog_refresh_outcomes_work_observed
    ON comic_catalog_refresh_outcomes (work_id, observed_at DESC, id DESC);
