// 期刊树的读取状态。
//
// 树不是「一个可能为 null 的 DTO」，而是「为某个 workId 读到的一份事实」：这两件事
// 必须绑在一起判断。否则 workId 变了之后，上一棵树以及由它派生的阅读能力会继续被
// 当成当前作品的事实——路由已经指向别的作品，屏幕上却还是上一个作品的层级，旧树里
// 同 mediaItemId 的可读结论还会外溢到新作品的正文入口上。
//
// 因此每一份已落地的事实都带着它属于哪个 workId：请求开始会立刻把上一份事实换成
// pending，请求失败只会留下属于该 workId 的错误——页面不会出现「层级还在，但请求
// 已经在途」的中间态。

import type { HavenError } from "@/lib/ipc/errors"
import type { PeriodicalTreeDto } from "@/lib/ipc/generated/wire"

export type PeriodicalTreeLoad =
  | { status: "pending" }
  | { status: "ready"; workId: string; tree: PeriodicalTreeDto }
  | { status: "failed"; workId: string; error: HavenError }

/**
 * 当前路由还没有落地任何事实：请求在途，或者上一份事实属于别的作品、新请求还没开始。
 *
 * 是单例，重复作废不会触发多余的重渲染。
 */
export const treeLoadPending: PeriodicalTreeLoad = { status: "pending" }

/** 成功落地：这份树明确是 `workId` 读到的。 */
export function treeLoadReady(workId: string, tree: PeriodicalTreeDto): PeriodicalTreeLoad {
  return { status: "ready", workId, tree }
}

/** 失败落地：错误同样属于 `workId`，不能冒充别的作品的失败。 */
export function treeLoadFailed(workId: string, error: HavenError): PeriodicalTreeLoad {
  return { status: "failed", workId, error }
}

export type PeriodicalTreeLoadView =
  | { state: "pending" }
  | { state: "failed"; error: HavenError }
  | { state: "ready"; tree: PeriodicalTreeDto }

/**
 * 页面此刻可以把哪一份已落地的事实当作当前 `workId` 的期刊层级。
 *
 * 只有「就是为这个 workId 读到的」响应才算当前事实。workId 变了之后，上一棵树的卷期
 * 与文章、以及由它派生的阅读能力都不再属于当前路由：这里判成 pending（正在读取），
 * 既不沿用旧树，也不把它伪装成空树或加载成功——作废旧事实与「后端确认没有卷期」是
 * 两回事，后者是 ready 且 `volumes` 为空。
 */
export function resolvePeriodicalTreeLoadView(
  load: PeriodicalTreeLoad,
  workId: string | undefined,
): PeriodicalTreeLoadView {
  if (load.status === "pending") return { state: "pending" }
  if (workId === undefined || load.workId !== workId) return { state: "pending" }
  if (load.status === "failed") return { state: "failed", error: load.error }
  return { state: "ready", tree: load.tree }
}
