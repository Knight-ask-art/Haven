// 首页布局的编辑模型（Appearance Stage 1B）。
//
// 事实边界：唯一事实源是 Rust 侧保存的 `HomeLayoutDto`（三个闭合模块 ID、档位、
// 固定 4×12 网格里的坐标）。本模块只做两件事：
//   1. 把已保存的布局归一成一份可编辑的模块清单（只含白名单模块）；
//   2. 把编辑结果重新排成一份**必然合法**的布局（按顺序填充 4 列网格，绝不重叠）。
//
// 刻意不做的事：不接受任意模块 ID（清单是闭合的，未知 ID 直接丢弃而不是塞进草稿），
// 也不把坐标交给用户随便拖——一个越界/重叠的坐标在 Rust 侧是
// `APPEARANCE_INVALID_HOME_LAYOUT`，前端不该生成一份后端必然拒绝的布局。

import type { HomeLayoutWire } from "@/lib/ipc/settings-wire"
import { defaultHomeLayout, guardHomeLayout, homeModuleColumnSpan } from "@/lib/ipc/settings-wire"

export type HomeModuleId = HomeLayoutWire["modules"][number]["module"]
export type HomeModuleSize = HomeLayoutWire["modules"][number]["size"]

/** 允许出现在首页布局里的模块（闭合集合；与 Rust `HomeModuleId` 同集合）。 */
export const HOME_MODULES: ReadonlyArray<{
  id: HomeModuleId
  label: string
  description: string
}> = [
  { id: "continue", label: "继续", description: "最近打开、还没看完的内容。" },
  { id: "recently_added", label: "最近添加", description: "媒体库里最新入库的作品。" },
  { id: "shelf-favorites", label: "收藏", description: "已收藏作品的横向内容架。" },
]

/** 档位（闭合集合）与中文标签。 */
export const HOME_MODULE_SIZES: ReadonlyArray<{ value: HomeModuleSize; label: string }> = [
  { value: "small", label: "紧凑" },
  { value: "medium", label: "标准" },
  { value: "large", label: "通栏" },
]

const HOME_MODULE_IDS: readonly HomeModuleId[] = HOME_MODULES.map((module) => module.id)

/** 一个模块在编辑器里的状态。 */
export interface HomeModuleSetting {
  module: HomeModuleId
  visible: boolean
  size: HomeModuleSize
}

/** 首页桌面网格里一个真实可见的模块位置。 */
export interface HomeModulePlacement extends HomeModuleSetting {
  row: number
  column: number
  order: number
}

export function isHomeModuleId(value: unknown): value is HomeModuleId {
  return typeof value === "string" && (HOME_MODULE_IDS as readonly string[]).includes(value)
}

export function isHomeModuleSize(value: unknown): value is HomeModuleSize {
  return value === "small" || value === "medium" || value === "large"
}

/** 模块中文标签；未知 ID 返回 null（调用方据此走「不渲染」而不是编一个名字）。 */
export function homeModuleLabel(module: HomeModuleId): string {
  return HOME_MODULES.find((entry) => entry.id === module)?.label ?? module
}

/** 编辑器初始化：三个模块都可见、都是中档（与领域默认布局同形）。 */
export function defaultHomeModuleSettings(): HomeModuleSetting[] {
  return HOME_MODULES.map((module) => ({ module: module.id, visible: true, size: "medium" }))
}

/**
 * 已保存布局 → 编辑器清单。
 *
 * - 只保留白名单模块：后端若给出未知模块（不可能，但守卫也不是本函数的输入），
 *   这里也不会把它带进草稿；
 * - 布局里没有的模块进清单并标记为隐藏——「隐藏」是用户能看到的显式状态，
 *   比「从清单里消失」更诚实；
 * - 顺序取布局里按 `order` 升序的实际顺序（可见模块），隐藏模块按白名单顺序追加。
 */
export function layoutToHomeModuleSettings(layout: HomeLayoutWire): HomeModuleSetting[] {
  const source = guardHomeLayout(layout) ? layout : defaultHomeLayout()
  const ordered = [...source.modules].sort((left, right) => left.order - right.order)
  const settings: HomeModuleSetting[] = []
  for (const placement of ordered) {
    if (!isHomeModuleId(placement.module)) continue
    if (settings.some((entry) => entry.module === placement.module)) continue
    settings.push({
      module: placement.module,
      visible: true,
      size: isHomeModuleSize(placement.size) ? placement.size : "medium",
    })
  }
  for (const module of HOME_MODULES) {
    if (settings.some((entry) => entry.module === module.id)) continue
    settings.push({ module: module.id, visible: false, size: "medium" })
  }
  return settings
}

/** 首页真实投影使用后端保存的位置，不再只取顺序后交给 CSS 自动猜位置。 */
export function layoutToHomeModulePlacements(layout: HomeLayoutWire): HomeModulePlacement[] {
  const source = guardHomeLayout(layout) ? layout : defaultHomeLayout()
  const settings = layoutToHomeModuleSettings(source).filter((entry) => entry.visible)
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
 * 可见模块按清单顺序放入 4 列网格，档位决定列跨度，放不下就换到下一行。
 * `order` 保持用户看到的先后顺序；坐标由这一顺序和档位确定。空清单是合法的
 * **显式空布局**（用户隐藏了全部模块），与「从未保存过」由 revision 区分。
 */
export function homeModuleSettingsToLayout(settings: readonly HomeModuleSetting[]): HomeLayoutWire {
  const modules: HomeLayoutWire["modules"] = []
  const seen = new Set<HomeModuleId>()
  const gridColumns = homeModuleColumnSpan("large")
  let row = 0
  let column = 0
  for (const setting of settings) {
    if (!setting.visible) continue
    if (!isHomeModuleId(setting.module) || seen.has(setting.module)) continue
    seen.add(setting.module)
    const order = modules.length
    const size = isHomeModuleSize(setting.size) ? setting.size : "medium"
    const span = homeModuleColumnSpan(size)
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
export function moveHomeModule(
  settings: readonly HomeModuleSetting[],
  module: HomeModuleId,
  delta: number,
): HomeModuleSetting[] {
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

/**
 * 将一个可见模块放到可见模块列表的指定位置。
 *
 * 隐藏模块不参与拖放排序：它们在保存布局时不会发送给后端，因此把它们插进可见顺序
 * 只会制造「看似改了、保存后又恢复」的假反馈。拖放结束后将隐藏项稳定放在列表末尾。
 */
export function reorderHomeModule(
  settings: readonly HomeModuleSetting[],
  module: HomeModuleId,
  targetVisibleIndex: number,
): HomeModuleSetting[] {
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

export function setHomeModuleVisible(
  settings: readonly HomeModuleSetting[],
  module: HomeModuleId,
  visible: boolean,
): HomeModuleSetting[] {
  return settings.map((entry) => (entry.module === module ? { ...entry, visible } : entry))
}

export function setHomeModuleSize(
  settings: readonly HomeModuleSetting[],
  module: HomeModuleId,
  size: HomeModuleSize,
): HomeModuleSetting[] {
  if (!isHomeModuleSize(size)) return [...settings]
  return settings.map((entry) => (entry.module === module ? { ...entry, size } : entry))
}

/** 两份清单是否等价（顺序、可见性、档位都比；用于判断「有没有可保存的改动」）。 */
export function homeModuleSettingsEqual(
  left: readonly HomeModuleSetting[],
  right: readonly HomeModuleSetting[],
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
