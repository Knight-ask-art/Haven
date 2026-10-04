// 总览布局 → 渲染器投影。
//
// 单独成模块而不是挂在 SettingsPage 上：那个文件只导出组件，把普通函数混进去会让
// `react/only-export-components` 报警（也会破坏该文件的 fast-refresh 边界）。这个投影
// 本身与 React 无关，放在 lib 里可以被单独断言——「读不到布局时说清楚是默认布局」
// 这类语义值得有自己的测试，而不是只能通过挂载整页来验证。

import { defaultOverviewLayout } from "@/lib/ipc/settings-wire"
import {
  layoutToOverviewModulePlacements,
  type OverviewModulePlacement,
} from "./overview-layout"
import type { OverviewLayoutEditorState } from "./useOverviewLayout"

/**
 * 总览渲染器看到的那一份布局投影。
 *
 * 三种状态各有明确含义，绝不互相冒充：
 * - `loading`：还没读到布局，此时**不渲染任何模块**（按默认布局先画一遍等于在用户读到
 *   事实之前替它下结论）；
 * - `error`：读取失败。这时渲染的是**领域默认布局**，并且必须由横幅说清「这是默认布局、
 *   不是你保存的排列」——静默按默认渲染会让用户以为自己隐藏的模块又回来了；
 * - `ready`：按**已保存**的排列渲染。用的是 `savedLayout`（后端保存的原始布局，含坐标），
 *   而不是编辑器草稿：草稿还没落盘，总览不该显示一份磁盘上不存在的排列。
 *   `stale` 表示当前 revision 已过期（其他窗口保存过），页面据此提示「排列可能已不是
 *   最新」，而不是假称它是最新的。
 */
export type OverviewLayoutProjection =
  | { status: "loading" }
  | { status: "error"; message: string; placements: OverviewModulePlacement[] }
  | { status: "ready"; placements: OverviewModulePlacement[]; stale: boolean }

export function overviewLayoutProjection(state: OverviewLayoutEditorState): OverviewLayoutProjection {
  if (state.status === "loading") return { status: "loading" }
  if (state.status === "error") {
    return {
      status: "error",
      message: state.message ?? "总览布局读取失败",
      placements: layoutToOverviewModulePlacements(defaultOverviewLayout()),
    }
  }
  // 直接用**存储里的坐标**，不从模块清单重算：清单是有损投影，重算出来的排列只与后端
  // 保存的那一份在前端是唯一写入方时才恰好相同。
  return {
    status: "ready",
    placements: layoutToOverviewModulePlacements(state.savedLayout),
    stale: state.status === "conflict",
  }
}
