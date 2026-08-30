-- 021_artwork_cache: 受控 Artwork 的来源策略、抓取状态和本地文件索引。
--
-- image_proxy.id 是已经进入 Work/Artwork 引用的稳定身份，迁移不得重建或改写它。
-- image_cache_entries 只描述可删除、可联网重建的技术缓存；SQLite 不保存图片字节。
ALTER TABLE image_proxy ADD COLUMN source_id TEXT;
ALTER TABLE image_proxy ADD COLUMN normalized_host TEXT;
ALTER TABLE image_proxy ADD COLUMN last_fetch_status TEXT NOT NULL DEFAULT 'unknown';
ALTER TABLE image_proxy ADD COLUMN last_fetched_at INTEGER;
ALTER TABLE image_proxy ADD COLUMN last_error_code TEXT;

CREATE INDEX IF NOT EXISTS idx_image_proxy_source_host
    ON image_proxy (source_id, normalized_host);

CREATE TABLE IF NOT EXISTS image_cache_entries (
    artwork_id           TEXT NOT NULL,
    variant              TEXT NOT NULL,
    relative_path        TEXT NOT NULL,
    mime                  TEXT NOT NULL,
    content_hash         TEXT NOT NULL,
    byte_size             INTEGER NOT NULL,
    etag                  TEXT,
    last_modified         TEXT,
    last_accessed_at      INTEGER NOT NULL,
    expires_at            INTEGER NOT NULL,
    stale_if_error_until  INTEGER NOT NULL,
    PRIMARY KEY (artwork_id, variant),
    FOREIGN KEY (artwork_id) REFERENCES image_proxy(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_image_cache_entries_eviction
    ON image_cache_entries (expires_at, last_accessed_at);
