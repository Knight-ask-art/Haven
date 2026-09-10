-- 037_comic_progress_subjects: 跨章节漫画进度主体及其可追踪状态。
--
-- Subject 只保存作品/版本/媒体条目身份映射和连续性状态；页码、比例、完成度、
-- keyframe 与 Progress revision 继续只属于 progress 表。
CREATE TABLE comic_progress_subjects (
    id                                  TEXT PRIMARY KEY,
    work_id                             TEXT NOT NULL REFERENCES works(id) ON DELETE RESTRICT,
    edition_id                          TEXT NOT NULL REFERENCES editions(id) ON DELETE RESTRICT,
    canonical_media_item_id             TEXT NOT NULL REFERENCES media_items(id) ON DELETE RESTRICT,
    authoritative_progress_media_item_id TEXT REFERENCES media_items(id) ON DELETE SET NULL,
    state                               TEXT NOT NULL
                                        CHECK (state IN ('active', 'redirected')),
    created_at                          INTEGER NOT NULL,
    updated_at                          INTEGER NOT NULL,
    algorithm_version                   TEXT NOT NULL,
    redirect_subject_id                 TEXT REFERENCES comic_progress_subjects(id) ON DELETE RESTRICT,
    CHECK (
        (state = 'active' AND redirect_subject_id IS NULL)
        OR (state = 'redirected' AND redirect_subject_id IS NOT NULL)
    )
);

CREATE INDEX idx_comic_progress_subjects_work
    ON comic_progress_subjects (work_id, created_at, id);
