-- V02-SETTINGS-PRIVACY-DATA-007
-- 搜索历史是可删除的本地偏好，不与播放历史、进度或离线资源共用表。
CREATE TABLE IF NOT EXISTS search_history (
    term         TEXT PRIMARY KEY,
    last_used_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_search_history_last_used
    ON search_history(last_used_at DESC);
