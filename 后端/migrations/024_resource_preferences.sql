-- ADR-RESOURCE-PREF-001 / V02-RESOURCE-PREF-001
-- 资源级阅读/漫画偏好与全局 settings 分离。data_json 只保存已校验 Patch。
CREATE TABLE IF NOT EXISTS edition_preferences (
    edition_id  TEXT PRIMARY KEY REFERENCES editions(id) ON DELETE CASCADE,
    data_json   TEXT NOT NULL,
    revision    TEXT NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS media_item_preferences (
    media_item_id TEXT PRIMARY KEY REFERENCES media_items(id) ON DELETE CASCADE,
    edition_id    TEXT NOT NULL REFERENCES editions(id) ON DELETE CASCADE,
    data_json     TEXT NOT NULL,
    revision      TEXT NOT NULL,
    updated_at    INTEGER NOT NULL,
    UNIQUE(media_item_id, edition_id)
);

CREATE INDEX IF NOT EXISTS idx_media_item_preferences_edition
    ON media_item_preferences(edition_id);
