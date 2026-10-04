-- 047_reading_activity: 阅读活动的持久化事实（Reading Overview 的唯一来源）。
--
-- 总览页此前只能展示一个硬编码的空结果：库里没有任何「读了多久」的事实。
-- progress / history 只保存**时刻**（last_active_at、started_at、completed_at）
-- 与位置比例，从它们反推分钟数是在编造数据。这一张窄表补上缺的那半：
-- 每次内容会话闭合时记下开始与结束时刻，时长是两者的差，不是独立输入。
--
-- 关键约束：
--   * id 是会话身份的规范小写连字符 UUID，同时是幂等键——同一次会话重复上报
--     （重试、重复 close）只会命中同一行，同一段时间不会被统计两次。
--   * duration 必须是正的，且有 24 小时上限，与 haven-domain 的
--     MAX_SESSION_DURATION_MS 逐值一致：时钟跳变或未正常闭合的会话不该变成
--     一条污染统计的假时长。上限不写成「>= 0」而是「> 0」，因为 0 长的会话
--     不是事实，只是开合发生在同一毫秒。
--   * category 是闭合集合，与 haven-domain 的 ReadingSessionCategory 一致。
--     刻意不含 'all'：一次真实会话必然属于某一类。
--
-- 不引用 media_items / works：阅读统计是应用级聚合事实，删除作品不该连带删除
-- 已经发生的阅读时长，一条悬空的统计行也不会阻塞任何业务删除（与 045 同一理由）。
--
-- 只追加：001..046 不在此文件内改动。

CREATE TABLE IF NOT EXISTS reading_sessions (
    id            TEXT NOT NULL PRIMARY KEY CHECK (
        length(id) = 36
        AND substr(id, 9, 1) = '-'
        AND substr(id, 14, 1) = '-'
        AND substr(id, 19, 1) = '-'
        AND substr(id, 24, 1) = '-'
        AND replace(id, '-', '') NOT GLOB '*[^0-9a-f]*'
        AND length(replace(id, '-', '')) = 32
    ),
    media_item_id TEXT NOT NULL CHECK (length(media_item_id) > 0),
    category      TEXT NOT NULL CHECK (
        category IN ('video', 'book', 'comic', 'periodical')
    ),
    started_at    INTEGER NOT NULL CHECK (started_at >= 0),
    ended_at      INTEGER NOT NULL CHECK (ended_at > started_at),
    created_at    INTEGER NOT NULL CHECK (created_at >= 0),
    -- 与 haven-domain 的 MAX_SESSION_DURATION_MS 一致（24 小时）。
    CHECK (ended_at - started_at <= 86400000)
);

-- 读取路径只按开始时刻的范围扫描（本地日历换算属于领域聚合）。
CREATE INDEX IF NOT EXISTS idx_reading_sessions_started_at
    ON reading_sessions (started_at);
