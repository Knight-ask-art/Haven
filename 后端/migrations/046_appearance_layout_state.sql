-- 046_appearance_layout_state: 首页布局的显式保存标记与 CAS 版本。
--
-- appearance_home_modules 只保存模块行，因此空布局无法与“从未保存”区分。
-- 单例元数据行解决这个问题：
--   * 没有 meta 行：从未自定义，读取侧回落到领域默认布局；
--   * 有 meta 行但没有 module 行：用户显式保存了空布局。
--
-- 模块明细仍保留在 045 的窄表中，两个表由 Application/Repository 在同一事务内
-- 写入。revision 是不透明 CAS token，不承载路径、URL 或用户输入。

CREATE TABLE IF NOT EXISTS appearance_home_layout_meta (
    singleton_id  INTEGER NOT NULL PRIMARY KEY CHECK (singleton_id = 1),
    revision      TEXT NOT NULL CHECK (length(revision) BETWEEN 8 AND 160),
    schema_version INTEGER NOT NULL CHECK (schema_version = 1),
    updated_at    INTEGER NOT NULL CHECK (updated_at >= 0)
);
