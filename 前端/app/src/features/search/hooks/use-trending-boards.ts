import { useCallback, useEffect, useRef, useState } from "react"
import { HavenError } from "@/lib/ipc/errors"
import type { TrendingBoardsDto } from "@/lib/ipc/generated/wire"

import { getTrendingBoards, refreshTrendingBoards } from "../ipc/trending-gateway"

export type TrendingLoadStatus = "loading" | "ready" | "refreshing" | "error"

export interface TrendingState {
  boards: TrendingBoardsDto | null
  status: TrendingLoadStatus
  error: HavenError | null
  retryAvailableAt: number | null
}

const REFRESH_COOLDOWN_MS = 60_000

/**
 * 搜索空态的本地快照优先加载器：
 * Query 先返回可用快照，随后 Refresh 替换它；旧请求不能覆盖新搜索状态。
 */
export function useTrendingBoards(active: boolean): TrendingState & { retry: () => void } {
  const [state, setState] = useState<TrendingState>({
    boards: null,
    status: "loading",
    error: null,
    retryAvailableAt: null,
  })
  const [retryRevision, setRetryRevision] = useState(0)
  const generation = useRef(0)

  useEffect(() => {
    const currentGeneration = ++generation.current
    if (!active) {
      setState({ boards: null, status: "loading", error: null, retryAvailableAt: null })
      return
    }

    let cancelled = false
    setState((current) => ({
      ...current,
      status: current.boards?.boards.length ? "refreshing" : "loading",
      error: null,
      retryAvailableAt: null,
    }))

    const stillCurrent = () => !cancelled && generation.current === currentGeneration
    const load = async () => {
      let hasSnapshot = false
      try {
        const snapshot = await getTrendingBoards()
        if (!stillCurrent()) return
        hasSnapshot = snapshot.boards.length > 0
        setState({
          boards: snapshot,
          status: hasSnapshot ? "refreshing" : "loading",
          error: null,
          retryAvailableAt: null,
        })
      } catch (error) {
        if (!stillCurrent()) return
        setState({
          boards: null,
          status: "loading",
          error: toHavenErrorOrNull(error),
          retryAvailableAt: null,
        })
      }

      try {
        const refreshed = await refreshTrendingBoards()
        if (!stillCurrent()) return
        setState({ boards: refreshed, status: "ready", error: null, retryAvailableAt: null })
      } catch (error) {
        if (!stillCurrent()) return
        const refreshError = toHavenErrorOrNull(error)
        setState((current) => ({
          boards: hasSnapshot || current.boards?.boards.length ? current.boards : null,
          status: hasSnapshot || current.boards?.boards.length ? "ready" : "error",
          error: refreshError,
          retryAvailableAt: Date.now() + REFRESH_COOLDOWN_MS,
        }))
      }
    }
    void load()

    return () => {
      cancelled = true
    }
  }, [active, retryRevision])

  useEffect(() => {
    if (!state.retryAvailableAt) return
    const timeout = window.setTimeout(() => {
      setState((current) => ({ ...current, retryAvailableAt: null }))
    }, Math.max(0, state.retryAvailableAt - Date.now()))
    return () => window.clearTimeout(timeout)
  }, [state.retryAvailableAt])

  const retry = useCallback(() => {
    if (state.retryAvailableAt && state.retryAvailableAt > Date.now()) return
    setRetryRevision((current) => current + 1)
  }, [state.retryAvailableAt])

  return { ...state, retry }
}

function toHavenErrorOrNull(error: unknown): HavenError | null {
  return error instanceof HavenError ? error : null
}
