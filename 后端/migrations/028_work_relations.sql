-- 028_work_relations: Work relations for deduplication and Edition grouping
-- Directed relations with 10 types, unique per (from, to, type)

CREATE TABLE work_relations (
    id              TEXT PRIMARY KEY,
    from_work_id    TEXT NOT NULL REFERENCES works(id) ON DELETE CASCADE,
    to_work_id      TEXT NOT NULL REFERENCES works(id) ON DELETE CASCADE,
    relation_type   TEXT NOT NULL CHECK (relation_type IN (
        'original_of','adaptation_of','sequel_of','prequel_of','spin_off_of',
        'side_story_of','remake_of','same_franchise','inspired_by','related'
    )),
    evidence        TEXT,
    created_at      INTEGER NOT NULL,
    UNIQUE(from_work_id, to_work_id, relation_type),
    CHECK (from_work_id != to_work_id)
);

CREATE INDEX idx_work_relations_from ON work_relations(from_work_id);
CREATE INDEX idx_work_relations_to ON work_relations(to_work_id);
CREATE INDEX idx_work_relations_type ON work_relations(relation_type);
