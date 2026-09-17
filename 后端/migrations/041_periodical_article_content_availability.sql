-- 041_periodical_article_content_availability: 保存 Provider 对文章正文可用性的观察。
--
-- 040 建表时没有这一列，导入时的 Provider 观察被直接丢弃，页面只能看到笼统的
-- 「不可阅读」，无法区分「来源只有元数据」与「没有观察到」。本迁移把这条观察
-- 变成文章的持久化事实。
--
-- 已有行只能保守为 'unknown'：
-- - Provider 的观察无法从本地事实重建（本地没有任何「来源说了什么」的记录）；
-- - 'metadata_only' 是对来源的断言，'full_text' 更是「有可读正文」的断言，
--   把旧行追溯成其中任何一个都是在替来源下没有依据的结论。
--
-- 列是闭合枚举且 NOT NULL：任何未定义的取值、以及 NULL 都写不进去，因此读取端
-- 不需要为「缺失」发明一个状态。
ALTER TABLE periodical_articles
    ADD COLUMN provider_content_availability TEXT NOT NULL DEFAULT 'unknown'
        CHECK (provider_content_availability IN ('full_text', 'metadata_only', 'unknown'));
