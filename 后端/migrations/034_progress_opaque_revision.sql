-- 034_progress_opaque_revision: Progress 使用独立 opaque CAS token。
--
-- `updated_at` 仅是 RFC3339 展示时间的持久化来源，不再承担并发版本语义。
-- 旧行使用 SQLite 随机字节生成一次性迁移 token；新写入由 Repository 生成 UUID v4。
ALTER TABLE progress ADD COLUMN revision TEXT NOT NULL DEFAULT '';

UPDATE progress
SET revision = 'legacy-' || lower(hex(randomblob(16)))
WHERE revision = '';
