-- 009_availability_source: 可用性状态来源（R-MAIN-02 复审修复）。
--
-- 位置级操作（disconnect / path-missing / rebind 无效化 / 恢复）不得覆盖
-- 用户/扫描器显式标记（SourceUnavailable / TemporarilyUnavailable / Unknown /
-- 资源自身 Missing）。新增来源列：
--   - 'user'    ：扫描器/用户显式设置，位置级操作绝不触碰；
--   - 'storage' ：位置级自动标记（可被位置级操作归位）；
--   - 'unknown' ：迁移前数据（无来源记录），位置级操作可迁移归位。
-- 发布前无持久库；历史 'unknown' 资源由位置级操作按 'storage' 规则归位。

ALTER TABLE resources ADD COLUMN availability_source TEXT NOT NULL DEFAULT 'unknown';
