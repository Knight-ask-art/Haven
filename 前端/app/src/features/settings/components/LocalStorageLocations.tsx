import { useState } from "react"
import type { useStorageLocations } from "../lib/useStorageLocations"
import { ManagementSection } from "./StorageSettingsLayout"

const STATUS_LABELS: Record<string, string> = {
  connected: "可用", read_only: "只读", disconnected: "已断开", missing: "需要重新绑定",
  unavailable: "不可用", auth_expired: "需要重新授权", error: "错误", disabled: "已停用",
}

/** 本地目录的大列表；只保留已有扫描、重绑、移除能力。 */
export function LocalStorageLocations({ storage }: { storage: ReturnType<typeof useStorageLocations> }) {
  const [confirmId, setConfirmId] = useState<string | null>(null)
  const locations = storage.locations.filter(location => location.providerType === "local")
  return <ManagementSection feature="storage.locations" title="本地目录" description="扫描本地文件，将影视、图书和漫画收进媒体库。"
    action={<button type="button" className="storage-action storage-action-primary" disabled={storage.busy} onClick={() => void storage.addDirectory()}>添加本地目录</button>}>
    {storage.isLoading && <p role="status" className="storage-message">正在读取本地目录…</p>}
    {storage.error && <div role="alert" className="storage-error">{storage.error.message}<button type="button" className="storage-action" onClick={() => void storage.reload()}>重新读取</button></div>}
    {!storage.isLoading && !storage.error && locations.length === 0 && <p className="storage-message">还没有本地目录。添加文件夹后，即可扫描其中的媒体。</p>}
    {locations.length > 0 && <table className="storage-table"><thead><tr><th scope="col">名称</th><th scope="col">类型</th><th scope="col">状态</th><th scope="col">操作</th></tr></thead><tbody>
      {locations.map(location => {
        const scan = storage.scans[location.locationId]
        const running = scan && !scan.terminal
        return <tr key={location.locationId}><td><strong>{location.displayName}</strong>{scan && <span className="storage-detail" role="status">{scan.phase} · 已见 {scan.filesSeen} · 新增 {scan.newItem}{scan.message ? ` · ${scan.message}` : ""}</span>}</td>
          <td>本地目录</td><td>{STATUS_LABELS[location.status] ?? location.status}</td><td><div className="storage-actions">
            {running ? <button type="button" className="storage-action" disabled={storage.busy} onClick={() => void storage.cancelDirectoryScan(location, scan)}>取消扫描</button>
              : <button type="button" className="storage-action" disabled={storage.busy || location.status !== "connected"} onClick={() => void storage.scanDirectory(location)}>扫描</button>}
            <details className="storage-row-menu"><summary aria-label={`${location.displayName}的更多操作`}>···</summary><div>
              <button type="button" className="storage-action" disabled={storage.busy || Boolean(running)} onClick={() => void storage.rebindDirectory(location)}>重新绑定</button>
              <button type="button" className="storage-action storage-action-danger" disabled={storage.busy || Boolean(running)} onClick={() => setConfirmId(location.locationId)}>移除绑定</button>
            </div></details>
          </div>{confirmId === location.locationId && <div className="storage-actions" role="group" aria-label="确认移除本地目录">
            <span className="storage-detail">移除应用内索引，保留原始文件。</span>
            <button type="button" className="storage-action" disabled={storage.busy} onClick={() => setConfirmId(null)}>取消</button>
            <button type="button" className="storage-action storage-action-danger" disabled={storage.busy || Boolean(running)} onClick={() => { setConfirmId(null); void storage.removeDirectory(location) }}>确认移除</button>
          </div>}</td></tr>
      })}
    </tbody></table>}
  </ManagementSection>
}
