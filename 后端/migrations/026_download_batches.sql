-- 026_download_batches: Batch download aggregation for整本/整季
-- Batch aggregates N tasks; Task enters global scheduler, Batch only aggregates state

CREATE TABLE download_batches (
    id                TEXT PRIMARY KEY,
    title             TEXT NOT NULL,
    category          TEXT NOT NULL,
    subject_type      TEXT NOT NULL CHECK (subject_type IN ('resource','media_item','media_group','edition','collection')),
    subject_id        TEXT NOT NULL,
    target_storage_id TEXT NOT NULL REFERENCES storage_locations(id) ON DELETE RESTRICT,
    state             TEXT NOT NULL CHECK (state IN ('queued','downloading','verifying','completed','failed','cancelled','partial_completed')),
    total_tasks       INTEGER NOT NULL DEFAULT 0 CHECK (total_tasks >= 0),
    completed_tasks   INTEGER NOT NULL DEFAULT 0 CHECK (completed_tasks >= 0),
    total_bytes       INTEGER CHECK (total_bytes IS NULL OR total_bytes >= 0),
    completed_bytes   INTEGER NOT NULL DEFAULT 0 CHECK (completed_bytes >= 0),
    created_at        INTEGER NOT NULL,
    updated_at        INTEGER NOT NULL,
    CHECK (completed_tasks <= total_tasks),
    CHECK (total_bytes IS NULL OR completed_bytes <= total_bytes)
);

CREATE INDEX idx_download_batches_state ON download_batches(state);
CREATE INDEX idx_download_batches_created_at ON download_batches(created_at DESC);

-- Extend download_tasks for batch and scheduling

ALTER TABLE download_tasks ADD COLUMN batch_id TEXT REFERENCES download_batches(id) ON DELETE SET NULL;
ALTER TABLE download_tasks ADD COLUMN priority TEXT NOT NULL DEFAULT 'normal' CHECK (priority IN ('low','normal','high'));
ALTER TABLE download_tasks ADD COLUMN provider_key TEXT;
ALTER TABLE download_tasks ADD COLUMN host_key TEXT;
ALTER TABLE download_tasks ADD COLUMN variant_key TEXT NOT NULL DEFAULT '';
ALTER TABLE download_tasks ADD COLUMN resource_identity TEXT;
ALTER TABLE download_tasks ADD COLUMN retry_count INTEGER NOT NULL DEFAULT 0 CHECK (retry_count >= 0);
ALTER TABLE download_tasks ADD COLUMN not_before INTEGER;
ALTER TABLE download_tasks ADD COLUMN resumable INTEGER CHECK (resumable IN (0,1));

CREATE INDEX idx_download_tasks_batch_id ON download_tasks(batch_id);
CREATE INDEX idx_download_tasks_priority ON download_tasks(priority DESC, created_at ASC);
CREATE INDEX idx_download_tasks_host_key ON download_tasks(host_key);
CREATE INDEX idx_download_tasks_provider_key ON download_tasks(provider_key);

-- Partial unique index for active identity deduplication
CREATE UNIQUE INDEX download_task_active_identity
ON download_tasks(resource_identity, target_storage_id, variant_key)
WHERE state IN ('queued','downloading','verifying');

-- For batch atomicity, ensure offline_resource_id remains unique (already in 013)
