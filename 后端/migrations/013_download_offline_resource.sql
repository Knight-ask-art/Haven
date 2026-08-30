ALTER TABLE download_tasks
    ADD COLUMN offline_resource_id TEXT REFERENCES resources(id) ON DELETE SET NULL;

CREATE UNIQUE INDEX idx_download_tasks_offline_resource_id
    ON download_tasks(offline_resource_id);

