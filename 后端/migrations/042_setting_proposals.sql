-- 042_setting_proposals: 设置变更提案与变更回执（V02-SETTING-PROPOSAL-001）。
--
-- 提案把「要改哪一项设置」从一次直接写入变成可版本化、可校验、可审计的领域事实：
-- - payload_json 是 canonical JSON 执行载荷（对象键按 UTF-8 字节序、紧凑输出），
--   digest 是它的 SHA-256 小写十六进制。两者的自洽性每次读取都由领域层重算校验，
--   存储端不提供任何「只信一列」的捷径。
-- - scope / section / edition_id / media_item_id 是**冗余 target 列**，只用于闭合
--   约束与检索；读取时必须与 payload 里的 target 完全一致，不一致即视为行被篡改。
-- - status 是闭合枚举，pending 之外均为终态，任何路径都不会迁移回 pending。
--
-- 目标列刻意**不建外键**：目标被删除时，apply 必须返回稳定的
-- SETTING_PROPOSAL_TARGET_NOT_FOUND，而不是被 CASCADE 连带删除审计事实
-- （那会变成「提案不存在」），也不是被 NO ACTION 反向阻塞删除媒体条目。
-- 提案与回执之间的外键则是硬约束：回执只能属于一条真实存在的提案，且与它 1:1。
--
-- 只追加：001..041 不在此文件内改动。

CREATE TABLE IF NOT EXISTS setting_proposals (
    id            TEXT PRIMARY KEY,
    scope         TEXT NOT NULL CHECK (scope IN ('global', 'edition', 'media_item')),
    section       TEXT CHECK (
        section IS NULL
        OR section IN ('general', 'appearance', 'playback', 'reading', 'comic', 'downloads', 'privacy')
    ),
    edition_id    TEXT,
    media_item_id TEXT,
    payload_json  TEXT NOT NULL,
    digest        TEXT NOT NULL CHECK (length(digest) = 64),
    status        TEXT NOT NULL CHECK (status IN ('pending', 'applied', 'rejected', 'expired')),
    created_at    INTEGER NOT NULL CHECK (created_at >= 0),
    expires_at    INTEGER NOT NULL CHECK (expires_at > created_at),
    updated_at    INTEGER NOT NULL CHECK (updated_at >= 0),
    -- 每个 scope 对应唯一一种 target 形状，多余列必须为 NULL。
    CHECK (
        scope <> 'global'
        OR (section IS NOT NULL AND edition_id IS NULL AND media_item_id IS NULL)
    ),
    CHECK (
        scope <> 'edition'
        OR (section IS NULL AND edition_id IS NOT NULL AND media_item_id IS NULL)
    ),
    CHECK (
        scope <> 'media_item'
        OR (section IS NULL AND edition_id IS NOT NULL AND media_item_id IS NOT NULL)
    )
);

-- 到期清理/待办扫描的读取路径。
CREATE INDEX IF NOT EXISTS idx_setting_proposals_status_expires
    ON setting_proposals(status, expires_at);

CREATE TABLE IF NOT EXISTS setting_change_receipts (
    id               TEXT PRIMARY KEY,
    -- 回执只允许对应一条真实存在的提案，且一条提案最多一张回执。
    -- 回执是 append-only 审计事实：已产生回执的父提案不得被删除，避免级联擦除历史。
    proposal_id      TEXT NOT NULL UNIQUE REFERENCES setting_proposals(id) ON DELETE RESTRICT,
    proposal_digest  TEXT NOT NULL CHECK (length(proposal_digest) = 64),
    scope            TEXT NOT NULL CHECK (scope IN ('global', 'edition', 'media_item')),
    section          TEXT CHECK (
        section IS NULL
        OR section IN ('general', 'appearance', 'playback', 'reading', 'comic', 'downloads', 'privacy')
    ),
    edition_id       TEXT,
    media_item_id    TEXT,
    -- 回执保存的是**值**的 canonical JSON，不是 patch：跨进程可稳定比较与重放。
    before_json      TEXT NOT NULL,
    after_json       TEXT NOT NULL,
    -- changed=0 时这里是未改动的当前版本（目标从未持久化则为 NULL）。
    applied_revision TEXT,
    changed          INTEGER NOT NULL CHECK (changed IN (0, 1)),
    provenance_json  TEXT NOT NULL,
    applied_at       INTEGER NOT NULL CHECK (applied_at >= 0),
    -- 真正改动过就必然写出了一个新版本；没有版本号就不可能是 changed。
    CHECK (changed = 0 OR applied_revision IS NOT NULL),
    CHECK (
        scope <> 'global'
        OR (section IS NOT NULL AND edition_id IS NULL AND media_item_id IS NULL)
    ),
    CHECK (
        scope <> 'edition'
        OR (section IS NULL AND edition_id IS NOT NULL AND media_item_id IS NULL)
    ),
    CHECK (
        scope <> 'media_item'
        OR (section IS NULL AND edition_id IS NOT NULL AND media_item_id IS NOT NULL)
    )
);
