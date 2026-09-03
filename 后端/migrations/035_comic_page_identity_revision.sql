-- 035_comic_page_identity_revision: 为完整页面观察增加独立 opaque revision。
--
-- 页面身份不是单页事实的简单集合：空页面序列也必须能够被版本化，
-- 因此 revision 单独存放在 state 表，而不是重复写入每一行页面。
CREATE TABLE comic_page_identity_states (
    media_item_id TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
    revision      TEXT NOT NULL,
    updated_at    INTEGER NOT NULL,
    PRIMARY KEY (media_item_id),
    CHECK (length(trim(revision)) > 0)
);

-- 已经观察过页面的媒体条目获得一次性 legacy revision；未观察过的条目
-- 保持无 state 行，由首次 replace_if_revision(None) 原子创建。
INSERT INTO comic_page_identity_states (media_item_id, revision, updated_at)
SELECT media_item_id,
       'legacy-' || lower(hex(randomblob(16))),
       MAX(updated_at)
FROM comic_page_identities
GROUP BY media_item_id;
