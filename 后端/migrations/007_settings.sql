-- 007_settings: Settings 持久化（BE-SETTINGS-001）
--
-- 按 Section 存储：每个 Section 独立一行（schema_version + revision + data_json）。
-- - revision：状态版本（实际变化时新生成；相同值重复更新幂等返回当前 revision）。
-- - Secret 禁止进入 data_json（凭据走 CredentialStore，只存 credential_ref）。

CREATE TABLE IF NOT EXISTS settings (
    section         TEXT PRIMARY KEY,
    schema_version  INTEGER NOT NULL,
    revision        TEXT NOT NULL,
    data_json       TEXT NOT NULL,
    updated_at      INTEGER NOT NULL
);
