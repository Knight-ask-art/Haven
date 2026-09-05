-- 032_comic_progress_migration_snapshots: 漫画进度迁移的可撤销审计快照。
--
-- old/new_progress_json 仅是内部 SQLite 快照，不是 Wire。应用层只对 Comic
-- Locator 生成快照，因此不包含 pageId、grant、远端 URL 或归档 entry。
-- RESTRICT 保留快照引用的 MediaItem，避免用户仍可撤销时物理删除内容节点。
CREATE TABLE comic_progress_migration_snapshots (
    id                       TEXT PRIMARY KEY,
    source_media_item_id     TEXT NOT NULL REFERENCES media_items(id) ON DELETE RESTRICT,
    target_media_item_id     TEXT NOT NULL REFERENCES media_items(id) ON DELETE RESTRICT,
    source_revision           TEXT NOT NULL,
    target_revision_before    TEXT,
    old_progress_json         TEXT NOT NULL,
    old_target_progress_json  TEXT,
    new_progress_json         TEXT NOT NULL,
    mode                      TEXT NOT NULL
                              CHECK (mode IN ('shared', 'one_time', 'suggested', 'none')),
    confidence                TEXT NOT NULL
                              CHECK (confidence IN ('high', 'medium', 'low')),
    strategy                  TEXT NOT NULL
                              CHECK (strategy IN ('stable_key', 'content_fingerprint', 'reordered_anchor',
                                                   'nearest_surviving_page', 'proportional_fallback', 'no_target')),
    evidence_json             TEXT NOT NULL,
    created_at                INTEGER NOT NULL,
    applied_revision          TEXT,
    state                     TEXT NOT NULL DEFAULT 'applied'
                              CHECK (state IN ('applied', 'reverted')),
    reverted_at               INTEGER
);

CREATE INDEX idx_comic_progress_migration_snapshots_created
    ON comic_progress_migration_snapshots (created_at DESC);

CREATE INDEX idx_comic_progress_migration_snapshots_target
    ON comic_progress_migration_snapshots (target_media_item_id, state);
