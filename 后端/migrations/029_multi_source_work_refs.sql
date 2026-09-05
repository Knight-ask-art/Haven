-- 029_multi_source_work_refs: 允许一个 Work 绑定多个来源引用。
--
-- 014 的 work_id 是 PRIMARY KEY，导致同一个 Work 只能保留一条来源引用。
-- 本迁移保留已有数据，把唯一性收敛到 (provider, external_id)：
--   * 同一来源同一远端作品至多绑定一个 Work；
--   * 一个 Work 可以有多条来源引用；
--   * 删除 Work 仍级联删除其来源引用；
--   * 不保存端点 URL、路径、凭据或运行时 grant。
CREATE TABLE work_source_refs_new (
    provider     TEXT NOT NULL,
    external_id  TEXT NOT NULL,
    work_id      TEXT NOT NULL REFERENCES works(id) ON DELETE CASCADE,
    PRIMARY KEY (provider, external_id)
);

INSERT INTO work_source_refs_new (provider, external_id, work_id)
SELECT provider, external_id, work_id
FROM work_source_refs;

DROP TABLE work_source_refs;
ALTER TABLE work_source_refs_new RENAME TO work_source_refs;

CREATE INDEX idx_work_source_refs_work
    ON work_source_refs (work_id);
