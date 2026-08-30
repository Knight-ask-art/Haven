-- 001_initial: 核心领域表（Work / Edition / MediaItem / Resource / Progress / Marker / ...）
-- 规范：plan/DOMAIN_MODEL.md、plan/TECHNICAL_ARCHITECTURE.md §25
-- 约定：
--   * ID 一律 TEXT（UUID v7 字符串）
--   * 时间一律 INTEGER（UTC 毫秒）
--   * 枚举一律 TEXT（snake_case，与 Rust 枚举 serde 一致）
--   * Locator 存 JSON 文本（locator_json），版本号单独字段

CREATE TABLE works (
    id               TEXT PRIMARY KEY,
    canonical_title  TEXT NOT NULL,
    original_title   TEXT,
    sort_title       TEXT,
    description      TEXT,
    work_type        TEXT NOT NULL,
    release_year     INTEGER,
    language         TEXT,
    status           TEXT NOT NULL,
    poster           TEXT,
    cover            TEXT,
    backdrop         TEXT,
    thumbnail        TEXT,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL
);

CREATE TABLE editions (
    id                  TEXT PRIMARY KEY,
    work_id             TEXT NOT NULL REFERENCES works(id) ON DELETE RESTRICT,
    title               TEXT NOT NULL,
    subtitle            TEXT,
    edition_type        TEXT NOT NULL,
    release_date        TEXT,
    language            TEXT,
    region              TEXT,
    publisher_or_studio TEXT,
    description         TEXT,
    poster              TEXT,
    cover               TEXT,
    backdrop            TEXT,
    thumbnail           TEXT,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL
);
CREATE INDEX idx_editions_work ON editions(work_id);

CREATE TABLE media_items (
    id              TEXT PRIMARY KEY,
    edition_id      TEXT NOT NULL REFERENCES editions(id) ON DELETE RESTRICT,
    parent_id       TEXT REFERENCES media_items(id) ON DELETE RESTRICT,
    media_type      TEXT NOT NULL,
    title           TEXT NOT NULL,
    category        TEXT NOT NULL DEFAULT 'all',
    season          INTEGER,
    episode         INTEGER,
    volume          REAL,
    chapter         REAL,
    ordinal         REAL,
    custom_label    TEXT,
    duration_ms     INTEGER,
    page_count      INTEGER,
    chapter_count   INTEGER,
    published_at    TEXT,
    status          TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
CREATE INDEX idx_media_items_edition ON media_items(edition_id);
CREATE INDEX idx_media_items_parent ON media_items(parent_id);
CREATE INDEX idx_media_items_category ON media_items(category);

CREATE TABLE resources (
    id                  TEXT PRIMARY KEY,
    media_item_id       TEXT NOT NULL REFERENCES media_items(id) ON DELETE RESTRICT,
    resource_type       TEXT NOT NULL,
    source_id           TEXT,
    storage_location_id TEXT,
    locator_kind        TEXT NOT NULL,
    locator_json        TEXT NOT NULL,
    mime_type           TEXT,
    size                INTEGER,
    hash_algorithm      TEXT,
    hash_digest         TEXT,
    availability        TEXT NOT NULL,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL
);
CREATE INDEX idx_resources_media_item ON resources(media_item_id);

CREATE TABLE progress (
    id              TEXT PRIMARY KEY,
    work_id         TEXT NOT NULL REFERENCES works(id) ON DELETE RESTRICT,
    edition_id      TEXT NOT NULL REFERENCES editions(id) ON DELETE RESTRICT,
    media_item_id   TEXT NOT NULL UNIQUE REFERENCES media_items(id) ON DELETE RESTRICT,
    locator_json    TEXT NOT NULL,
    locator_version INTEGER NOT NULL DEFAULT 1,
    completion      TEXT NOT NULL,
    percentage      REAL,
    last_active_at  INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
CREATE INDEX idx_progress_last_active ON progress(last_active_at DESC);

CREATE TABLE markers (
    id             TEXT PRIMARY KEY,
    work_id        TEXT NOT NULL REFERENCES works(id) ON DELETE RESTRICT,
    edition_id     TEXT NOT NULL REFERENCES editions(id) ON DELETE RESTRICT,
    media_item_id  TEXT NOT NULL REFERENCES media_items(id) ON DELETE RESTRICT,
    locator_json   TEXT NOT NULL,
    marker_type    TEXT NOT NULL,
    title          TEXT,
    excerpt        TEXT,
    note           TEXT,
    preview        TEXT,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,
    deleted_at     INTEGER
);
CREATE INDEX idx_markers_media_item ON markers(media_item_id);
CREATE INDEX idx_markers_deleted ON markers(deleted_at);

CREATE TABLE favorites (
    work_id       TEXT REFERENCES works(id) ON DELETE RESTRICT,
    edition_id    TEXT REFERENCES editions(id) ON DELETE RESTRICT,
    media_item_id TEXT REFERENCES media_items(id) ON DELETE RESTRICT,
    created_at    INTEGER NOT NULL,
    CHECK (
        (work_id IS NOT NULL) +
        (edition_id IS NOT NULL) +
        (media_item_id IS NOT NULL) = 1
    )
);
CREATE UNIQUE INDEX idx_favorites_work ON favorites(work_id) WHERE work_id IS NOT NULL;
CREATE UNIQUE INDEX idx_favorites_edition ON favorites(edition_id) WHERE edition_id IS NOT NULL;
CREATE UNIQUE INDEX idx_favorites_media_item ON favorites(media_item_id) WHERE media_item_id IS NOT NULL;
CREATE INDEX idx_favorites_created ON favorites(created_at DESC);

CREATE TABLE history_entries (
    id              TEXT PRIMARY KEY,
    media_item_id   TEXT NOT NULL REFERENCES media_items(id) ON DELETE RESTRICT,
    work_id         TEXT NOT NULL REFERENCES works(id) ON DELETE RESTRICT,
    edition_id      TEXT NOT NULL REFERENCES editions(id) ON DELETE RESTRICT,
    locator_json    TEXT,
    started_at      INTEGER NOT NULL,
    last_active_at  INTEGER NOT NULL,
    completed_at    INTEGER
);
CREATE INDEX idx_history_media_item ON history_entries(media_item_id);
CREATE INDEX idx_history_last_active ON history_entries(last_active_at DESC);

CREATE TABLE storage_locations (
    id              TEXT PRIMARY KEY,
    provider_type   TEXT NOT NULL,
    display_name    TEXT NOT NULL,
    root_ref        TEXT NOT NULL,
    credential_ref  TEXT,
    status          TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
