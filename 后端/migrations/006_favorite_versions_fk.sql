-- 006_favorite_versions_fk: work_favorite_versions 外键（非阻塞建议 → 复核修复）
--
-- 005 未引用 works(id)：旧库可能合法存在"Work 已删除、版本行残留"的孤儿行。
-- foreign_keys=ON 时无条件复制孤儿行会触发 FOREIGN KEY constraint failed 使迁移失败
-- （第四轮复核复现）。故复制时 JOIN works 过滤孤儿（清理策略：孤儿版本行随迁移删除）。
-- SQLite 无法直接 ALTER 加 FK，重建表并迁移数据（迁移系统 checksum 保护已应用迁移，故追加 006）。

CREATE TABLE work_favorite_versions_new (
    work_id    TEXT PRIMARY KEY REFERENCES works(id) ON DELETE CASCADE,
    revision   TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);

INSERT INTO work_favorite_versions_new (work_id, revision, updated_at)
SELECT v.work_id, v.revision, v.updated_at
FROM work_favorite_versions AS v
JOIN works AS w ON w.id = v.work_id;

DROP TABLE work_favorite_versions;
ALTER TABLE work_favorite_versions_new RENAME TO work_favorite_versions;
