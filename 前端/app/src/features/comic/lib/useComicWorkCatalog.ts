// Comic Reader 的 Work 级章节目录读取。
//
// 事实源是后端 Work 聚合；Reader 只消费 Gateway 校验后的 DTO，不自行推导
// 章节顺序、当前章节或上一章/下一章。刷新是显式动作，不会在后台自动触发。

import { useCallback, useEffect, useRef, useState } from "react"
import { toHavenError } from "@/lib/ipc/errors"
import type { ComicWorkChapterCatalogDto } from "@/lib/ipc/generated/wire"
import {
  getComicWorkChapterCatalog,
  refreshComicWorkChapterCatalog,
} from "../ipc/comic-work-chapter-catalog-gateway"

export type ComicWorkCatalogState =
  | { status: "idle" | "loading" }
  | { status: "ready"; catalog: ComicWorkChapterCatalogDto }
  | { status: "error"; message: string; retryable: boolean }

export interface ComicWorkCatalogController {
  state: ComicWorkCatalogState
  refreshing: boolean
  refreshError: string | null
  refresh: () => void
  retry: () => void
}

export interface UseComicWorkCatalogOptions {
  /** Demo 分支不请求生产目录；Browser Demo 的章节事实仍由 Demo 自身提供。 */
  enabled: boolean
  mediaItemId?: string
}

export function useComicWorkCatalog({
  enabled,
  mediaItemId,
}: UseComicWorkCatalogOptions): ComicWorkCatalogController {
  const [state, setState] = useState<ComicWorkCatalogState>({ status: "idle" })
  const [retryNonce, setRetryNonce] = useState(0)
  const [refreshing, setRefreshing] = useState(false)
  const [refreshError, setRefreshError] = useState<string | null>(null)
  const generationRef = useRef(0)

  useEffect(() => {
    const generation = ++generationRef.current
    setRefreshing(false)
    setRefreshError(null)
    if (!enabled || !mediaItemId) {
      setState({ status: "idle" })
      return () => { generationRef.current += 1 }
    }
    let active = true
    setState({ status: "loading" })
    void getComicWorkChapterCatalog({ workId: null, mediaItemId })
      .then((catalog) => {
        if (!active || generationRef.current !== generation) return
        setState({ status: "ready", catalog })
      })
      .catch((error: unknown) => {
        if (!active || generationRef.current !== generation) return
        const normalized = toHavenError(error)
        setState({
          status: "error",
          message: normalized.dto.userMessage,
          retryable: normalized.retryable,
        })
      })
    return () => { active = false }
  }, [enabled, mediaItemId, retryNonce])

  const retry = useCallback(() => {
    setRetryNonce((value) => value + 1)
  }, [])

  const refresh = useCallback(() => {
    if (!enabled || !mediaItemId) return
    const generation = ++generationRef.current
    setRefreshing(true)
    setRefreshError(null)
    void refreshComicWorkChapterCatalog({ workId: null, mediaItemId })
      .then((catalog) => {
        if (generationRef.current !== generation) return
        setState({ status: "ready", catalog })
      })
      .catch((error: unknown) => {
        if (generationRef.current !== generation) return
        setRefreshError(toHavenError(error).dto.userMessage)
      })
      .finally(() => {
        if (generationRef.current === generation) setRefreshing(false)
      })
  }, [enabled, mediaItemId])

  return { state, refreshing, refreshError, refresh, retry }
}
