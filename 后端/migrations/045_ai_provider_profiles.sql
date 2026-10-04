-- 045_ai_provider_profiles: AI Provider Profile（非敏感配置）+ 模型发现前置。
--
-- 安全边界（docs/architecture/AI_SYSTEM.md §3、§4）：
-- - 本表**只**存非敏感配置；API key 只存在于 CredentialStore（haven:ai:<profile_id>）。
-- - 因此没有任何列可以承载 secret、credential_ref 或 target 名。
-- - 表约束把"能在 Rust 侧通过校验的形状"同时钉在数据库层：
--   profile_id 只允许 ASCII 小写字母/数字/_/-，kind 是闭合集合，
--   revision 非空（CAS 条件写的基础），端点必须是 http(s) 开头。
-- 幂等：CREATE TABLE / INDEX 均带 IF NOT EXISTS；不触碰任何既有表。
-- 不破坏已有设置：settings 分区、来源、凭据引用都不在本迁移范围内。

CREATE TABLE IF NOT EXISTS ai_provider_profiles (
    profile_id        TEXT PRIMARY KEY CHECK (
        length(profile_id) BETWEEN 1 AND 60
        AND profile_id GLOB '[A-Za-z0-9]*'
        AND profile_id NOT GLOB '*[^A-Za-z0-9_-]*'
    ),
    kind              TEXT NOT NULL CHECK (kind IN ('openai_compatible')),
    display_name      TEXT NOT NULL CHECK (length(display_name) BETWEEN 1 AND 80),
    endpoint          TEXT NOT NULL CHECK (
        length(endpoint) BETWEEN 1 AND 2048
        AND (endpoint LIKE 'http://%' OR endpoint LIKE 'https://%')
    ),
    enabled           INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    selected_model_id TEXT CHECK (
        selected_model_id IS NULL
        OR length(selected_model_id) BETWEEN 1 AND 200
    ),
    revision          TEXT NOT NULL CHECK (length(revision) BETWEEN 1 AND 128),
    created_at        INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at        INTEGER NOT NULL CHECK (updated_at >= created_at)
);

CREATE INDEX IF NOT EXISTS idx_ai_provider_profiles_enabled
    ON ai_provider_profiles(enabled, profile_id);
