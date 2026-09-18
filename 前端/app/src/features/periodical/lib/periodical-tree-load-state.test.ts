import { describe, expect, it } from "vitest"
import { HavenError } from "@/lib/ipc/errors"
import type { PeriodicalTreeDto } from "@/lib/ipc/generated/wire"
import {
  resolvePeriodicalTreeLoadView,
  treeLoadFailed,
  treeLoadPending,
  treeLoadReady,
} from "./periodical-tree-load-state"

const WORK_A = "11111111-1111-4111-8111-111111111111"
const WORK_B = "11111111-1111-4111-8111-111111111112"
const PERIODICAL_A = "22222222-2222-4222-8222-222222222201"

function treeFor(workId: string, volumes: PeriodicalTreeDto["volumes"] = []): PeriodicalTreeDto {
  return {
    schemaVersion: 1,
    workId,
    periodical: {
      id: PERIODICAL_A,
      workId,
      title: "示范期刊",
      issnPrint: "1234-5679",
      issnElectronic: null,
      publisher: null,
    },
    volumes,
  }
}

describe("resolvePeriodicalTreeLoadView", () => {
  it("shows what was read for the current work, including a legitimately empty tree", () => {
    const tree = treeFor(WORK_A)
    expect(resolvePeriodicalTreeLoadView(treeLoadReady(WORK_A, tree), WORK_A))
      .toEqual({ state: "ready", tree })

    // 后端确认没有卷期是合法结果，必须是 ready；旧事实被作废是 pending。两者不能合并。
    const empty = treeFor(WORK_A, [])
    expect(resolvePeriodicalTreeLoadView(treeLoadReady(WORK_A, empty), WORK_A))
      .toEqual({ state: "ready", tree: empty })
    expect(resolvePeriodicalTreeLoadView(treeLoadPending, WORK_A)).toEqual({ state: "pending" })
  })

  it("refuses to show the previous work's tree on the next work's route", () => {
    // 这是这次修复的核心：workId 已经变了而新请求还没开始的那一帧里，上一棵树不是
    // 当前作品的事实——它既不能被渲染，也不能被当成空树或加载成功。
    const previous = treeLoadReady(WORK_A, treeFor(WORK_A))

    expect(resolvePeriodicalTreeLoadView(previous, WORK_B)).toEqual({ state: "pending" })
  })

  it("refuses to show the previous work's failure on the next work's route", () => {
    const error = new HavenError({ code: "IPC_UNAVAILABLE", userMessage: "读取失败", retryable: true })
    const previous = treeLoadFailed(WORK_A, error)

    // 错误同样是某个 workId 的事实：不能拿上一个作品的失败解释当前作品。
    expect(resolvePeriodicalTreeLoadView(previous, WORK_B)).toEqual({ state: "pending" })
    expect(resolvePeriodicalTreeLoadView(previous, WORK_A)).toEqual({ state: "failed", error })
  })

  it("stays pending while no response has landed for the current work", () => {
    expect(resolvePeriodicalTreeLoadView(treeLoadPending, WORK_A)).toEqual({ state: "pending" })
    expect(resolvePeriodicalTreeLoadView(treeLoadPending, WORK_B)).toEqual({ state: "pending" })
  })

  it("stays pending when the route carries no work identity at all", () => {
    expect(resolvePeriodicalTreeLoadView(treeLoadPending, undefined)).toEqual({ state: "pending" })
    expect(resolvePeriodicalTreeLoadView(treeLoadReady(WORK_A, treeFor(WORK_A)), undefined))
      .toEqual({ state: "pending" })
    expect(resolvePeriodicalTreeLoadView(
      treeLoadFailed(WORK_A, new HavenError({ code: "IPC_UNAVAILABLE", userMessage: "读取失败", retryable: true })),
      undefined,
    )).toEqual({ state: "pending" })
  })

  it("passes the tree instance through untouched", () => {
    const tree = treeFor(WORK_A)
    const view = resolvePeriodicalTreeLoadView(treeLoadReady(WORK_A, tree), WORK_A)

    // 同一份事实只解析一次：页面渲染与能力查询必须看到同一个对象。
    expect(view.state === "ready" && view.tree).toBe(tree)
  })
})
