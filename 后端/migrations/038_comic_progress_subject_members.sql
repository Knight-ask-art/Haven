-- 038_comic_progress_subject_members: Subject 与媒体条目之间的连续性关系。
--
-- evidence_json 只由领域 ChapterEvidence 序列化产生，不是运行时请求/资源通道。
CREATE TABLE comic_progress_subject_members (
    subject_id          TEXT NOT NULL REFERENCES comic_progress_subjects(id) ON DELETE CASCADE,
    media_item_id       TEXT NOT NULL REFERENCES media_items(id) ON DELETE RESTRICT,
    relationship        TEXT NOT NULL
                        CHECK (relationship IN ('canonical', 'equivalent', 'candidate')),
    confidence          TEXT NOT NULL CHECK (confidence IN ('high', 'medium', 'low')),
    evidence_json       TEXT NOT NULL,
    state               TEXT NOT NULL
                        CHECK (state IN ('active', 'candidate', 'retired')),
    algorithm_version   TEXT NOT NULL,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    PRIMARY KEY (subject_id, media_item_id, state)
);

CREATE UNIQUE INDEX uq_comic_progress_subject_members_active_media_item
    ON comic_progress_subject_members (media_item_id)
    WHERE state = 'active';
