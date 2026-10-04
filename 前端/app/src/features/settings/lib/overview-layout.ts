// 设置页「总览」布局的编辑模型。
//
// 事实边界与首页布局同形（见 home-layout.ts），但作用于**另一份**布局：唯一事实源是
// Rust 侧保存的 `OverviewLayoutDto`（五个闭合模块 ID、档位、固定 3×12 网格里的坐标）。
// 本模块只做两件事：
//   1. 把已保存的布局归一成一份可编辑的模块清单（只含白名单模块）；
//   2. 把编辑结果重新排成一份**必然合法**的布局（按顺序填充 3 列网格，绝不重叠）。
//
// 为什么是 3 列而不是首页的 4 列：总览现有的真实版式里有一行 2:1 的两张图
// （`xl:grid-cols-[minmax(0,1.92fr)_minmax(300px,1fr)]`），3 列是能表达 2:1 的最小网格
// （medium 2 列 + small 1 列）；4 列只能给出均分，默认布局就不再与升级前视觉等价。
//
// 刻意不做的事：不接受任意模块 ID（清单是闭合的，未知 ID 直接丢弃而不是塞进草稿），
// 也不支持自由拖拽摆放——布局的不变量是「占用格子不重叠 + 排序号唯一」，任意坐标随时
// 可能落在两条不变量之外，在 Rust 侧就是 `APPEARANCE_INVALID_OVERVIEW_LAYOUT`。所以这里
// 的位置是**由顺序与档位确定的**：用户用上移/下移决定先后、用档位决定占几列，坐标由
// 打包算法算出。这与首页布局是同一套取舍。

import type { OverviewLayoutWire } from "@/lib/ipc/settings-wire"
import {
  defaultOverviewLayout,
  guardOverviewLayout,
  overviewModuleColumnSpan,
} from "@/lib/ipc/settings-wire"

export type OverviewModuleId = OverviewLayoutWire["modules"][number]["module"]
export type OverviewModuleSize = OverviewLayoutWire["modules"][number]["size"]

/** 允许出现在总览布局里的模块（闭合集合；与 Rust `OverviewModuleId` 同集合）。 */
export const OVERVIEW_MODULES: ReadonlyArray<{
  id: OverviewModuleId
  label: string
  description: string
}> = [
  { id: "preferences", label: "当前偏好", description: "主题、阅读字体、启动页与默认播放倍速四张卡。" },
  { id: "metrics", label: "阅读指标", description: "首选类型、最长连续、日均时长与本周阅读。" },
  { id: "reading-minutes", label: "每日阅读时长", description: "最近 7 天每天的阅读分钟柱状图。" },
  { id: "type-share", label: "作品类型分布", description: "按阅读时长计算的小说、漫画、影视与报刊占比。" },
  { id: "reading-heatmap", label: "阅读时段", description: "最近 7 天按星期与小时着色的阅读热力图。" },
]

/** 档位（闭合集合）与易懂标签；具体宽度由布局示意图表达，不暴露网格比例。 */
export const OVERVIEW_MODULE_SIZES: ReadonlyArray<{ value: OverviewModuleSize; label: string }> = [
  { value: "small", label: "紧凑" },
  { value: "medium", label: "标准" },
  { value: "large", label: "通栏" },
]

const OVERVIEW_MODULE_IDS: readonly OverviewModuleId[] = OVERVIEW_MODULES.map((module) => module.id)

/** 一个模块在编辑器里的状态。 */
export interface OverviewModuleSetting {
  module: OverviewModuleId
  visible: boolean
  size: OverviewModuleSize
}

/** 总览网格里一个真实可见的模块位置。 */
export interface OverviewModulePlacement extends OverviewModuleSetting {
  row: number
  column: number
  order: number
}

export function isOverviewModuleId(value: unknown): value is OverviewModuleId {
  return typeof value === "string" && (OVERVIEW_MODULE_IDS as readonly string[]).includes(value)
}

export function isOverviewModuleSize(value: unknown): value is OverviewModuleSize {
  return value === "small" || value === "medium" || value === "large"
}

/** 模块中文标签；未知 ID 返回 null（调用方据此走「不渲染」而不是编一个名字）。 */
export function overviewModuleLabel(module: OverviewModuleId): string {
  return OVERVIEW_MODULES.find((entry) => entry.id === module)?.label ?? module
}

/**
 * 编辑器初始化：五个模块都可见，档位取**领域默认布局**里的那一档。
 *
 * 不能像首页那样一律给 medium：默认总览里两张图是 medium + small（2:1），一律 medium
 * 会让「未保存过任何自定义」时的编辑器清单与实际渲染的默认布局对不上——用户点一下
 * 保存就会把默认版式改成另一种排法，而那不是他要做的改动。
 */
export function defaultOverviewModuleSettings(): OverviewModuleSetting[] {
  return layoutToOverviewModuleSettings(defaultOverviewLayout())
}

/**
 * 已保存布局 → 编辑器清单。
 *
 * - 只保留白名单模块：后端若给出未知模块，这里也不会把它带进草稿；
 * - 布局里没有的模块进清单并标记为隐藏——「隐藏」是用户能看到的显式状态，
 *   比「从清单里消失」更诚实；
 * - 顺序取布局里按 `order` 升序的实际顺序（可见模块），隐藏模块按白名单顺序追加。
 */
export function layoutToOverviewModuleSettings(layout: OverviewLayoutWire): OverviewModuleSetting[] {
  const source = guardOverviewLayout(layout) ? layout : defaultOverviewLayout()
  const ordered = [...source.modules].sort((left, right) => left.order - right.order)
  const settings: OverviewModuleSetting[] = []
  for (const placement of ordered) {
    if (!isOverviewModuleId(placement.module)) continue
    if (settings.some((entry) => entry.module === placement.module)) continue
    settings.push({
      module: placement.module,
      visible: true,
      size: isOverviewModuleSize(placement.size) ? placement.size : "medium",
    })
  }
  for (const module of OVERVIEW_MODULES) {
    if (settings.some((entry) => entry.module === module.id)) continue
    settings.push({ module: module.id, visible: false, size: "medium" })
  }
  return settings
}

/**
 * 已保存布局 → 渲染用的模块位置。
 *
 * 坐标**原样取自布局**（`row` / `column` / `order` 都是存储里的既成事实），本函数不重算
 * 位置：调用方拿到的就是后端保存的那一份。守卫不过时回落到领域默认布局，与读取侧一致。
 */
export function layoutToOverviewModulePlacements(layout: OverviewLayoutWire): OverviewModulePlacement[] {
  const source = guardOverviewLayout(layout) ? layout : defaultOverviewLayout()
  const settings = layoutToOverviewModuleSettings(source).filter((entry) => entry.visible)
  const placements = new Map(source.modules.map((placement) => [placement.module, placement]))
  return settings.flatMap((setting) => {
    const placement = placements.get(setting.module)
    return placement
      ? [{
          ...setting,
          row: placement.row,
          column: placement.column,
          order: placement.order,
        }]
      : []
  })
}

/**
 * 编辑器清单 → 可提交的布局。
 *
 * 可见模块按清单顺序放入 3 列网格，档位决定列跨度，放不下就换到下一行。
 * `order` 保持用户看到的先后顺序；坐标由这一顺序和档位确定。空清单是合法的
 * **显式空布局**（用户隐藏了全部模块），与「从未保存过」由 revision 区分。
 */
export function overviewModuleSettingsToLayout(
  settings: readonly OverviewModuleSetting[],
): OverviewLayoutWire {
  const modules: OverviewLayoutWire["modules"] = []
  const seen = new Set<OverviewModuleId>()
  const gridColumns = overviewModuleColumnSpan("large")
  let row = 0
  let column = 0
  for (const setting of settings) {
    if (!setting.visible) continue
    if (!isOverviewModuleId(setting.module) || seen.has(setting.module)) continue
    seen.add(setting.module)
    const order = modules.length
    const size = isOverviewModuleSize(setting.size) ? setting.size : "medium"
    const span = overviewModuleColumnSpan(size)
    if (column + span > gridColumns) {
      row += 1
      column = 0
    }
    modules.push({
      module: setting.module,
      size,
      row,
      column,
      order,
    })
    column += span
    if (column === gridColumns) {
      row += 1
      column = 0
    }
  }
  return { schemaVersion: 1, modules }
}

/** 在清单内上/下移动一个模块；越界是空操作（返回原数组，不制造顺序变化）。 */
export function moveOverviewModule(
  settings: readonly OverviewModuleSetting[],
  module: OverviewModuleId,
  delta: number,
): OverviewModuleSetting[] {
  const index = settings.findIndex((entry) => entry.module === module)
  if (index < 0) return [...settings]
  const target = index + delta
  if (target < 0 || target >= settings.length) return [...settings]
  const next = [...settings]
  const [moved] = next.splice(index, 1)
  if (!moved) return [...settings]
  next.splice(target, 0, moved)
  return next
}

/** 将一个可见总览模块放到可见模块列表的指定位置；隐藏项不参与可视排序。 */
export function reorderOverviewModule(
  settings: readonly OverviewModuleSetting[],
  module: OverviewModuleId,
  targetVisibleIndex: number,
): OverviewModuleSetting[] {
  const visible = settings.filter((entry) => entry.visible)
  const hidden = settings.filter((entry) => !entry.visible)
  const sourceIndex = visible.findIndex((entry) => entry.module === module)
  if (sourceIndex < 0 || !Number.isInteger(targetVisibleIndex)) return [...settings]

  const boundedTarget = Math.max(0, Math.min(targetVisibleIndex, visible.length - 1))
  if (boundedTarget === sourceIndex) return [...settings]

  const [moved] = visible.splice(sourceIndex, 1)
  if (!moved) return [...settings]
  visible.splice(boundedTarget, 0, moved)
  return [...visible, ...hidden]
}

export function setOverviewModuleVisible(
  settings: readonly OverviewModuleSetting[],
  module: OverviewModuleId,
  visible: boolean,
): OverviewModuleSetting[] {
  return settings.map((entry) => (entry.module === module ? { ...entry, visible } : entry))
}

export function setOverviewModuleSize(
  settings: readonly OverviewModuleSetting[],
  module: OverviewModuleId,
  size: OverviewModuleSize,
): OverviewModuleSetting[] {
  if (!isOverviewModuleSize(size)) return [...settings]
  return settings.map((entry) => (entry.module === module ? { ...entry, size } : entry))
}

/** 两份清单是否等价（顺序、可见性、档位都比；用于判断「有没有可保存的改动」）。 */
export function overviewModuleSettingsEqual(
  left: readonly OverviewModuleSetting[],
  right: readonly OverviewModuleSetting[],
): boolean {
  if (left.length !== right.length) return false
  return left.every((entry, index) => {
    const other = right[index]
    return other !== undefined
      && entry.module === other.module
      && entry.visible === other.visible
      && entry.size === other.size
  })
}
