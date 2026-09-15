-- 040_periodicals: 报刊（期刊）层级：期刊 → 卷 → 期 → 文章。
--
-- 归属是逐级外键，不是通用 parent_id 复用：periodicals.work_id 绑定期刊自身的
-- Work，periodical_volumes.periodical_id、periodical_issues.volume_id、
-- periodical_articles.issue_id 逐级约束。文章通过 media_item_id 复用既有
-- MediaItem，因此本迁移不重复保存阅读、进度或资源事实。
--
-- 身份：
-- - 期刊身份是 ISSN（print / electronic 两个独立列，各自唯一，至少一个非 NULL）；
-- - 卷/期的身份是 identity_key（数值卷号/期号的定点表示、规范化标签，或来源未
--   给出时的 'unassigned'）。SQLite 把 NULL 视为互不相同的唯一键值，因此唯一
--   约束必须落在 identity_key 而不是可空的 label/number 列上；
-- - 文章来源身份 (source_key, remote_article_id) 唯一，保证重复导入幂等；
-- - media_item_id 唯一：一篇文章的 MediaItem 只能被一条文章行绑定。
--
-- 不保存 URL、Cookie、grant、请求头或本地路径。
CREATE TABLE periodicals (
    id              TEXT PRIMARY KEY,
    work_id         TEXT NOT NULL REFERENCES works(id) ON DELETE CASCADE,
    title           TEXT NOT NULL,
    issn_print      TEXT,
    issn_electronic TEXT,
    publisher       TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    CHECK (length(trim(title)) > 0),
    CHECK (issn_print IS NOT NULL OR issn_electronic IS NOT NULL),
    CHECK (issn_print IS NULL OR issn_electronic IS NULL OR issn_print <> issn_electronic)
);

CREATE UNIQUE INDEX idx_periodicals_work ON periodicals (work_id);
CREATE UNIQUE INDEX idx_periodicals_issn_print
    ON periodicals (issn_print) WHERE issn_print IS NOT NULL;
CREATE UNIQUE INDEX idx_periodicals_issn_electronic
    ON periodicals (issn_electronic) WHERE issn_electronic IS NOT NULL;

CREATE TABLE periodical_volumes (
    id            TEXT PRIMARY KEY,
    periodical_id TEXT NOT NULL REFERENCES periodicals(id) ON DELETE CASCADE,
    label         TEXT,
    number        REAL,
    year          INTEGER,
    ordinal       INTEGER NOT NULL CHECK (ordinal >= 0),
    identity_key  TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    CHECK (length(trim(identity_key)) > 0),
    CHECK (number IS NULL OR number >= 0),
    CHECK (year IS NULL OR (year BETWEEN 1000 AND 2999)),
    UNIQUE (periodical_id, identity_key)
);

CREATE INDEX idx_periodical_volumes_periodical
    ON periodical_volumes (periodical_id, ordinal, id);

CREATE TABLE periodical_issues (
    id               TEXT PRIMARY KEY,
    volume_id        TEXT NOT NULL REFERENCES periodical_volumes(id) ON DELETE CASCADE,
    label            TEXT,
    number           REAL,
    publication_date TEXT,
    ordinal          INTEGER NOT NULL CHECK (ordinal >= 0),
    identity_key     TEXT NOT NULL,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL,
    CHECK (length(trim(identity_key)) > 0),
    CHECK (number IS NULL OR number >= 0),
    UNIQUE (volume_id, identity_key)
);

CREATE INDEX idx_periodical_issues_volume
    ON periodical_issues (volume_id, ordinal, id);

CREATE TABLE periodical_articles (
    id                TEXT PRIMARY KEY,
    issue_id          TEXT NOT NULL REFERENCES periodical_issues(id) ON DELETE CASCADE,
    media_item_id     TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
    ordinal           INTEGER CHECK (ordinal IS NULL OR ordinal >= 0),
    title             TEXT NOT NULL,
    doi               TEXT,
    page_range_start  TEXT,
    page_range_end    TEXT,
    source_key        TEXT NOT NULL,
    remote_article_id TEXT NOT NULL,
    created_at        INTEGER NOT NULL,
    updated_at        INTEGER NOT NULL,
    CHECK (length(trim(title)) > 0),
    CHECK (length(trim(source_key)) > 0),
    CHECK (length(trim(remote_article_id)) > 0),
    CHECK (page_range_end IS NULL OR page_range_start IS NOT NULL),
    UNIQUE (source_key, remote_article_id)
);

CREATE INDEX idx_periodical_articles_issue
    ON periodical_articles (issue_id, ordinal, id);
-- 文章内容身份与 MediaItem 必须 1:1：同一 MediaItem 被两条文章行同时宣称，
-- 会让阅读、进度与资源归属出现两个互相矛盾的期刊归属。
CREATE UNIQUE INDEX idx_periodical_articles_media_item
    ON periodical_articles (media_item_id);
