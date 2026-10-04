-- 052_cloud_drive_storage: 云盘账户 / 目录 / 对象绑定 + 凭据清理 outbox。
--
-- 边界（只读 Google Drive 切片）：
-- - 只新增表与索引，不修改任何既有表。008 的 `idx_storage_locations_root_unique`
--   （lower(root_ref)）保持原样：云盘位置写入的 root_ref 是**内部 UUID**；真实的
--   Google folder ID 大小写敏感，只保存在 cloud_folder_bindings。
-- - 表内没有、也不允许有 token / access_token / scope / 授权码 / 回调 query 列。
--   账户只保存不透明 `credential_ref`（haven:google_drive:<account_id>），秘密只在
--   CredentialStore。CHECK 只钉住 `haven:<provider>:<profile-id>` 的形状，以及
--   「connected ⇒ credential_ref NOT NULL」（断开墓碑在 keystore 清理完成后才允许清空
--   引用）；拒绝空段 / 空白 / 反斜杠仍由 Rust 侧 `CredentialRef` 解析负责。
-- - Provider ID 一律 COLLATE BINARY：Google ID 大小写敏感，不能被排序规则折叠。
-- - 外键级联只删除绑定行：删对象绑定 / 删 Resource 行时绑定行随之消失（反方向无级联）。
--   移除云盘目录走与本地位置**完全相同**的应用内清理（删除该位置 Resource 与仅由该位置
--   派生的孤儿内容链及其用户状态），但绝不删除远端（Provider）文件，也不影响共享账户
--   凭据、同账户其他目录或仍被其他位置引用的共享内容。
-- - 时间列一律 UTC 毫秒。

CREATE TABLE cloud_accounts (
    id                  TEXT PRIMARY KEY CHECK (length(id) = 36),
    -- 本切片只注册 google_drive：Provider 键在存储层固定，未支持的提供方不得落库。
    provider            TEXT NOT NULL CHECK (provider = 'google_drive'),
    -- Google user.permissionId（与 ports::validate_drive_object_id 同一字母表）。
    provider_account_id TEXT NOT NULL COLLATE BINARY CHECK (
        length(provider_account_id) BETWEEN 1 AND 128
        AND provider_account_id NOT GLOB '*[^A-Za-z0-9_-]*'
    ),
    display_name        TEXT NOT NULL CHECK (length(display_name) BETWEEN 1 AND 120),
    -- 授权代际：connect / disconnect 递增，恒 > 0。
    generation          INTEGER NOT NULL CHECK (generation > 0),
    connected           INTEGER NOT NULL CHECK (connected IN (0, 1)),
    -- 不透明 CredentialRef；断开后保留到 keystore 删除完成（ADR-001 顺序）。
    credential_ref      TEXT CHECK (
        credential_ref IS NULL
        OR (
            length(credential_ref) BETWEEN 1 AND 128
            AND credential_ref GLOB 'haven:google_drive:*'
            AND credential_ref NOT GLOB 'haven:*:*:*'
        )
    ),
    updated_at          INTEGER NOT NULL CHECK (updated_at >= 0),
    -- 连接中的账户必须持有凭据引用；断开墓碑允许 ref 为 NULL（keystore 清理收尾后才清空）。
    CHECK (connected = 0 OR credential_ref IS NOT NULL)
);

CREATE UNIQUE INDEX idx_cloud_accounts_provider_identity
    ON cloud_accounts(provider, provider_account_id COLLATE BINARY);

CREATE TABLE cloud_folder_bindings (
    -- 一个 StorageLocation 最多绑定一个云盘目录；删除位置即解除绑定。
    location_id        TEXT PRIMARY KEY
        REFERENCES storage_locations(id) ON DELETE CASCADE,
    -- 账户删除被拒绝（RESTRICT）：断开是墓碑而不是删除，不允许留下无主目录。
    account_id         TEXT NOT NULL
        REFERENCES cloud_accounts(id) ON DELETE RESTRICT,
    provider_folder_id TEXT NOT NULL COLLATE BINARY CHECK (
        length(provider_folder_id) BETWEEN 1 AND 128
        AND provider_folder_id NOT GLOB '*[^A-Za-z0-9_-]*'
    ),
    -- 内部 UUID（绝不写 Provider ID：真实 ID 大小写敏感，会被 lower(root_ref) 折叠）。
    root_ref           TEXT NOT NULL CHECK (length(root_ref) = 36),
    created_at         INTEGER NOT NULL CHECK (created_at >= 0)
);

CREATE UNIQUE INDEX idx_cloud_folder_bindings_identity
    ON cloud_folder_bindings(account_id, provider_folder_id COLLATE BINARY);

CREATE TABLE cloud_object_bindings (
    -- 内部对象 UUID；即 ResourceLocator::StorageObject.object_id（绝不是 file ID）。
    id               TEXT PRIMARY KEY CHECK (length(id) = 36),
    location_id      TEXT NOT NULL
        REFERENCES cloud_folder_bindings(location_id) ON DELETE CASCADE,
    provider_file_id TEXT NOT NULL COLLATE BINARY CHECK (
        length(provider_file_id) BETWEEN 1 AND 128
        AND provider_file_id NOT GLOB '*[^A-Za-z0-9_-]*'
    ),
    display_name     TEXT NOT NULL CHECK (length(display_name) BETWEEN 1 AND 300),
    -- 非零且 ≤ 128 MiB（CLOUD_OBJECT_MAX_BYTES）：只读单文件 PDF。
    size_bytes       INTEGER NOT NULL CHECK (size_bytes > 0 AND size_bytes <= 134217728),
    -- 一行 Resource 只允许一个对象绑定；删除 Resource 即解除绑定（不反向删内容）。
    resource_id      TEXT NOT NULL UNIQUE REFERENCES resources(id) ON DELETE CASCADE,
    media_item_id    TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
    created_at       INTEGER NOT NULL CHECK (created_at >= 0)
);

CREATE UNIQUE INDEX idx_cloud_object_bindings_identity
    ON cloud_object_bindings(location_id, provider_file_id COLLATE BINARY);

CREATE INDEX idx_cloud_object_bindings_media_item
    ON cloud_object_bindings(media_item_id);

-- 凭据清理 outbox：stage（store.set 之前）→ keystore delete → finish_credential_cleanup。
-- 只有作用域内的不透明 ref，没有任何秘密材料。
CREATE TABLE cloud_credential_cleanup (
    credential_ref TEXT PRIMARY KEY CHECK (
        length(credential_ref) BETWEEN 1 AND 128
        AND credential_ref GLOB 'haven:google_drive:*'
        AND credential_ref NOT GLOB 'haven:*:*:*'
    ),
    staged_at      INTEGER NOT NULL CHECK (staged_at >= 0),
    cleanup_ready  INTEGER NOT NULL DEFAULT 0 CHECK (cleanup_ready IN (0, 1))
);
