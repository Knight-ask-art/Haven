import { useEffect, useRef, useState } from "react"
import type { CloudAccountDto } from "@/lib/ipc/generated/wire"
import { useStorageLocations } from "../lib/useStorageLocations"
import { useCloudStorageRegistry } from "../lib/useCloudStorageRegistry"
import { cloudStorageGateway } from "../ipc/cloud-storage-gateway"
import { toHavenError } from "@/lib/ipc/errors"
import { ManagementHeading, ManagementSection } from "./StorageSettingsLayout"
import { LocalStorageLocations } from "./LocalStorageLocations"
import { CloudConnectPanel } from "./CloudConnectPanel"
import { CloudBrowsePanel } from "./CloudBrowsePanel"

type StorageView = { kind: "overview" } | { kind: "connect"; accountId?: string }
  | { kind: "browse"; account: CloudAccountDto; locationId?: string }

/** 本地 / 云盘两个大列表；列表、授权与目录状态都由真实后端读回。 */
export function StorageSettings({ showNotice }: { showNotice: (message: string) => void }) {
  const local = useStorageLocations(showNotice)
  const cloud = useCloudStorageRegistry(local.locations)
  const [view, setView] = useState<StorageView>({ kind: "overview" })
  const [confirmAccount, setConfirmAccount] = useState<string | null>(null)
  const [confirmFolder, setConfirmFolder] = useState<string | null>(null)
  const [removing, setRemoving] = useState(false)
  const removeLock = useRef(false)
  const alive = useRef(false)
  useEffect(() => {
    alive.current = true
    return () => { alive.current = false }
  }, [])
  const available = cloud.snapshot?.oauthAvailable === true
  const refresh = async () => { await local.reload(); await cloud.reload() }

  const removeFolder = async (id: string) => {
    if (removeLock.current) return
    removeLock.current = true
    setRemoving(true)
    try {
      const removed = await cloudStorageGateway.removeFolder(id)
      if (!alive.current) return
      setConfirmFolder(null)
      if (removed) showNotice("已移除云盘目录绑定；远端原文件与账户授权保留")
      await refresh()
    } catch (failure) { if (alive.current) showNotice(toHavenError(failure).message) }
    finally { removeLock.current = false; if (alive.current) setRemoving(false) }
  }
  if (view.kind === "connect") return <CloudConnectPanel key={view.accountId ?? "new"} available={available} accountId={view.accountId} onBack={() => setView({ kind: "overview" })}
    onAuthorized={account => { void refresh(); setView({ kind: "browse", account }) }} />
  if (view.kind === "browse") return <CloudBrowsePanel key={`${view.account.id}-${view.locationId ?? "choose"}`} account={view.account} locationId={view.locationId} onBack={() => { setView({ kind: "overview" }); void refresh() }}
    onRegistered={id => { void refresh(); setView({ kind: "browse", account: view.account, locationId: id }); showNotice("已登记云盘目录；选择 PDF 后加入媒体库") }}
    onImported={() => showNotice("PDF 已加入媒体库，可以在线阅读")} />

  return <div className="storage-downloads-settings">
    <ManagementHeading title="存储" description="管理媒体所在的位置。移除绑定不会删除原始文件。" />
    <LocalStorageLocations storage={local} />
    <ManagementSection feature="storage.googleDrivePdf" title="云盘" description="先授权账户，再选择媒体文件夹；不是同步或备份。"
      action={<button type="button" className="storage-action" disabled={cloud.loading || cloud.busy || cloud.snapshot === null} onClick={() => setView({ kind: "connect" })}>连接 Google Drive</button>}>
      {cloud.loading && <p className="storage-message" role="status">正在读取云盘状态…</p>}
      {cloud.error && <div role="alert" className="storage-error">{cloud.error.message}<button type="button" className="storage-action" onClick={() => void refresh()}>重新读取</button></div>}
      {cloud.snapshot?.oauthAvailable === false && !cloud.loading && !cloud.error && <p className="storage-message">当前应用未配置 Google 授权。可查看连接流程，暂不能发起授权。</p>}
      <table className="storage-table"><thead><tr><th scope="col">名称</th><th scope="col">类型</th><th scope="col">状态</th><th scope="col">操作</th></tr></thead><tbody>
        {cloud.snapshot?.accounts.length === 0 && <tr><td><strong>Google Drive</strong><span className="storage-detail">授权后添加目录，只读导入 PDF。</span></td><td>云盘</td><td>{available ? "未连接" : "未配置"}</td><td><button type="button" className="storage-action" onClick={() => setView({ kind: "connect" })}>{available ? "连接流程" : "查看连接流程"}</button></td></tr>}
        {cloud.snapshot?.accounts.map(account => <tr key={account.id}><td><strong>{account.displayName}</strong><span className="storage-detail">Google Drive · {cloud.folders.filter(folder => folder.accountId === account.id).length} 个媒体目录</span>
          {cloud.folders.filter(folder => folder.accountId === account.id).map(folder => {
            const location = local.locations.find(item => item.locationId === folder.locationId)
            return <div key={folder.locationId} className="storage-actions"><span className="storage-detail">{location?.displayName ?? "媒体目录"}</span>
              <button type="button" className="storage-action" disabled={!account.connected || !available || removing} onClick={() => setView({ kind: "browse", account, locationId: folder.locationId })}>浏览</button>
              <button type="button" className="storage-action storage-action-danger" disabled={removing} onClick={() => setConfirmFolder(folder.locationId)}>移除</button>
              {confirmFolder === folder.locationId && <><span className="storage-detail">只移除该目录的应用内索引。</span><button type="button" className="storage-action" onClick={() => setConfirmFolder(null)}>取消</button><button type="button" className="storage-action storage-action-danger" disabled={removing} onClick={() => void removeFolder(folder.locationId)}>确认移除</button></>}
            </div>
          })}</td><td>Google Drive</td><td>{account.connected ? available ? "已授权 · 只读" : "已授权 · 配置未就绪" : "已断开"}</td><td><div className="storage-actions">
            {account.connected ? <button type="button" className="storage-action" disabled={!available || cloud.busy} onClick={() => setView({ kind: "browse", account })}>添加目录</button>
              : <button type="button" className="storage-action" disabled={!available || cloud.busy} onClick={() => setView({ kind: "connect", accountId: account.id })}>重新授权</button>}
            {account.connected && <details className="storage-row-menu"><summary aria-label={`${account.displayName}的更多操作`}>···</summary><div><button type="button" className="storage-action" disabled={!available || cloud.busy} onClick={() => setView({ kind: "connect", accountId: account.id })}>重新授权</button><button type="button" className="storage-action storage-action-danger" disabled={cloud.busy} onClick={() => setConfirmAccount(account.id)}>断开账户</button></div></details>}
          </div>{confirmAccount === account.id && <div className="storage-actions" role="group" aria-label="确认断开云盘账户"><span className="storage-detail">该账户的全部目录将暂不可读，远端文件保留。</span><button type="button" className="storage-action" disabled={cloud.busy} onClick={() => setConfirmAccount(null)}>取消</button><button type="button" className="storage-action storage-action-danger" disabled={cloud.busy} onClick={async () => { if (await cloud.disconnect(account) && alive.current) { setConfirmAccount(null); await local.reload(); if (alive.current) showNotice("已断开云盘账户，远端文件保留") } }}>确认断开</button></div>}</td></tr>)}
      </tbody></table>
    </ManagementSection>
    <p className="storage-message">扫描与云盘读取均不移动原文件。移除媒体目录会清理 Haven 内由它派生的索引与关联记录；共享内容保留。</p>
  </div>
}
