import type { CloudAccountDto } from "@/lib/ipc/generated/wire"
import { useCloudAuthorization } from "../lib/useCloudAuthorization"
import { ManagementHeading, ManagementSection } from "./StorageSettingsLayout"

const AUTH_STATUS = {
  idle: "尚未授权", starting: "正在打开系统浏览器…", pending: "等待 Google 授权回调…",
  completing: "正在保存授权…", authorized: "授权已返回", completed: "授权已完成",
  cancelled: "已取消授权", expired: "授权已过期，请重新发起", failed: "授权未完成，请重试",
}

/** Figma 的账户授权 / 选择媒体目录两步流程；不把登录成功当作导入成功。 */
export function CloudConnectPanel({ available, accountId, onBack, onAuthorized }: {
  available: boolean; accountId?: string; onBack: () => void; onAuthorized: (account: CloudAccountDto) => void
}) {
  const auth = useCloudAuthorization(accountId, onAuthorized)
  return <div className="storage-downloads-settings">
    <ManagementHeading title="连接 Google Drive" description="授权与选择目录分别完成，只读接入你的媒体文件。" />
    <div className="storage-connect-steps"><button type="button" className="storage-action" onClick={onBack}>返回存储</button><span aria-current="step">01 授权账户</span><span aria-hidden="true">→</span><span>02 选择媒体文件夹</span></div>
    <ManagementSection title="Google Drive 只读连接" description="使用系统浏览器授权。Haven 不会在这个页面索取账户密码。">
      <dl className="storage-connect-facts"><dt>账户</dt><dd aria-live="polite">{AUTH_STATUS[auth.status]}<p>{available ? "在系统浏览器中选择 Google 账户并确认授权。" : "当前应用未配置 Google OAuth 客户端，暂不能发起授权。"}</p></dd>
        <dt>授权范围</dt><dd>Google Drive 账户级只读权限<p>Google 的授权可读取账户内文件；Haven 只登记你确认的媒体目录。不上传、不删除云盘文件，不自动同步或备份。</p></dd>
        <dt>媒体文件夹</dt><dd>授权后选择<p>确认文件夹后才添加媒体库位置，再选择 PDF 导入；登录成功不等于已经导入内容。</p></dd>
      </dl>
      {auth.error && <div role="alert" className="storage-error">{auth.error.message}</div>}
      <div className="storage-browse-footer"><p className="storage-detail">取消或失败不会移除原有目录。</p><div className="storage-actions">
        {auth.pending && <button type="button" className="storage-action" onClick={() => void auth.cancel()}>取消授权</button>}
        <button type="button" className="storage-action storage-action-primary" disabled={!available || auth.pending || auth.status === "completed"} onClick={() => void auth.begin()}>打开 Google 授权页</button>
      </div></div>
    </ManagementSection>
  </div>
}
