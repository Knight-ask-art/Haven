CREATE TABLE download_tasks (
    id                 TEXT PRIMARY KEY,
    work_id            TEXT REFERENCES works(id) ON DELETE RESTRICT,
    edition_id         TEXT REFERENCES editions(id) ON DELETE RESTRICT,
    media_item_id      TEXT REFERENCES media_items(id) ON DELETE RESTRICT,
    source_resource_id TEXT NOT NULL REFERENCES resources(id) ON DELETE RESTRICT,
    target_storage_id  TEXT NOT NULL REFERENCES storage_locations(id) ON DELETE RESTRICT,
    state              TEXT NOT NULL CHECK (state IN (
        'queued', 'resolving', 'downloading', 'paused', 'verifying',
        'completed', 'failed', 'cancelled', 'interrupted'
    )),
    bytes_total        INTEGER CHECK (bytes_total IS NULL OR bytes_total >= 0),
    bytes_downloaded   INTEGER NOT NULL DEFAULT 0 CHECK (bytes_downloaded >= 0),
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL,
    CHECK (bytes_total IS NULL OR bytes_downloaded <= bytes_total)
);

CREATE INDEX idx_download_tasks_state ON download_tasks(state);
CREATE INDEX idx_download_tasks_created_at ON download_tasks(created_at DESC);
CREATE INDEX idx_download_tasks_media_item_id ON download_tasks(media_item_id);
CREATE INDEX idx_download_tasks_source_target
    ON download_tasks(source_resource_id, target_storage_id);
