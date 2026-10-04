-- 045_interface_font_assets: 界面自定义字体资产（导入字体的元数据 + 原始字节）。
--
-- 该表只服务「外观 → 界面字体 → 导入字体」这一条真实链路：
-- - id 是 opaque UUID，也是唯一被 WebView 与设置 JSON 引用的标识；
--   `appearance.interfaceFontAssetId` 只保存它，**永不保存文件路径**。
-- - bytes 是导入时的原始字体字节，随元数据一起持久化，删除时同事务移除，
--   因此不存在「元数据删了、文件还在」或反过来的中间状态。
-- - extension / mime_type 都是闭合枚举且必须互相匹配；实际校验发生在导入边界
--   （扩展名 + 文件签名 + 大小上限 32 MiB），这里再兜一层数据库约束。
-- - family_name 是展示用的族名（优先取字体 name 表，解析失败时退化为文件名），
--   file_name 只用于展示，任何路径拼接都不允许使用它。
-- - sha256 是导入去重键（同一份文件重复导入复用已有行），非唯一约束：
--   单窗口串行导入下 find-then-insert 足够，重复行即使出现也不影响正确性。
--
-- 只追加：001..044 不在此文件内改动。

CREATE TABLE IF NOT EXISTS interface_font_assets (
    id          TEXT PRIMARY KEY,
    family_name TEXT NOT NULL CHECK (length(family_name) BETWEEN 1 AND 120),
    file_name   TEXT NOT NULL CHECK (length(file_name) BETWEEN 1 AND 260),
    extension   TEXT NOT NULL CHECK (extension IN ('ttf', 'otf', 'woff2')),
    mime_type   TEXT NOT NULL CHECK (mime_type IN ('font/ttf', 'font/otf', 'font/woff2')),
    byte_size   INTEGER NOT NULL CHECK (byte_size > 0 AND byte_size <= 33554432),
    sha256      TEXT NOT NULL CHECK (length(sha256) = 64),
    bytes       BLOB NOT NULL,
    created_at  INTEGER NOT NULL CHECK (created_at >= 0)
);

CREATE INDEX IF NOT EXISTS interface_font_assets_sha256_idx
    ON interface_font_assets (sha256);

CREATE INDEX IF NOT EXISTS interface_font_assets_created_at_idx
    ON interface_font_assets (created_at);
