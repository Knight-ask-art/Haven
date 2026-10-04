import { useCallback, useEffect, useRef, useState } from "react"
import { toHavenError, type HavenError } from "@/lib/ipc/errors"
import {
  SCAN_PHASE_LABELS, cancelScan, listStorageLocations, pickLocalDirectory,
  rebindLocalDirectory, removeStorageLocation, startLibraryScan, type StorageLocationWire,
} from "../ipc/storage-gateway"

export interface StorageScanState {
  taskId: string; phaseCode: string; phase: string; filesSeen: number; newItem: number
  message: string | null; terminal: boolean
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
      setScans(previous => ({ ...previous, [location.locationId]: {
        taskId: event.data.taskId, phaseCode: event.kind, phase, filesSeen: event.data.filesSeen,
        newItem: event.data.new, message: event.data.message ?? null, terminal,
      } }))
      if (terminal) showNotice(`${location.displayName}：${phase}`)
    })
    if (!alive.current) return
    setScans(previous => ({ ...previous, [location.locationId]: previous[location.locationId]?.taskId === result.taskId ? previous[location.locationId] : {
      taskId: result.taskId, phaseCode: "started", phase: SCAN_PHASE_LABELS.started,
      filesSeen: 0, newItem: 0, message: null, terminal: false,
    } }))
    if (result.alreadyRunning) showNotice("该目录已有扫描任务，已接入现有任务")
  })

  const cancelDirectoryScan = (location: StorageLocationWire, scan: StorageScanState) => action(async () => {
    const result = await cancelScan(scan.taskId)
    if (!alive.current) return
    setScans(previous => ({ ...previous, [location.locationId]: {
      ...scan, phaseCode: result.phase, phase: SCAN_PHASE_LABELS[result.phase] ?? result.phase, terminal: true,
    } }))
    showNotice(result.alreadyTerminal ? "扫描已结束" : "已取消扫描")
  })

  return { locations, scans, isLoading, error, busy, reload, addDirectory, rebindDirectory, removeDirectory, scanDirectory, cancelDirectoryScan }
}
