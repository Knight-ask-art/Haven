-- 033_comic_chapter_catalog_refresh: 章节目录刷新状态和来源展示事实。
--
-- 目录刷新必须能区分：来源明确返回的不可用章节、完整目录中消失的章节、
-- 以及因分页上限而暂时看不见的章节。状态只影响来源引用/资源可用性，
-- 不删除 MediaItem、Progress、Marker 或 History。
ALTER TABLE comic_chapter_source_refs
    ADD COLUMN source_order INTEGER NOT NULL DEFAULT 0 CHECK (source_order >= 0);

ALTER TABLE comic_chapter_source_refs
    ADD COLUMN availability TEXT NOT NULL DEFAULT 'unknown'
        CHECK (availability IN (
            'available', 'temporarily_unavailable', 'external_only', 'unknown', 'missing'
        ));

ALTER TABLE comic_chapter_source_refs
    ADD COLUMN published_at TEXT;

ALTER TABLE comic_chapter_source_refs
    ADD COLUMN source_updated_at TEXT;

ALTER TABLE comic_chapter_source_refs
    ADD COLUMN last_seen_generation INTEGER
        CHECK (last_seen_generation IS NULL OR last_seen_generation >= 0);

CREATE TABLE comic_chapter_catalog_states (
    source_key       TEXT NOT NULL,
    remote_work_id   TEXT NOT NULL,
    generation       INTEGER NOT NULL CHECK (generation >= 0),
    fetched_at       INTEGER NOT NULL,
    total            INTEGER,
    truncated        INTEGER NOT NULL CHECK (truncated IN (0, 1)),
    PRIMARY KEY (source_key, remote_work_id)
);
