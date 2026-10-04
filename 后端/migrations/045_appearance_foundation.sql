-- 045_appearance_foundation: 应用外观（Appearance）Stage 1A 的规范化事实表。
--
-- 只新增两张窄表，**不使用 JSON 承载**外观事实：
-- - appearance_assets：外观资产（字体 / 静态壁纸 / 动态壁纸）的元数据。
--   资产 ID 是规范小写连字符 UUID，**不是路径也不是 URL**；体积按种类分档设上限，
--   展示名有界且必须已修剪、且不含控制字符或格式/双向控制字符；只有 validated 才
--   代表字节真的通过了校验。
-- - appearance_home_modules：首页模块布局。模块 ID 与档位是闭合集合，坐标落在固定
--   网格内，模块、**占用格子**（含档位跨度）与排序号各自唯一——同一个首页不可能出现
--   两个同位置模块，也不可能出现两个并列排序号（稳定顺序必须是库里的既成事实，
--   而不是读取时的巧合）。
--
-- 两张表都不引用其他业务表：外观配置是应用级事实，删除作品/版本/资源既不该连带
-- 删除外观配置，外观配置也不该反向阻塞业务删除。
--
-- 上限常量与 haven-domain 的诚实来源保持同步（`appearance.rs`）：
--   字体 32 MiB、静态壁纸 32 MiB、动态壁纸 256 MiB、展示名 120 字符、
--   首页网格 12 行 × 4 列、最多 16 个模块、schema 版本 1。
--
-- 只追加：001..044 不在此文件内改动，也不改写任何既有 settings 行。

CREATE TABLE IF NOT EXISTS appearance_assets (
    -- NOT NULL 是必须的：SQLite 的 rowid 表不会为 TEXT 主键隐式加 NOT NULL，
    -- 而 NULL 会让下面这个形状 CHECK 静默通过（CHECK 结果为 NULL 视为满足）。
    id               TEXT NOT NULL PRIMARY KEY CHECK (
        length(id) = 36
        AND substr(id, 9, 1) = '-'
        AND substr(id, 14, 1) = '-'
        AND substr(id, 19, 1) = '-'
        AND substr(id, 24, 1) = '-'
        AND replace(id, '-', '') NOT GLOB '*[^0-9a-f]*'
        AND length(replace(id, '-', '')) = 32
    ),
    kind             TEXT NOT NULL CHECK (
        kind IN ('font', 'static_wallpaper', 'dynamic_wallpaper')
    ),
    validation_state TEXT NOT NULL CHECK (
        validation_state IN ('pending', 'validated', 'rejected')
    ),
    byte_size        INTEGER NOT NULL CHECK (byte_size > 0),
    -- 展示名是可选的展示事实：要么为 NULL，要么是去首尾空白后仍非空的有界文本。
    --
    -- 这一列的判断必须与 haven-domain 的 `bounded_display_name` 是同一组结论：
    --   * 首尾空白 = Unicode White_Space 全集（与 Rust `str::trim` 同一集合）。
    --     SQLite 默认的 trim(X) 只去半角空格，会把「NBSP / 全角空格包起来的名字」
    --     当成落库形态接受，而领域在写入前已经把它们修剪掉——两端语义因此会漂移，
    --     所以这里显式给出码位集合：0x09..0x0d、0x20、0x85、0xa0、0x1680、
    --     0x2000..0x200a、0x2028、0x2029、0x202f、0x205f、0x3000。
    --     0x09..0x0d 已被下面的控制字符检查拒绝，写进集合只是让两端集合逐项一致。
    --   * 控制字符 = C0（0x00..0x1f）与 DEL（0x7f）任何位置都拒绝
    --     （与 `has_opaque_control_character` 同一集合）。0x00 单独用 instr 覆盖，
    --     因为 GLOB 的字符类模式承载不了 NUL。
    --   * 格式/双向控制字符 = U+200B..U+200F、U+2028、U+2029、U+202A..U+202E、
    --     U+2060、U+2066..U+2069、U+FEFF（18 个码位，与 haven-domain 的
    --     `has_display_name_format_control` 逐项一致）任何位置都拒绝。它们不打印任何
    --     可见字形，却能伪造视觉顺序或隐藏可见文本：不加这条，库里就能存下两个在界面上
    --     渲染成同一串字的展示名（Trojan Source 那一类）。
    --     落到这里的必须是**修剪后**的形态，所以 U+2028/U+2029 同时属于上面的
    --     White_Space 集合：它们在首尾只可能被 trim 掉（未修剪的形态本来就因为
    --     `display_name = trim(...)` 而被拒绝），只有出现在中间才由本行拒绝——
    --     与领域「先修剪、再查字符」的顺序结论一致。
    display_name     TEXT CHECK (
        display_name IS NULL
        OR (
            length(display_name) BETWEEN 1 AND 120
            AND display_name = trim(display_name, char(
                9, 10, 11, 12, 13, 32, 133, 160, 5760,
                8192, 8193, 8194, 8195, 8196, 8197, 8198, 8199, 8200, 8201, 8202,
                8232, 8233, 8239, 8287, 12288
            ))
            AND instr(display_name, char(0)) = 0
            AND display_name NOT GLOB (
                '*[' || char(1) || '-' || char(31) || char(127) || ']*'
            )
            AND display_name NOT GLOB (
                '*[' || char(8203) || '-' || char(8207)
                      || char(8232) || char(8233)
                      || char(8234) || '-' || char(8238)
                      || char(8288)
                      || char(8294) || '-' || char(8297)
                      || char(65279) || ']*'
            )
        )
    ),
    created_at       INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at       INTEGER NOT NULL CHECK (updated_at >= 0),
    CHECK (updated_at >= created_at),
    -- 体积上限按种类分档；三种情况穷尽闭合的 kind。
    CHECK (
        (kind = 'font' AND byte_size <= 33554432)
        OR (kind = 'static_wallpaper' AND byte_size <= 33554432)
        OR (kind = 'dynamic_wallpaper' AND byte_size <= 268435456)
    )
);

-- 资产注册表的读取路径：按种类与校验状态筛选可用资产。
CREATE INDEX IF NOT EXISTS idx_appearance_assets_kind_state
    ON appearance_assets(kind, validation_state);

CREATE TABLE IF NOT EXISTS appearance_home_modules (
    -- 闭合模块集合：只有当前真实存在的三个首页模块，不引入仅服务于夹具的 ID。
    module_id      TEXT NOT NULL CHECK (
        module_id IN ('continue', 'recently_added', 'shelf-favorites')
    ),
    size           TEXT NOT NULL CHECK (size IN ('small', 'medium', 'large')),
    row_index      INTEGER NOT NULL CHECK (row_index >= 0 AND row_index < 12),
    column_index   INTEGER NOT NULL CHECK (column_index >= 0 AND column_index < 4),
    sort_order     INTEGER NOT NULL CHECK (sort_order >= 0 AND sort_order < 16),
    -- 当前只支持 schema 版本 1；新版本必须由新的追加迁移重建表，不能就地放宽。
    schema_version INTEGER NOT NULL CHECK (schema_version = 1),
    updated_at     INTEGER NOT NULL CHECK (updated_at >= 0),
    PRIMARY KEY (module_id),
    -- 排序号在整份布局里唯一：并列的 sort_order 会让「稳定顺序」退化成
    -- 存储返回顺序这个偶然事实（haven-domain 的 HomeLayout 全局校验同一条）。
    UNIQUE (sort_order),
    -- 这里刻意**没有** `UNIQUE (row_index, column_index)`：起始格唯一严格弱于下面触发器
    -- 表达的「占用格子不重叠」，而 BEFORE 触发器先于唯一性检查执行，两处都写会让
    -- UNIQUE 永远不可达，只会把「为什么被拒绝」变成执行顺序的产物。
    -- 档位跨度不得越出右侧边界（small 1 列、medium 2 列、large 整行 4 列；
    -- 与 haven-domain 的 `HomeModuleSize::column_span` 同集合）。
    CHECK (
        (size = 'small' AND column_index + 1 <= 4)
        OR (size = 'medium' AND column_index + 2 <= 4)
        OR (size = 'large' AND column_index + 4 <= 4)
    )
);

-- 占用格子不得重叠（按档位跨度展开比较，不只是起始格）。
--
-- 上面的 CHECK 只保证单个模块不越出右侧边界：越界与重叠是两件事。起始格不同也可能
-- 压在同一个格子上——medium 从第 1 列开始占 (0,0)(0,1)，small 从第 2 列开始占 (0,1)，
-- 两者的起始格不同，第 2 列却同时属于两者。
--
-- SQLite 的列 CHECK 不能带子查询，所以这条不变量由触发器表达。判断规则与
-- haven-domain 的 `HomeModulePlacement::overlaps` 逐项一致：
--   行区间相交（当前所有档位的行跨度都是 1，因此就是 row_index 相等）
--   且列区间相交（existing.column_index < NEW.column_index + span(NEW)
--                 且 NEW.column_index < existing.column_index + span(existing)）。
-- 所有档位都只占一行，行跨度是 1：引入两行高的档位必须先由新的追加迁移重建约束，
-- 不能在领域里单方面放开。
--
-- 表本身被 `sort_order < 16` 与 `UNIQUE (sort_order)` 限死在 16 行以内，触发器的
-- EXISTS 即使全表扫描也只是常数规模，因此不需要为它额外建索引。
CREATE TRIGGER IF NOT EXISTS trg_appearance_home_modules_cell_overlap_insert
BEFORE INSERT ON appearance_home_modules
FOR EACH ROW
BEGIN
    SELECT RAISE(
        ABORT,
        'appearance_home_modules: occupied grid cells must not overlap'
    )
    WHERE EXISTS (
        SELECT 1
        FROM appearance_home_modules AS existing
        WHERE existing.row_index = NEW.row_index
          AND existing.column_index < NEW.column_index + CASE NEW.size
                  WHEN 'small' THEN 1
                  WHEN 'medium' THEN 2
                  ELSE 4
              END
          AND NEW.column_index < existing.column_index + CASE existing.size
                  WHEN 'small' THEN 1
                  WHEN 'medium' THEN 2
                  ELSE 4
              END
    );
END;

-- 同一条不变量在 UPDATE 路径上同样成立：移动模块或改变档位不能让两个模块压在同一
-- 格上。只在坐标/档位真的改变时触发，改 updated_at 不触发（表上唯一的非坐标写入）。
CREATE TRIGGER IF NOT EXISTS trg_appearance_home_modules_cell_overlap_update
BEFORE UPDATE OF row_index, column_index, size ON appearance_home_modules
FOR EACH ROW
BEGIN
    SELECT RAISE(
        ABORT,
        'appearance_home_modules: occupied grid cells must not overlap'
    )
    WHERE EXISTS (
        SELECT 1
        FROM appearance_home_modules AS existing
        WHERE existing.module_id <> NEW.module_id
          AND existing.row_index = NEW.row_index
          AND existing.column_index < NEW.column_index + CASE NEW.size
                  WHEN 'small' THEN 1
                  WHEN 'medium' THEN 2
                  ELSE 4
              END
          AND NEW.column_index < existing.column_index + CASE existing.size
                  WHEN 'small' THEN 1
                  WHEN 'medium' THEN 2
                  ELSE 4
              END
    );
END;

-- 这里刻意**不建** `idx_appearance_home_modules_order`：`UNIQUE (sort_order)` 已经
-- 建好排序键上的索引，按 sort_order 读取本来就走它。再声明一个同列索引不会更安全，
-- 只会多一份写入与维护成本，并让「哪个索引才是读取路径真正用的」变得含糊。
