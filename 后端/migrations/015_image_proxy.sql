-- 015_image_proxy: 受控图片代理映射（契约 §36 演进，C1 元数据观感层）。
--
-- - 外部海报 URL 不进 IPC：Work.artwork 只存 haven-resource://image/<id> 受控 URI，
--   本表承担 id → 真实地址的解析（仅注册过的地址可被代理拉取，天然防 SSRF）。
-- - target_url 唯一：同一海报只生成一个稳定 id。
CREATE TABLE IF NOT EXISTS image_proxy (
    id         TEXT PRIMARY KEY,
    target_url TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL
);
