import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import type { CloudAccountDto, CloudFolderDto, CloudStorageListDto, StorageLocationDto } from "@/lib/ipc/generated/wire"
import { toHavenError, type HavenError } from "@/lib/ipc/errors"
import { cloudStorageGateway } from "../ipc/cloud-storage-gateway"

/** 云盘账户与位置关联只读投影，网络与持久化均在后端。 */
export function useCloudStorageRegistry(locations: StorageLocationDto[]) {
  const [snapshot, setSnapshot] = useState<CloudStorageListDto | null>(null)
  const [folders, setFolders] = useState<CloudFolderDto[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<HavenError | null>(null)
  const [busy, setBusy] = useState(false)
  const epoch = useRef(0)
  const alive = useRef(false)
  const lock = useRef(false)
  const ids = useMemo(() => locations.filter(location => location.providerType === "google_drive").map(location => location.locationId), [locations])
  const key = ids.join(",")
  const reload = useCallback(async () => {
    const current = ++epoch.current
    setLoading(true); setError(null)
    try {
      const next = await cloudStorageGateway.list()
      const bindings: CloudFolderDto[] = []
      // 绑定是本地投影；限制 IPC 批量并发，不探测远端目录。
      const locationIds = key ? key.split(",") : []
      for (let offset = 0; offset < locationIds.length; offset += 4) {
        const group = await Promise.all(locationIds.slice(offset, offset + 4).map(id => cloudStorageGateway.folderBinding(id)))
        bindings.push(...group.filter((item): item is CloudFolderDto => item !== null))
      }
      if (alive.current && current === epoch.current) { setSnapshot(next); setFolders(bindings) }
    } catch (failure) {
      if (alive.current && current === epoch.current) setError(toHavenError(failure))
    } finally {
      if (alive.current && current === epoch.current) setLoading(false)
    }
  }, [key])
  useEffect(() => {
    alive.current = true
    void reload()
    return () => { alive.current = false; epoch.current++ }
  }, [reload])

  const disconnect = async (account: CloudAccountDto): Promise<boolean> => {
    if (lock.current) return false
    lock.current = true; setBusy(true)
    try {
      await cloudStorageGateway.disconnect({ accountId: account.id, expectedGeneration: account.generation })
      if (alive.current) await reload()
      return true
    } catch (failure) {
      if (alive.current) setError(toHavenError(failure))
      return false
    } finally { lock.current = false; if (alive.current) setBusy(false) }
  }
  return { snapshot, folders, loading, error, busy, reload, disconnect }
}
