-- 005_favorites_revision: 收藏状态版本（R-FAV-001 裁决）
--
-- revision 语义：收藏**状态版本**。
-- - 状态变化（未收藏↔收藏）时生成新 revision 并 UPSERT 本表。
-- - 重复设置相同状态：不更新版本，返回当前 revision（幂等收敛）。
-- - 取消收藏时版本行保留，使"重复取消"返回相同 revision。
-- - favorite.changed 事件携带与 FavoriteSetResult 相同的 revision。

CREATE TABLE IF NOT EXISTS work_favorite_versions (
    work_id    TEXT PRIMARY KEY,
    revision   TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
