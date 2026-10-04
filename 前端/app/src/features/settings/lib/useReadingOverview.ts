// 阅读总览加载路径（设置页「总览」分组的唯一数据入口）。
//
// 与 useSettingsForm 同一模式：把异步适配拆成可独立断言的纯函数
// （`loadReadingOverview`），Hook 只负责请求身份与状态收口。页面因此永远拿不到
// 一份「手写的」统计——loading / empty / ready / error 四态都由真实聚合决定。

import { useCallback, useEffect, useRef, useState } from "react"
import { toHavenError } from "@/lib/ipc/errors"
import type { ReadingOverviewGateway } from "../ipc/reading-overview-gateway"
import { readingOverviewGateway } from "../ipc/reading-overview-gateway"
import type { ReadingOverviewState } from "./reading-overview"
import {
  hasReadingOverviewData,
  mapReadingOverview,
  readingOverviewRequest,
} from "./reading-overview"

/**
 * 一次总览读取：固定 7 天窗口 + 调用时刻的显式 UTC 偏移。
 *
 * 任何失败都收敛成 error 态（带可展示文案），不会退化成一份空统计——「读不到」
 * 与「没有阅读记录」是两种不同事实，页面上的呈现也不同。
 */
export async function loadReadingOverview(
  gateway: ReadingOverviewGateway,
  now: () => Date = () => new Date(),
): Promise<ReadingOverviewState> {
  try {
    const dto = await gateway.readingOverviewGet(readingOverviewRequest(now()))
    const stats = mapReadingOverview(dto)
    return hasReadingOverviewData(stats) ? { status: "ready", stats } : { status: "empty", stats }
  } catch (error) {
    return { status: "error", message: toHavenError(error).message }
  }
}

export interface ReadingOverviewController {
  state: ReadingOverviewState
  /** 重新读取（error 态的「重试」入口）。 */
  reload: () => void
}

export function useReadingOverview(
  gateway: ReadingOverviewGateway = readingOverviewGateway,
): ReadingOverviewController {
  const [state, setState] = useState<ReadingOverviewState>({ status: "loading" })
  const requestId = useRef(0)

  const load = useCallback(async (): Promise<void> => {
    const id = ++requestId.current
    setState({ status: "loading" })
    const result = await loadReadingOverview(gateway)
    // 只有最后一次请求的结果能落地：重试与首次加载交错时，先发出的那次不得覆盖。
    if (id !== requestId.current) return
    setState(result)
  }, [gateway])

  useEffect(() => {
    void load()
  }, [load])

  return { state, reload: () => void load() }
}
