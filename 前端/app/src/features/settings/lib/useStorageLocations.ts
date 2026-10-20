import { useCallback, useEffect, useRef, useState } from "react"
import { toHavenError, type HavenError } from "@/lib/ipc/errors"
import {
  SCAN_PHASE_LABELS, cancelScan, listStorageLocations, pickLocalDirectory,
  rebindLocalDirectory, removeStorageLocation, startLibraryScan, type StorageLocationWire,
} from "../ipc/storage-gateway"

export interface StorageScanState {
  taskId: string; phaseCode: string; phase: string; filesSeen: number; newItem: number
  /** 后端权威失败条目数（扫描已见条目中未能索引的数量）。 */
  errors: number
  message: string | null
  /** 同一任务内最近一条安全警告（只保留一条；终态后保留供排查）。 */
  warning: string | null
  terminal: boolean
  cancelling?: boolean
}

/** 警告展示上限：后端文案本身有界，这里再兜一层，避免异常长度挤占进度行。 */
const MAX_WARNING_LENGTH = 120

function boundedWarning(message: string | null): string | null {
  if (!message) return null
  return message.length > MAX_WARNING_LENGTH ? `${message.slice(0, MAX_WARNING_LENGTH - 1)}…` : message
}

/** 权威位置读回、原生目录动作与扫描状态；过期请求不覆盖更新的目录列表。 */
export function useStorageLocations(showNotice: (message: string) => void) {
  const [locations, setLocations] = useState<StorageLocationWire[]>([])
  const [scans, setScans] = useState<Record<string, StorageScanState>>({})
  const [isLoading, setIsLoading] = useState(true)
  const [error, setError] = useState<HavenError | null>(null)
  const [busy, setBusy] = useState(false)
  const epoch = useRef(0)
  const alive = useRef(false)
  const actionLock = useRef(false)

  const reload = useCallback(async () => {
    const current = ++epoch.current
    setIsLoading(true)
    setError(null)
    try {
      const result = await listStorageLocations()
      if (alive.current && current === epoch.current) setLocations(result)
    } catch (failure) {
      if (alive.current && current === epoch.current) setError(toHavenError(failure))
    } finally {
      if (alive.current && current === epoch.current) setIsLoading(false)
    }
  }, [])

  useEffect(() => {
    alive.current = true
    void reload()
    return () => { alive.current = false; epoch.current++ }
  }, [reload])

  const action = async (work: () => Promise<void>) => {
    if (actionLock.current) return
    actionLock.current = true
    setBusy(true)
    try { await work() } catch (failure) {
      const normalized = toHavenError(failure)
      if (alive.current && normalized.code !== "OPERATION_CANCELLED") showNotice(normalized.message)
    } finally {
      actionLock.current = false
      if (alive.current) setBusy(false)
    }
  }

  const addDirectory = () => action(async () => {
    await pickLocalDirectory()
    if (!alive.current) return
    await reload()
    showNotice("已添加本地目录")
  })

  const rebindDirectory = (location: StorageLocationWire) => action(async () => {
    await rebindLocalDirectory(location.locationId)
    if (!alive.current) return
    await reload()
    showNotice(`已重新绑定：${location.displayName}`)
  })

  const removeDirectory = (location: StorageLocationWire) => action(async () => {
    await removeStorageLocation(location.locationId)
    if (!alive.current) return
    setScans(previous => {
      const next = { ...previous }; delete next[location.locationId]; return next
    })
    await reload()
    showNotice(`已移除：${location.displayName}（应用内索引退出媒体库，原始文件保留）`)
  })

  const scanDirectory = (location: StorageLocationWire) => action(async () => {
    const result = await startLibraryScan(location.locationId, event => {
      if (!alive.current) return
      const terminal = ["completed", "cancelled", "failed"].includes(event.kind)
      const phase = SCAN_PHASE_LABELS[event.kind] ?? event.kind
      const errors = event.data.errors ?? 0
      setScans(previous => {
        const prior = previous[location.locationId]
        // 只沿用**同一任务**的取消中状态与最近警告；换任务（含新任务复用位置）时重置。
        const sameTask = prior?.taskId === event.data.taskId
        const cancelling = !terminal && sameTask && prior?.cancelling === true
        // 终态事件的 message 是后端有界的计数摘要；其它阶段的 message 只在警告事件里出现。
        const warning = sameTask
          ? boundedWarning(event.kind === "warning" ? event.data.message ?? prior?.warning ?? null : prior?.warning ?? null)
          : boundedWarning(event.kind === "warning" ? event.data.message : null)
        return { ...previous, [location.locationId]: {
          taskId: event.data.taskId, phaseCode: event.kind, phase: cancelling ? "正在取消" : phase,
          filesSeen: event.data.filesSeen, newItem: event.data.new,
          errors, message: terminal ? event.data.message ?? null : null, warning, terminal, cancelling,
        } }
      })
      if (terminal) showNotice(`${location.displayName}：${phase}${errors > 0 ? `（${errors} 个条目未能索引）` : ""}`)
    })
    if (!alive.current) return
    setScans(previous => ({ ...previous, [location.locationId]: previous[location.locationId]?.taskId === result.taskId ? previous[location.locationId] : {
      taskId: result.taskId, phaseCode: "started", phase: SCAN_PHASE_LABELS.started,
      filesSeen: 0, newItem: 0, errors: 0, message: null, warning: null, terminal: false,
    } }))
    if (result.alreadyRunning) showNotice("该目录已有扫描任务，已接入现有任务")
  })

  const cancelDirectoryScan = (location: StorageLocationWire, scan: StorageScanState) => action(async () => {
    const result = await cancelScan(scan.taskId)
    if (!alive.current) return
    setScans(previous => {
      const current = previous[location.locationId]
      // 终态事件可能先于取消响应到达；不能用响应覆盖终态或另一任务。
      if (!current || current.taskId !== scan.taskId || current.terminal) return previous
      return { ...previous, [location.locationId]: {
        ...current,
        phaseCode: result.alreadyTerminal ? result.phase : current.phaseCode,
        phase: result.alreadyTerminal ? (SCAN_PHASE_LABELS[result.phase] ?? result.phase) : "正在取消",
        terminal: result.alreadyTerminal,
        cancelling: !result.alreadyTerminal,
      } }
    })
    showNotice(result.alreadyTerminal ? "扫描已结束" : "取消请求已受理")
  })

  return { locations, scans, isLoading, error, busy, reload, addDirectory, rebindDirectory, removeDirectory, scanDirectory, cancelDirectoryScan }
}
