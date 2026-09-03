-- 036_comic_chapter_profile_observation: 保存每个来源章节最近一次观察到的
-- Edition 画像，避免多个来源章节共享一个 Edition 时丢失来源证据。
--
-- 旧数据没有历史观察列，保持 NULL 并由 Repository 回退到关联 Edition 画像；
-- 新的目录刷新/章节来源写入会保存经过校验的结构化画像 JSON。
ALTER TABLE comic_chapter_source_refs
    ADD COLUMN observed_edition_profile TEXT
        CHECK (
            observed_edition_profile IS NULL
            OR length(trim(observed_edition_profile)) > 0
        );
