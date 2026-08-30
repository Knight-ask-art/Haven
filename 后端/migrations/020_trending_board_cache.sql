-- 020_trending_board_cache: 搜索页热榜技术快照。
--
-- 这是可删除、可重建的缓存，不是 Work/Edition/MediaItem 事实。
-- payload_json 只允许保存已验证的 canonical TrendingBoardDto（受控 artwork URI）。
CREATE TABLE IF NOT EXISTS trending_board_cache (
    board_id      TEXT PRIMARY KEY,
    source_id     TEXT NOT NULL,
    payload_json  TEXT NOT NULL,
    revision      TEXT NOT NULL,
    refreshed_at  INTEGER NOT NULL,
    expires_at    INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_trending_board_cache_expires
    ON trending_board_cache (expires_at);
