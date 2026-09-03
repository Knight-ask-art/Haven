-- 031_comic_chapter_and_page_identity: 章节来源身份与页面稳定身份。
--
-- 这些表只保存经过 provider 校验的 opaque identity 和内容 fingerprint。
-- pageId、grant、远端 URL、CDN 地址、归档 entry 和 provider header 永不入库。
CREATE TABLE comic_chapter_source_refs (
    source_key                  TEXT NOT NULL,
    remote_work_id              TEXT NOT NULL,
    remote_chapter_id           TEXT NOT NULL,
    media_item_id               TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
    chapter_number              REAL,
    volume_number               REAL,
    title                       TEXT,
    page_count                  INTEGER,
    authoritative_content_key   TEXT,
    updated_at                  INTEGER NOT NULL,
    PRIMARY KEY (source_key, remote_work_id, remote_chapter_id),
    CHECK (length(trim(source_key)) > 0),
    CHECK (length(trim(remote_work_id)) > 0),
    CHECK (length(trim(remote_chapter_id)) > 0)
);

CREATE INDEX idx_comic_chapter_source_refs_media_item
    ON comic_chapter_source_refs (media_item_id);

CREATE TABLE comic_page_identities (
    media_item_id TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
    page_index    INTEGER NOT NULL CHECK (page_index >= 0),
    stable_key    TEXT,
    fingerprint   TEXT,
    updated_at    INTEGER NOT NULL,
    PRIMARY KEY (media_item_id, page_index)
);

CREATE INDEX idx_comic_page_identities_stable_key
    ON comic_page_identities (media_item_id, stable_key);

CREATE INDEX idx_comic_page_identities_fingerprint
    ON comic_page_identities (media_item_id, fingerprint);
