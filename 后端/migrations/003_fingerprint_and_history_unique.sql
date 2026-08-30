-- 003_fingerprint_and_history_unique: 扫描指纹列 + 历史唯一约束
--
-- 1. resources 增加指纹列（LocalLibraryScanner 变化检测用）：
--    - modified_ms：文件修改时间（变化检测的一部分）
--    - fingerprint_first / fingerprint_last：FastFingerprint 首/末块 SHA-256
-- 2. history_entries 增加 media_item_id 唯一索引（并发 record 幂等的 DB 保障）：
--    与 Repository upsert（ON CONFLICT(media_item_id)）配合，防止并发双写。

ALTER TABLE resources ADD COLUMN modified_ms INTEGER;
ALTER TABLE resources ADD COLUMN fingerprint_first TEXT;
ALTER TABLE resources ADD COLUMN fingerprint_last TEXT;

CREATE UNIQUE INDEX idx_history_media_item_unique ON history_entries(media_item_id);
