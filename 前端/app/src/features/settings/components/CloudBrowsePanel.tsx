import { Link } from "react-router"
import type { CloudAccountDto } from "@/lib/ipc/generated/wire"
import { useCloudBrowse } from "../lib/useCloudBrowse"
import { ManagementHeading, ManagementSection } from "./StorageSettingsLayout"

function fileSize(value: number | null) {
  if (value === null) return "大小未知"
  return value < 1024 * 1024 ? `${Math.ceil(value / 1024)} KB` : `${(value / (1024 * 1024)).toFixed(1)} MB`
}

/** 用同一大表单呈现选目录与已登记目录的 PDF 导入，不添加额外的小面板。 */
export function CloudBrowsePanel({ account, locationId, onBack, onRegistered, onImported }: {
  account: CloudAccountDto; locationId?: string; onBack: () => void; onRegistered: (id: string) => void; onImported: () => void
}) {
  const browse = useCloudBrowse(account.id, locationId, onRegistered, onImported)
  return <div className="storage-downloads-settings">
    <ManagementHeading title={locationId ? "浏览 Google Drive" : "选择媒体文件夹"} description={locationId ? "将所选目录中的 PDF 加入媒体库，在线读取原文件。" : "只有确认目录后才添加存储位置，授权本身不会导入内容。"} />
    <div className="storage-connect-steps"><button type="button" className="storage-action" onClick={onBack}>返回存储</button><span>01 已授权 · {account.displayName}</span><span aria-hidden="true">→</span><span aria-current="step">02 {locationId ? "浏览媒体目录" : "选择媒体文件夹"}</span></div>
    <ManagementSection title={locationId ? "目录内容" : "媒体文件夹"} description={locationId ? "首期支持 PDF（最大 128 MB）。子目录需单独添加；不递归扫描、不下载离线副本。" : "打开目录后，确认将当前目录登记到媒体库。"}
      action={<button type="button" className="storage-action" disabled={browse.busy} onClick={() => void browse.reload()}>刷新目录</button>}>
      <div className="storage-browse-toolbar"><nav aria-label="云盘目录层级" className="storage-actions">{browse.trail.map((crumb, index) => <button type="button" className="storage-action" key={`${index}-${crumb.handle ?? "root"}`} disabled={browse.busy || Boolean(locationId) || index === browse.trail.length - 1} onClick={() => void browse.ascend(index)}>{crumb.name}</button>)}</nav>
        {!locationId && <input aria-label="媒体目录显示名称" value={browse.displayName} maxLength={120} disabled={browse.busy} onChange={event => browse.setDisplayName(event.target.value)} />}
      </div>
      {browse.error && <div role="alert" className="storage-error">{browse.error.message}<button type="button" className="storage-action" disabled={browse.busy} onClick={() => void browse.root()}>重新打开目录</button></div>}
      {browse.busy && <p role="status" className="storage-message">正在处理…</p>}
      {browse.notice && <div role="status" className="storage-message">{browse.notice} <Link to="/library" className="storage-action">前往媒体库</Link></div>}
      {browse.page && <table className="storage-table"><thead><tr><th scope="col">名称</th><th scope="col">类型</th><th scope="col">大小</th><th scope="col">操作</th></tr></thead><tbody>
        {browse.page.entries.map((entry, index) => <tr key={entry.handle ?? `${entry.displayName}-${index}`}><td>{entry.displayName}</td><td>{entry.isFolder ? "文件夹" : entry.pdfSupported ? "PDF" : "其他文件"}</td><td>{entry.isFolder ? "—" : fileSize(entry.sizeBytes)}</td><td>
          {entry.isFolder && !locationId ? <button type="button" className="storage-action" disabled={browse.busy || !entry.handle} onClick={() => void browse.enter(entry)}>打开</button>
            : entry.isFolder ? <span className="storage-detail">需单独添加</span>
              : entry.pdfSupported && locationId ? <button type="button" className="storage-action" disabled={browse.busy || !entry.handle} onClick={() => void browse.importPdf(entry)}>加入媒体库</button>
                : <span className="storage-detail">{!locationId ? "请先确认媒体目录" : "暂不支持"}</span>}
        </td></tr>)}
      </tbody></table>}
      {!browse.busy && browse.page?.entries.length === 0 && <p className="storage-message">这个目录暂时没有可列出的内容。</p>}
      <div className="storage-browse-footer"><span className="storage-detail">只读访问，不会改动云盘原始文件。</span><div className="storage-actions">
        {browse.page?.nextCursor && <button type="button" className="storage-action" disabled={browse.busy} onClick={() => void browse.next()}>下一页</button>}
        {!locationId && <button type="button" className="storage-action storage-action-primary" disabled={browse.busy || !browse.page?.folderHandle || !browse.displayName.trim()} onClick={() => void browse.register()}>确认当前文件夹</button>}
      </div></div>
    </ManagementSection>
  </div>
}
