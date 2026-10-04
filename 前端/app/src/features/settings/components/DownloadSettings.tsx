import { Link } from "react-router"
import type { SettingsFormController } from "../lib/useSettingsForm"
import { DOWNLOAD_CONCURRENCY_OPTIONS, DOWNLOAD_SPEED_LIMIT_OPTIONS } from "../lib/settingsDisplay"
import { ManagementHeading, ManagementSection } from "./StorageSettingsLayout"
import { SettingsToggle } from "./SettingsToggle"

/** 下载策略只有 draft；保存依然由 SettingsForm 的 CAS / Gateway 完成。 */
export function DownloadSettings({ form }: { form: SettingsFormController }) {
  const value = form.displayValue
  if (value.section !== "downloads") return null
  const disabled = form.isLoading || form.isSaving || form.state.status === "load-error"
  const status = form.isLoading ? "正在加载…" : form.state.status === "load-error" ? "无法读取当前设置" : form.isSaving ? "正在保存…" : form.hasError
    ? "设置尚未保存" : form.isDirty ? "有未保存的修改" : "设置已保存"
  return <div className="storage-downloads-settings">
    <ManagementHeading title="下载" description="调整离线任务的排队、速度和中断恢复。" />
    <ManagementSection title="下载策略" description="保存后用于新开始或恢复的任务，不会强制中断正在运行的任务。">
      <div className="download-policy-row" data-settings-features="downloads.concurrency" tabIndex={-1}>
        <div><h4 id="download-concurrency-label">同时下载数量</h4><p>超过数量的任务会等待；同一主机最多同时执行 2 个。</p></div>
        <div role="group" aria-labelledby="download-concurrency-label" className="download-segment">
          {DOWNLOAD_CONCURRENCY_OPTIONS.map(option => <button type="button" key={option.value} disabled={disabled}
            aria-pressed={value.concurrentTasks === option.value} onClick={() => form.change({ section: "downloads", concurrentTasks: option.value })}>{option.label}</button>)}
        </div>
      </div>
      <div className="download-policy-row" data-settings-features="downloads.speedLimit" tabIndex={-1}>
        <div><h4 id="download-speed-label">下载速度限制</h4><p>用于当前离线文件保存任务；不是影片画质设置。</p></div>
        <div role="group" aria-labelledby="download-speed-label" className="download-segment">
          {DOWNLOAD_SPEED_LIMIT_OPTIONS.map(option => <button type="button" key={option.value} disabled={disabled}
            aria-pressed={value.speedLimit === option.value} onClick={() => form.change({ section: "downloads", speedLimit: option.value })}>{option.label}</button>)}
        </div>
      </div>
      <div className="download-policy-row" data-settings-features="downloads.autoContinue" tabIndex={-1}><div><h4>自动继续中断任务</h4><p>重新打开 Haven 后继续意外中断的任务，主动暂停的任务保持暂停。</p></div>
        <SettingsToggle compact checked={value.autoContinue} disabled={disabled} label="自动继续中断任务"
          onChange={autoContinue => form.change({ section: "downloads", autoContinue })} />
      </div>
    </ManagementSection>
    <div className="storage-management-note"><div><h4>离线位置由已注册的存储管理</h4><p>设置不会移动原始文件；管理本地媒体目录，请前往存储。</p></div>
      <Link to="/settings/storage" className="storage-action">管理存储</Link>
    </div>
    {form.hasError && <div role="alert" className="storage-error">{form.errorMessage}<button type="button" className="storage-action" onClick={form.retry}>{form.state.status === "conflict" ? "重新加载" : "重试"}</button></div>}
    <footer className="storage-management-note"><p role="status">{status}</p><div className="storage-actions">
      <button type="button" className="storage-action" disabled={disabled} onClick={form.resetToDefaults}>恢复默认</button>
      <button type="button" className="storage-action storage-action-primary" disabled={disabled || !form.isDirty || form.hasError} onClick={form.save}>保存修改</button>
    </div></footer>
  </div>
}
