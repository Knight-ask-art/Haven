-- 025_work_fts: Shadow columns for FTS5 (contentless, triggers deferred to 027 to avoid legacy DB issues)
ALTER TABLE works ADD COLUMN search_title TEXT NOT NULL DEFAULT '';
ALTER TABLE works ADD COLUMN search_original_title TEXT NOT NULL DEFAULT '';
ALTER TABLE works ADD COLUMN search_body TEXT NOT NULL DEFAULT '';
