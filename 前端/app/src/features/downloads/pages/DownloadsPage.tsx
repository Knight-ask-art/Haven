import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useNavigate, useSearchParams } from "react-router"
import { ArrowDownUp, RefreshCw, Search } from "lucide-react"

import { HavenIcon } from "@/components/ui/haven/HavenIcon"
import { Input } from "@/components/ui/input"
import { DownloadBatchItem } from "../components/DownloadBatchItem"
import { DownloadTaskList } from "../components/DownloadTaskList"
import {
  cancelDownload,
  listDownloads,
  pauseDownload,
  resumeDownload,
  retryDownload,
  removeDownloadRecord,
  deleteOfflineDownload,
  revealOfflineDownload,
  subscribeDownloadEvents,
} from "../ipc/download-gateway"
import { resolveDownloadsRuntimeState } from "../lib/downloads-runtime-state"
import {
  acceptDownloadEvent,
  applyDownloadEvent,
  createDownloadEventState,
  forgetDownloadEventsForTask,
  mergeLatestDownloadEvents,
} from "../lib/download-event-state"
import { downloadErrorMessage, downloadErrorRetryable } from "../lib/download-error"
import { HavenError, toHavenError } from "@/lib/ipc/errors"
import type { ContentCategory, DownloadEvent, DownloadTaskDto } from "@/lib/ipc/generated/wire"
import { getHavenClientMode } from "@/lib/ipc/runtime"
import { cn } from "@/lib/utils"
import { useNotice } from "@/app/notice-center/notice-context"
import type { NoticeAction } from "@/app/notice-center/notice-context"

type CategoryFilter = "all" | ContentCategory
type StatusFilter = "all" | "active" | "completed" | "failed"
type SortMode = "recent" | "name" | "progress"

const CATEGORY_OPTIONS: Array<{ id: CategoryFilter; label: string }> = [
  { id: "all", label: "全部" },
  { id: "video", label: "影视" },
  { id: "book", label: "图书" },
  { id: "comic", label: "漫画" },
  { id: "periodical", label: "报刊资料" },
]

const STATUS_OPTIONS: Array<{ id: StatusFilter; label: string }> = [
  { id: "all", label: "全部任务" },
  { id: "active", label: "进行中" },
  { id: "completed", label: "已完成" },
  { id: "failed", label: "需处理" },
]

const ACTIVE_STATES = new Set<DownloadTaskDto["state"]>([
  "queued",
  "resolving",
  "downloading",
  "paused",
  "verifying",
])

export function DownloadsPage() {
  const navigate = useNavigate()
  const [searchParams, setSearchParams] = useSearchParams()
  const runtimeState = resolveDownloadsRuntimeState(getHavenClientMode())
  const category = parseCategory(searchParams.get("category"))
  const status = parseStatus(searchParams.get("status"))
  const [query, setQuery] = useState("")
  const [sortMode, setSortMode] = useState<SortMode>("recent")
  const [tasks, setTasks] = useState<DownloadTaskDto[]>([])
  const [loading, setLoading] = useState(true)
  const [errorMessage, setErrorMessage] = useState<string | null>(null)
  const [subscriptionErrorMessage, setSubscriptionErrorMessage] = useState<string | null>(null)
  const [subscriptionRevision, setSubscriptionRevision] = useState(0)
  const [pendingTaskIds, setPendingTaskIds] = useState<Set<string>>(() => new Set())
  const [isDragOver, setIsDragOver] = useState(false)
  const eventStateRef = useRef(createDownloadEventState())
  const { push, confirm } = useNotice()

  const publishDownloadError = useCallback((
    error: unknown,
    fallback: string,
    dedupeKey: string,
    action?: NoticeAction,
  ) => {
    const normalized = toHavenError(error)
    push({
      kind: "error",
      title: "下载任务需处理",
      message: normalized.dto.userMessage || fallback,
      code: normalized.code,
      retryable: normalized.retryable,
      dedupeKey,
      action,
    })
    return normalized
  }, [push])

  const refresh = useCallback(async (silent = false) => {
    if (runtimeState !== "ready") return
    if (!silent) setLoading(true)
    try {
      const next = await listDownloads()
      setTasks(mergeLatestDownloadEvents(next, eventStateRef.current))
      setErrorMessage(null)
    } catch (error) {
      if (!silent) {
        setErrorMessage(userMessage(error, "无法加载下载任务"))
        publishDownloadError(error, "无法加载下载任务", "downloads:list")
      }
    } finally {
      if (!silent) setLoading(false)
    }
  }, [publishDownloadError, runtimeState])

  const runAction = useCallback(async (
    taskId: string,
    action: (id: string) => Promise<DownloadTaskDto>,
  ) => {
    setPendingTaskIds((current) => new Set(current).add(taskId))
    try {
      const updated = await action(taskId)
      setTasks((current) => current.map((task) => task.taskId === taskId ? updated : task))
      setErrorMessage(null)
    } catch (error) {
      setErrorMessage(userMessage(error, "下载操作失败"))
      const normalized = toHavenError(error)
      publishDownloadError(
        error,
        "下载操作失败",
        `downloads:${taskId}:${normalized.code}`,
        normalized.retryable
          ? { label: "重试", onClick: () => runAction(taskId, retryDownload) }
          : undefined,
      )
    } finally {
      setPendingTaskIds((current) => {
        const next = new Set(current)
        next.delete(taskId)
        return next
      })
    }
  }, [publishDownloadError])

  useEffect(() => {
    if (runtimeState !== "ready") return
    let mounted = true
    let disposeSubscription: (() => Promise<void>) | null = null
    const onEvent = (event: DownloadEvent) => {
      if (!mounted || !acceptDownloadEvent(eventStateRef.current, event)) return
      if ((event.data.state === "failed" || event.data.state === "interrupted") && event.data.errorCode) {
        const retryable = downloadErrorRetryable(event.data.errorCode)
        push({
          kind: "error",
          title: "下载任务需处理",
          message: downloadErrorMessage(event.data.errorCode),
          code: event.data.errorCode,
          retryable,
          dedupeKey: `downloads:${event.data.taskId}:${event.data.errorCode}`,
          action: retryable
            ? {
                label: "重试",
                onClick: () => runAction(event.data.taskId, retryDownload),
              }
            : undefined,
        })
      }
      setTasks((current) => applyDownloadEvent(current, event))
    }
    void subscribeDownloadEvents(onEvent)
      .then((dispose) => {
        if (!mounted) {
          void dispose().catch(() => undefined)
          return
        }
        disposeSubscription = dispose
        setSubscriptionErrorMessage(null)
      })
      .catch((error) => {
        if (mounted) {
          setSubscriptionErrorMessage(userMessage(error, "无法订阅下载进度"))
          publishDownloadError(error, "无法订阅下载进度", "downloads:subscription")
        }
      })
      .finally(() => {
        if (mounted) void refresh()
      })
    return () => {
      mounted = false
      if (disposeSubscription) void disposeSubscription().catch(() => undefined)
    }
  }, [publishDownloadError, push, refresh, runAction, runtimeState, subscriptionRevision])

  const runManagement = useCallback(async (
    taskId: string,
    action: (id: string) => Promise<{ recordRemoved: boolean; offlineResourceRemoved: boolean }>,
  ) => {
    setPendingTaskIds((current) => new Set(current).add(taskId))
    try {
      const result = await action(taskId)
      forgetDownloadEventsForTask(eventStateRef.current, taskId)
      if (result.recordRemoved) {
        setTasks((current) => current.filter((task) => task.taskId !== taskId))
      } else if (result.offlineResourceRemoved) {
        setTasks((current) => current.map((task) => task.taskId === taskId
          ? { ...task, offlineResourceId: null }
          : task))
      }
      setErrorMessage(null)
    } catch (error) {
      setErrorMessage(userMessage(error, "下载内容操作失败"))
      publishDownloadError(error, "下载内容操作失败", `downloads:${taskId}:management`)
    } finally {
      setPendingTaskIds((current) => {
        const next = new Set(current)
        next.delete(taskId)
        return next
      })
    }
  }, [publishDownloadError])

  const handleDeleteOffline = useCallback((taskId: string) => {
    void confirm({
      title: "删除离线内容",
      message: "删除离线内容后，文件将从离线目录中移除，但下载记录会保留。确定继续吗？",
      confirmLabel: "删除",
      cancelLabel: "取消",
      dedupeKey: `downloads:${taskId}:delete-confirm`,
    }).then((confirmed) => {
      if (confirmed) void runManagement(taskId, deleteOfflineDownload)
    })
  }, [confirm, runManagement])

  const handleRevealOffline = useCallback((taskId: string) => {
    void revealOfflineDownload(taskId).catch((error) => {
      setErrorMessage(userMessage(error, "无法打开文件所在位置"))
      publishDownloadError(error, "无法打开文件所在位置", `downloads:${taskId}:reveal`)
    })
  }, [publishDownloadError])

  const filteredTasks = useMemo(() => {
    const normalizedQuery = query.trim().toLocaleLowerCase()
    const next = tasks.filter((task) => {
      if (category !== "all" && task.category !== category) return false
      if (!matchesStatus(task, status)) return false
      if (!normalizedQuery) return true
      return task.title.toLocaleLowerCase().includes(normalizedQuery)
        || task.mediaType.toLocaleLowerCase().includes(normalizedQuery)
    })
    if (sortMode === "name") {
      return [...next].sort((left, right) => left.title.localeCompare(right.title, "zh-CN"))
    }
    if (sortMode === "progress") {
      return [...next].sort((left, right) => (right.progressRatio ?? 0) - (left.progressRatio ?? 0))
    }
    return [...next].sort((left, right) => right.createdAt.localeCompare(left.createdAt))
  }, [category, query, sortMode, status, tasks])

  const { batchGroups, singleTasks } = useMemo(() => {
    const groups = new Map<string, DownloadTaskDto[]>()
    const singles: DownloadTaskDto[] = []
    for (const task of filteredTasks) {
      if (task.batchId) {
        const list = groups.get(task.batchId) ?? []
        list.push(task)
        groups.set(task.batchId, list)
      } else {
        singles.push(task)
      }
    }
    return {
      batchGroups: Array.from(groups.entries()).map(([batchId, tasks]) => ({
        batchId,
        title: tasks[0]?.title ? `${tasks[0].title} 等 ${tasks.length} 项` : `批次 ${batchId.slice(0, 8)}`,
        tasks,
      })),
      singleTasks: singles,
    }
  }, [filteredTasks])

  if (runtimeState === "unavailable") return <DownloadsUnavailablePage />

  const activeCount = tasks.filter((task) => ACTIVE_STATES.has(task.state)).length
  const completedCount = tasks.filter((task) => task.state === "completed").length
  const issueCount = tasks.filter((task) => task.state === "failed" || task.state === "interrupted").length
  const visibleErrorMessage = errorMessage ?? subscriptionErrorMessage
  const retryVisibleError = () => {
    if (subscriptionErrorMessage) setSubscriptionRevision((current) => current + 1)
    else void refresh()
  }

  return (
    <div
      className="flex h-full w-full flex-col overflow-hidden bg-background"
      onDragOver={(e) => {
        e.preventDefault()
        setIsDragOver(true)
      }}
      onDragLeave={() => setIsDragOver(false)}
      onDrop={(e) => {
        e.preventDefault()
        setIsDragOver(false)
        const files = Array.from(e.dataTransfer.files)
        if (files.length > 0) {
          setErrorMessage(`检测到 ${files.length} 个文件，拖拽导入将在下一版本支持（已记录 ${files[0].name}）`)
        }
      }}
    >
      <header className="shrink-0 border-b border-border/60 px-[24px] pb-[18px] pt-[28px] md:px-[48px] lg:px-[72px]">
        <div className="flex flex-col gap-[18px] xl:flex-row xl:items-end xl:justify-between">
          <div>
            <div className="flex items-center gap-[10px]">
              <h1 className="text-2xl font-bold">下载</h1>
              <button
                type="button"
                onClick={() => void refresh()}
                disabled={loading}
                className="flex h-[34px] w-[34px] items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:opacity-40"
                title="刷新下载任务"
              >
                <RefreshCw className={cn("h-[16px] w-[16px]", loading && "animate-spin")} />
              </button>
            </div>
            <p className="mt-[5px] text-sm text-muted-foreground">
              {activeCount} 个进行中 · {completedCount} 个已完成{issueCount > 0 ? ` · ${issueCount} 个需处理` : ""}
            </p>
          </div>
          <div className="flex flex-col gap-[10px] sm:flex-row sm:items-center">
            <div className="relative w-full sm:w-[280px]">
              <Search className="absolute left-[12px] top-1/2 h-[15px] w-[15px] -translate-y-1/2 text-muted-foreground" />
              <Input
                type="search"
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder="搜索下载任务"
                className="h-[38px] rounded-full border-border/60 bg-muted/40 pl-[36px]"
              />
            </div>
            <button
              type="button"
              onClick={() => setSortMode((current) => current === "recent" ? "name" : current === "name" ? "progress" : "recent")}
              className="inline-flex h-[38px] items-center justify-center gap-[8px] rounded-full border border-border/60 px-[14px] text-xs font-semibold text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
            >
              <ArrowDownUp className="h-[15px] w-[15px]" />
              {sortMode === "recent" ? "最近添加" : sortMode === "name" ? "名称" : "进度"}
            </button>
          </div>
        </div>

        <div className="mt-[22px] flex flex-col gap-[14px]">
          <nav className="flex flex-wrap gap-x-[28px] gap-y-[8px]" aria-label="下载内容分类">
            {CATEGORY_OPTIONS.map((option) => (
              <button
                key={option.id}
                type="button"
                onClick={() => updateParam(searchParams, setSearchParams, "category", option.id)}
                className={cn(
                  "border-b-2 pb-[8px] text-sm font-semibold transition-colors",
                  category === option.id ? "border-foreground text-foreground" : "border-transparent text-muted-foreground hover:text-foreground",
                )}
              >
                {option.label}
              </button>
            ))}
          </nav>
          <div className="flex w-fit max-w-full gap-[4px] overflow-x-auto rounded-lg bg-muted/60 p-[4px]" role="tablist" aria-label="下载状态">
            {STATUS_OPTIONS.map((option) => (
              <button
                key={option.id}
                type="button"
                role="tab"
                aria-selected={status === option.id}
                onClick={() => updateParam(searchParams, setSearchParams, "status", option.id)}
                className={cn(
                  "h-[30px] whitespace-nowrap rounded-md px-[12px] text-xs font-semibold transition-colors",
                  status === option.id ? "bg-background text-foreground shadow-sm" : "text-muted-foreground hover:text-foreground",
                )}
              >
                {option.label}
              </button>
            ))}
          </div>
        </div>
      </header>

      {/* 存储配额 Banner（P1） */}
      <div className="mx-[24px] mt-[16px] flex items-center justify-between rounded-lg border border-border/60 bg-muted/30 px-[16px] py-[10px] text-xs md:mx-[48px] lg:mx-[72px]">
        <span className="text-muted-foreground">存储配额：本地离线内容由下载任务管理，超出可用空间时进入 <span className="font-semibold text-foreground">WaitingForSpace</span></span>
        <span className="hidden text-muted-foreground sm:block">拖拽文件到此处可导入（P1 预览）</span>
      </div>

      {isDragOver && (
        <div className="pointer-events-none absolute inset-0 z-10 flex items-center justify-center bg-background/80 backdrop-blur-sm">
          <div className="rounded-2xl border-2 border-dashed border-primary bg-background px-8 py-6 text-center shadow-xl">
            <p className="text-sm font-semibold">释放以导入文件</p>
            <p className="mt-1 text-xs text-muted-foreground">支持 EPUB / PDF / CBZ（下一版本落地指纹去重）</p>
          </div>
        </div>
      )}

      {visibleErrorMessage && (
        <div className="mx-[24px] mt-[16px] flex items-center justify-between gap-[16px] rounded-lg border border-destructive/30 bg-destructive/5 px-[16px] py-[12px] text-sm text-destructive md:mx-[48px] lg:mx-[72px]">
          <span>{visibleErrorMessage}</span>
          <button type="button" onClick={retryVisibleError} className="shrink-0 font-semibold hover:underline">重试</button>
        </div>
      )}

      <main className="min-h-0 flex-1 overflow-y-auto px-[24px] py-[20px] pb-[96px] md:px-[48px] lg:px-[72px]">
        {loading && tasks.length === 0 ? (
          <DownloadListSkeleton />
        ) : (
          <>
            <div className="mb-[12px] flex items-center justify-between text-xs text-muted-foreground">
              <span>
                {filteredTasks.length} 个任务
                {batchGroups.length > 0 && ` · ${batchGroups.length} 个批次`}
              </span>
              {pendingTaskIds.size > 0 && <span>正在处理 {pendingTaskIds.size} 个操作</span>}
            </div>
            {batchGroups.length > 0 && (
              <div className="mb-4 space-y-3">
                {batchGroups.map((group) => (
                  <DownloadBatchItem
                    key={group.batchId}
                    batchId={group.batchId}
                    title={group.title}
                    tasks={group.tasks}
                  />
                ))}
              </div>
            )}
            <DownloadTaskList
              tasks={singleTasks}
              pendingTaskIds={pendingTaskIds}
              onPause={(id) => void runAction(id, pauseDownload)}
              onResume={(id) => void runAction(id, resumeDownload)}
              onCancel={(id) => void runAction(id, cancelDownload)}
              onRetry={(id) => void runAction(id, retryDownload)}
              onOpen={(task) => navigate(downloadTargetRoute(task))}
              onRemoveRecord={(id) => void runManagement(id, removeDownloadRecord)}
              onDeleteOffline={handleDeleteOffline}
              onRevealOffline={handleRevealOffline}
            />
          </>
        )}
      </main>
    </div>
  )
}

function DownloadsUnavailablePage() {
  return (
    <div className="flex h-full w-full flex-col items-center justify-center bg-background px-[24px] text-center">
      <div className="mb-[16px] flex h-[56px] w-[56px] items-center justify-center rounded-full bg-muted">
        <HavenIcon symbol="download" size={32} className="text-muted-foreground" />
      </div>
      <h1 className="text-lg font-semibold">下载服务不可用</h1>
      <p className="mt-[7px] max-w-sm text-sm text-muted-foreground">请通过栖阅桌面程序打开下载中心。</p>
    </div>
  )
}

function DownloadListSkeleton() {
  return (
    <div className="flex flex-col gap-[10px]" aria-label="正在加载下载任务">
      {Array.from({ length: 4 }).map((_, index) => (
        <div key={index} className="flex min-h-[128px] animate-pulse gap-[20px] rounded-lg border border-border/60 p-[16px]">
          <div className="h-[96px] w-[68px] rounded-md bg-muted" />
          <div className="flex flex-1 flex-col gap-[12px] py-[8px]">
            <div className="h-[16px] w-2/5 rounded bg-muted" />
            <div className="h-[12px] w-1/5 rounded bg-muted/80" />
            <div className="mt-auto h-[5px] w-full rounded bg-muted" />
          </div>
        </div>
      ))}
    </div>
  )
}

function matchesStatus(task: DownloadTaskDto, filter: StatusFilter): boolean {
  if (filter === "all") return true
  if (filter === "active") return ACTIVE_STATES.has(task.state)
  if (filter === "completed") return task.state === "completed"
  return task.state === "failed" || task.state === "interrupted"
}

function parseCategory(value: string | null): CategoryFilter {
  return CATEGORY_OPTIONS.some((option) => option.id === value) ? value as CategoryFilter : "all"
}

function parseStatus(value: string | null): StatusFilter {
  return STATUS_OPTIONS.some((option) => option.id === value) ? value as StatusFilter : "all"
}

function updateParam(
  current: URLSearchParams,
  setSearchParams: ReturnType<typeof useSearchParams>[1],
  key: string,
  value: string,
) {
  const next = new URLSearchParams(current)
  if (value === "all") next.delete(key)
  else next.set(key, value)
  setSearchParams(next, { replace: true })
}

function userMessage(error: unknown, fallback: string): string {
  return error instanceof HavenError ? error.dto.userMessage : fallback
}

function downloadTargetRoute(task: DownloadTaskDto): string {
  if (!task.mediaItemId) return task.workId ? `/work/${task.workId}` : "/downloads"
  if (task.mediaType === "movie" || task.mediaType === "series" || task.mediaType === "episode") return `/player/${task.mediaItemId}`
  if (task.mediaType === "comic") return `/comic/${task.mediaItemId}`
  if (task.mediaType === "article") return `/article/${task.mediaItemId}`
  return `/reader/${task.mediaItemId}`
}
