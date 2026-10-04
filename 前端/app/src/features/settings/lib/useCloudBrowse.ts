import { useEffect, useRef, useState } from "react"
import type { CloudBrowseEntryDto, CloudBrowsePageDto } from "@/lib/ipc/generated/wire"
import { toHavenError, type HavenError } from "@/lib/ipc/errors"
import { cloudStorageGateway } from "../ipc/cloud-storage-gateway"

interface CloudBreadcrumb { handle: string | null; name: string }

const ROOT_NAME = "我的云盘"
const BOUND_NAME = "已登记目录"

/** 一页浏览只持不透明句柄；下一页替换当前页，避免无限累积远端条目。 */
export function useCloudBrowse(accountId: string, locationId: string | undefined, onRegistered: (id: string) => void, onImported: () => void) {
  const [page, setPage] = useState<CloudBrowsePageDto | null>(null)
  const [trail, setTrail] = useState<CloudBreadcrumb[]>([])
  const [error, setError] = useState<HavenError | null>(null)
  const [busy, setBusy] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const [displayName, setDisplayName] = useState("")
  const epoch = useRef(0)
  const alive = useRef(false)
  const lock = useRef(false)

  // seed 只表示目录导航；翻页/刷新不 seed，重新签发句柄也不代表逻辑目录变化。
  const read = async (query: () => Promise<CloudBrowsePageDto>, path: CloudBreadcrumb[], seed?: string) => {
    if (lock.current) return
    lock.current = true
    const current = ++epoch.current
    setBusy(true); setError(null); setNotice(null)
    try {
      const next = await query()
      if (!alive.current || epoch.current !== current) return
      const nextTrail = path.length
        ? path.map((crumb, index) => index === path.length - 1 ? { ...crumb, handle: next.folderHandle } : crumb)
        : [{ handle: next.folderHandle, name: locationId ? BOUND_NAME : ROOT_NAME }]
      setPage(next); setTrail(nextTrail)
      if (seed !== undefined) setDisplayName(seed)
    } catch (failure) {
      if (alive.current && epoch.current === current) setError(toHavenError(failure))
    } finally {
      if (epoch.current === current) {
        lock.current = false
        if (alive.current) setBusy(false)
      }
    }
  }

  const openRoot = (seed?: string) => read(() => locationId ? cloudStorageGateway.location(locationId) : cloudStorageGateway.root(accountId), [], seed)
  // 错误恢复从子目录返回 root 时是导航；当前就在 root 时保留用户草稿。
  const root = () => openRoot(!locationId && trail.length > 1 ? ROOT_NAME : undefined)
  useEffect(() => {
    alive.current = true
    void openRoot(locationId ? undefined : ROOT_NAME)
    return () => { alive.current = false; epoch.current++; lock.current = false }
    // 面板按账户 / 位置 key 挂载，交互请求由上面的 epoch 收口。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [accountId, locationId])

  const enter = (entry: CloudBrowseEntryDto) => {
    if (locationId || !entry.isFolder || entry.handle === null) return
    return read(() => cloudStorageGateway.folder(entry.handle!), [...trail, { handle: entry.handle, name: entry.displayName }], entry.displayName)
  }
  const ascend = (index: number) => {
    const crumb = trail[index]
    if (!crumb || locationId) return
    if (index === 0) return openRoot(ROOT_NAME)
    return read(() => cloudStorageGateway.folder(crumb.handle!), trail.slice(0, index + 1), crumb.name)
  }
  const reload = () => {
    if (locationId || trail.length <= 1) return root()
    const current = trail.at(-1)!
    return read(() => cloudStorageGateway.folder(current.handle!), trail)
  }
  const next = () => {
    const cursor = page?.nextCursor
    if (!cursor || lock.current) return
    // 服务端光标一次性消费；失败也不能重放，只能刷新目录重新取得页面。
    setPage(previous => previous ? { ...previous, nextCursor: null } : null)
    return read(() => cloudStorageGateway.next(cursor), trail)
  }

  const register = async () => {
    if (lock.current || locationId || !page?.folderHandle || !displayName.trim()) return
    lock.current = true; setBusy(true); setError(null)
    try {
      const id = await cloudStorageGateway.register({ folderHandle: page.folderHandle, displayName: displayName.trim() })
      if (alive.current) onRegistered(id)
    } catch (failure) { if (alive.current) setError(toHavenError(failure)) }
    finally { lock.current = false; if (alive.current) setBusy(false) }
  }
  const importPdf = async (entry: CloudBrowseEntryDto) => {
    if (lock.current || !locationId || !entry.pdfSupported || !entry.handle) return
    lock.current = true; setBusy(true); setError(null); setNotice(null)
    try {
      const imported = await cloudStorageGateway.importPdf({ locationId, fileHandle: entry.handle })
      if (alive.current) { setNotice(`已加入媒体库：${imported.displayName}`); onImported() }
    } catch (failure) { if (alive.current) setError(toHavenError(failure)) }
    finally { lock.current = false; if (alive.current) setBusy(false) }
  }
  return { page, trail, error, busy, notice, displayName, setDisplayName, root, enter, ascend, reload, next, register, importPdf }
}
