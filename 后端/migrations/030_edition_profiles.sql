-- 030_edition_profiles: 漫画 Edition 画像。
--
-- language 的值继续使用 editions.language；本表额外保存 language_kind，
-- 以区分 unknown 与 not_applicable，避免 nullable 语言字段丢失语义。
-- MirrorLabel 是来源/镜像展示标签，不应单独拆分 Edition。
CREATE TABLE edition_profiles (
    edition_id        TEXT PRIMARY KEY REFERENCES editions(id) ON DELETE CASCADE,
    language_kind     TEXT NOT NULL DEFAULT 'unknown'
                      CHECK (language_kind IN ('unknown', 'known', 'not_applicable')),
    translation_line  TEXT,
    translation_line_kind TEXT NOT NULL DEFAULT 'unknown'
                      CHECK (translation_line_kind IN ('unknown', 'known', 'not_applicable')),
    scan_group        TEXT,
    scan_group_kind   TEXT NOT NULL DEFAULT 'unknown'
                      CHECK (scan_group_kind IN ('unknown', 'content_line', 'mirror_label', 'not_applicable')),
    color_mode        TEXT NOT NULL DEFAULT 'unknown'
                      CHECK (color_mode IN ('unknown', 'full_color', 'grayscale', 'mixed')),
    created_at        INTEGER NOT NULL,
    updated_at        INTEGER NOT NULL,
    CHECK (
        (scan_group_kind IN ('unknown', 'not_applicable') AND scan_group IS NULL)
        OR (scan_group_kind IN ('content_line', 'mirror_label') AND scan_group IS NOT NULL)
    ),
    CHECK (
        (translation_line_kind IN ('unknown', 'not_applicable') AND translation_line IS NULL)
        OR (translation_line_kind = 'known' AND translation_line IS NOT NULL)
    )
);
