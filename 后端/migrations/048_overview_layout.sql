-- 048_overview_layout: 设置页「总览」的模块布局。
--
-- 与 045/046 的首页布局是**两张独立的表**，而不是同一张表多一列 kind：
--   * 首页是 4 列网格（045 的越界 CHECK 是 column_index + 1/2/4 <= 4）；
--   * 总览是 3 列网格（这里是 column_index + 1/2/3 <= 3）。
-- 把两者塞进一张表意味着所有 CHECK 与占格触发器都要写成「按 layout kind 分支」的
-- CASE，一旦分支写错就会静默放宽另一侧；而且两者的闭合模块集合完全不同。分开建表让
-- 每一侧的网格不变量都是一个可以逐行读懂的常量。
--
-- 总览为什么是 3 列：总览现有的真实版式是「两行整宽 + 一行 2:1 的图表 + 一行整宽热力图」，
-- 3 是能表达 2:1（medium 2 列 + small 1 列）的最小列数，因此默认布局可以逐屏复现升级前的
-- 观感。四列网格只能给出 2:2 的均分。上限常量与 haven-domain 的诚实来源同步
-- （`appearance.rs`）：网格 12 行 × 3 列、最多 16 个模块、schema 版本 1。
--
-- 只追加：001..047 不在此文件内改动，也不改写任何既有 settings 行。

CREATE TABLE IF NOT EXISTS appearance_overview_modules (
    -- 闭合模块集合：只有 SettingsOverview 真实渲染的五个内容块（页头与阅读统计错误
    -- 横幅不在此列——它们不是可管理的模块，见 haven-domain 的 OverviewModuleId）。
    module_id      TEXT NOT NULL CHECK (
        module_id IN (
            'preferences', 'metrics', 'reading-minutes', 'type-share', 'reading-heatmap'
        )
    ),
    size           TEXT NOT NULL CHECK (size IN ('small', 'medium', 'large')),
    row_index      INTEGER NOT NULL CHECK (row_index >= 0 AND row_index < 12),
    column_index   INTEGER NOT NULL CHECK (column_index >= 0 AND column_index < 3),
    sort_order     INTEGER NOT NULL CHECK (sort_order >= 0 AND sort_order < 16),
    -- 当前只支持 schema 版本 1；新版本必须由新的追加迁移重建表，不能就地放宽。
    schema_version INTEGER NOT NULL CHECK (schema_version = 1),
    updated_at     INTEGER NOT NULL CHECK (updated_at >= 0),
    PRIMARY KEY (module_id),
    -- 排序号在整份布局里唯一：并列的 sort_order 会让「稳定顺序」退化成存储返回顺序
    -- 这个偶然事实（haven-domain 的 OverviewLayout 全局校验同一条）。
    UNIQUE (sort_order),
    -- 档位跨度不得越出右侧边界（small 1 列、medium 2 列、large 整行 3 列；与
    -- haven-domain 的 `OverviewModuleSize::column_span` 同集合）。
    -- 这里刻意**没有** `UNIQUE (row_index, column_index)`，理由与 045 相同：起始格唯一
    -- 严格弱于下面触发器表达的「占用格子不重叠」，两处都写会让 UNIQUE 永远不可达。
    CHECK (
        (size = 'small' AND column_index + 1 <= 3)
        OR (size = 'medium' AND column_index + 2 <= 3)
        OR (size = 'large' AND column_index + 3 <= 3)
    )
);

-- 占用格子不得重叠（按档位跨度展开比较，不只是起始格）。
--
-- 上面的 CHECK 只保证单个模块不越出右侧边界：越界与重叠是两件事。起始格不同也可能压在
-- 同一个格子上——large 从第 1 列开始占满 (0,0)(0,1)(0,2)，small 从第 3 列开始占 (0,2)，
-- 两者的起始格不同，第 3 列却同属两者。
--
-- 判断规则与 haven-domain 的 `OverviewModulePlacement::overlaps` 逐项一致，也与 045 的
-- 首页版本同形（只是把跨度 4 换成 3、网格宽度换成 3）。所有档位都只占一行：引入两行高的
-- 档位必须先由新的追加迁移重建约束，不能在领域里单方面放开。
CREATE TRIGGER IF NOT EXISTS trg_appearance_overview_modules_cell_overlap_insert
BEFORE INSERT ON appearance_overview_modules
FOR EACH ROW
BEGIN
    SELECT RAISE(
        ABORT,
        'appearance_overview_modules: occupied grid cells must not overlap'
    )
    WHERE EXISTS (
        SELECT 1
        FROM appearance_overview_modules AS existing
        WHERE existing.row_index = NEW.row_index
          AND existing.column_index < NEW.column_index + CASE NEW.size
                  WHEN 'small' THEN 1
                  WHEN 'medium' THEN 2
                  ELSE 3
              END
          AND NEW.column_index < existing.column_index + CASE existing.size
                  WHEN 'small' THEN 1
                  WHEN 'medium' THEN 2
                  ELSE 3
              END
    );
END;

-- 同一条不变量在 UPDATE 路径上同样成立。
CREATE TRIGGER IF NOT EXISTS trg_appearance_overview_modules_cell_overlap_update
BEFORE UPDATE OF row_index, column_index, size ON appearance_overview_modules
FOR EACH ROW
BEGIN
    SELECT RAISE(
        ABORT,
        'appearance_overview_modules: occupied grid cells must not overlap'
    )
    WHERE EXISTS (
        SELECT 1
        FROM appearance_overview_modules AS existing
        WHERE existing.module_id <> NEW.module_id
          AND existing.row_index = NEW.row_index
          AND existing.column_index < NEW.column_index + CASE NEW.size
                  WHEN 'small' THEN 1
                  WHEN 'medium' THEN 2
                  ELSE 3
              END
          AND NEW.column_index < existing.column_index + CASE existing.size
                  WHEN 'small' THEN 1
                  WHEN 'medium' THEN 2
                  ELSE 3
              END
    );
END;

-- 与 046 同形：appearance_overview_modules 只保存模块行，空布局无法与「从未保存」区分。
--   * 没有 meta 行：从未自定义，读取侧回落到领域默认布局；
--   * 有 meta 行但没有 module 行：用户显式保存了空布局。
-- revision 是不透明 CAS token，不承载路径、URL 或用户输入。
CREATE TABLE IF NOT EXISTS appearance_overview_layout_meta (
    singleton_id   INTEGER NOT NULL PRIMARY KEY CHECK (singleton_id = 1),
    revision       TEXT NOT NULL CHECK (length(revision) BETWEEN 8 AND 160),
    schema_version INTEGER NOT NULL CHECK (schema_version = 1),
    updated_at     INTEGER NOT NULL CHECK (updated_at >= 0)
);
