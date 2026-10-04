import type { ReactNode } from "react"
import { BookOpen, Check, CircleCheck, Download, Folder, History, Home, Library, RefreshCw, ShieldCheck, TriangleAlert } from "lucide-react"
import { getHavenClientMode } from "@/lib/ipc/runtime"
import type { AppDirectoryKindDto } from "@/lib/ipc/generated/wire"
import { LAUNCH_PAGE_CHOICES, LAUNCH_PAGE_OPTIONS, launchPageChoice, optionLabel } from "../lib/settingsDisplay"
import { useAppInfo } from "../lib/useAppInfo"
import { useUpdater } from "../lib/useUpdater"
import type { SettingsFormController } from "../lib/useSettingsForm"
import { SettingsToggle } from "./SettingsToggle"
import { SystemSettingsIntro, SystemSettingsRow, SystemSettingsSection } from "./SystemSettingsLayout"

const LAUNCH_DESCRIPTIONS = {
  home: "从首页发现与继续阅读",
  library: "直接进入你的媒体库",
  continue: "接续最近阅读或播放的内容",
  last_session: "接续最近阅读或播放的内容",
} as const
const LAUNCH_ICONS = { home: Home, library: Library, continue: BookOpen, last_session: History }

export function GeneralSettings({ form }: { form: SettingsFormController }) {
  const value = form.displayValue
  if (value.section !== "general") return null
  const disabled = form.isLoading || form.isSaving || form.state.status === "load-error"
  const status = form.isLoading ? "正在读取设置…" : form.isSaving ? "正在保存…"
    : form.hasError ? "设置尚未保存" : form.isDirty ? "有未保存的修改" : "所有设置已保存"
  return <div className="system-settings">
    <SystemSettingsIntro section="General" title="通用" description="选择打开栖阅时的起点，让每一次阅读自然接续。" />
    <SystemSettingsSection title="启动行为" description="这些偏好会在下次启动时生效。">
      <fieldset disabled={disabled} className="system-settings__launch-fieldset" data-settings-features="general.launchPage" tabIndex={-1}>
        <legend>默认启动页</legend>
        <p className="system-settings__description">打开栖阅时，首先进入的空间。</p>
        <div className="system-settings__launch-options">
          {LAUNCH_PAGE_CHOICES.map((option) => {
            const Icon = LAUNCH_ICONS[option.value]
            return <label key={option.value} className="system-settings__launch-option">
              <input type="radio" name="general-launch-page" value={option.value} checked={launchPageChoice(value.launchPage) === option.value}
                onChange={() => form.change({ section: "general", launchPage: option.value })} />
              <span className="system-settings__launch-content">
                <Icon size={19} strokeWidth={1.7} aria-hidden="true" />
                <span><strong>{option.label}</strong><small>{LAUNCH_DESCRIPTIONS[option.value]}</small></span>
                <Check size={15} className="system-settings__launch-check" aria-hidden="true" />
              </span>
            </label>
          })}
        </div>
      </fieldset>
      <SystemSettingsRow feature="general.restoreSession" title="恢复上次状态" description="开启后优先接续最近的内容；没有可恢复内容时，使用上方选择的启动页。" icon={<History size={19} strokeWidth={1.7} />}>
        <SettingsToggle compact disabled={disabled} checked={value.restoreSession} label="恢复上次状态"
          onChange={(checked) => form.change({ section: "general", restoreSession: checked })} />
      </SystemSettingsRow>
      <p className="system-settings__footnote">当前启动页：{optionLabel(LAUNCH_PAGE_OPTIONS, value.launchPage)}{value.restoreSession && " · 优先恢复最近内容"}</p>
    </SystemSettingsSection>
    {form.hasError && <div role="alert" className="system-settings__error">
      <TriangleAlert size={17} aria-hidden="true" /><span>{form.errorMessage}</span>
      <button type="button" onClick={form.retry} className="system-settings__button system-settings__button--secondary">{form.state.status === "conflict" || form.state.status === "validation-error" ? "重新加载" : "重试"}</button>
    </div>}
    <footer className="system-settings__actions">
      <p role="status" className="system-settings__save-status">
        {form.isLoading || form.isSaving ? <RefreshCw size={15} className="system-settings__spin" aria-hidden="true" />
          : form.hasError || form.isDirty ? <TriangleAlert size={16} aria-hidden="true" /> : <CircleCheck size={16} aria-hidden="true" />}
        {status}
      </p>
      <div className="system-settings__button-group">
        <button type="button" disabled={disabled} onClick={form.resetToDefaults} className="system-settings__button system-settings__button--secondary">恢复默认</button>
        <button type="button" disabled={form.state.status !== "dirty"} onClick={form.save} className="system-settings__button system-settings__button--primary">{form.isSaving ? "正在保存…" : "保存修改"}</button>
      </div>
    </footer>
  </div>
}

export function UpdateSettings({ showNotice }: { showNotice: (message: string) => void }) {
  const app = useAppInfo()
  const updater = useUpdater()
  const preview = getHavenClientMode() === "mock"
  const busy = updater.status === "checking" || updater.status === "installing"
  const canInstall = updater.result?.status === "available" && (updater.status === "available" || (updater.status === "error" && updater.error?.retryable))
  const availableVersion = updater.result?.availableVersion
  const statusLabel = updater.status === "checking" ? "正在检查更新"
    : updater.status === "available" ? availableVersion ? `发现新版本 ${availableVersion}` : "发现新版本"
    : updater.status === "installing" ? updater.progress?.phase === "verifying" ? "正在校验更新包" : updater.progress?.phase === "installing" ? "正在启动安装器" : "正在下载更新"
    : updater.status === "installed" ? "更新已安装，请重新打开栖阅"
    : updater.status === "up_to_date" ? "已是最新版本"
    : updater.status === "error" ? "更新未完成" : "让栖阅保持最新"
  const statusDescription = updater.status === "checking" ? "正在连接更新服务，请稍候。"
    : updater.status === "available" ? "查看下方更新说明，准备好后再安装。"
    : updater.status === "installing" ? "更新包会先通过签名校验，再交由安装器处理。"
    : updater.status === "installed" ? "Windows 安装器会自动重新打开应用；重新打开后读取新版本。"
    : updater.status === "up_to_date" ? "当前没有需要安装的新版本。"
    : updater.status === "error" ? "当前版本仍可继续使用。请查看下方原因后再重试。"
    : preview ? "浏览器中可查看更新界面；检查与安装请在桌面应用中进行。" : "检查是否有新版本，安装由你决定。"
  const StatusIcon = updater.status === "error" ? TriangleAlert : updater.status === "up_to_date" ? CircleCheck
    : updater.status === "available" ? Download : RefreshCw
  const runInstall = async () => {
    if (await updater.install()) showNotice("更新已启动，应用将由系统安装器接管")
  }
  return <div className="system-settings">
    <SystemSettingsIntro section="Updates" title="更新" description="查看当前版本与更新状态，按自己的节奏升级。" />
    <section aria-label="应用更新" className="system-settings__surface" aria-busy={busy} data-settings-features="updates.application" tabIndex={-1}>
      <div className="system-settings__update-summary">
        <span className="system-settings__update-icon" data-error={updater.status === "error"}>
          <StatusIcon size={27} strokeWidth={1.6} className={busy ? "system-settings__spin" : undefined} aria-hidden="true" />
        </span>
        <div className="system-settings__update-copy" role="status" aria-live="polite">
          <p className="system-settings__eyebrow">栖阅应用</p><h3>{statusLabel}</h3><p>{statusDescription}</p>
          {updater.status === "installing" && updater.progress?.phase === "downloading" && <div className="system-settings__update-progress">
            <progress aria-label="更新下载进度" max={updater.progress.totalBytes || 1} value={updater.progress.totalBytes ? Math.min(updater.progress.downloadedBytes, updater.progress.totalBytes) : undefined} />
            <span>{(updater.progress.downloadedBytes / 1048576).toFixed(1)} MB{updater.progress.totalBytes ? ` / ${(updater.progress.totalBytes / 1048576).toFixed(1)} MB` : " · 正在下载"}</span>
          </div>}
        </div>
        <div className="system-settings__button-group">
          {canInstall && <button type="button" disabled={busy} onClick={() => void runInstall()} className="system-settings__button system-settings__button--primary"><Download size={15} aria-hidden="true" />{updater.status === "error" ? "重试安装" : "安装更新"}</button>}
          <button type="button" disabled={busy} onClick={() => void updater.check()} className={`system-settings__button system-settings__button--${updater.status === "available" ? "secondary" : "primary"}`}>
            <RefreshCw size={15} className={updater.status === "checking" ? "system-settings__spin" : undefined} aria-hidden="true" />
            {updater.status === "checking" ? "检查中…" : updater.status === "error" ? "重新检查" : "检查更新"}
          </button>
        </div>
      </div>
      <dl className="system-settings__version-strip">
        <div><dt>当前版本</dt><dd>{app.loading ? "读取中…" : app.info?.appVersion ?? "读取失败"}</dd></div>
        <div><dt>发布通道</dt><dd>{app.loading ? "读取中…" : preview ? "浏览器预览" : app.info?.buildChannel ?? "不可用"}</dd></div>
        <div><dt>安装方式</dt><dd>签名校验后安装</dd></div>
      </dl>
      {updater.status === "available" && <div className="system-settings__release-notes">
        <h4>更新说明{availableVersion ? ` · ${availableVersion}` : ""}</h4>
        {updater.result?.publishedAt && <p className="system-settings__description">发布时间：{updater.result.publishedAt}</p>}
        <p>{updater.result?.releaseNotes || "此版本未提供更新说明。"}</p>
      </div>}
      {updater.error && <div role="alert" className="system-settings__error"><TriangleAlert size={17} aria-hidden="true" /><span>{updater.error.dto.userMessage || "更新服务暂时不可用"}</span></div>}
      {app.error && <div role="alert" className="system-settings__error"><span>{app.error.dto.userMessage || "版本信息读取失败"}</span><button type="button" onClick={() => void app.reload()} className="system-settings__button system-settings__button--secondary">重新读取</button></div>}
    </section>
    <SystemSettingsSection title="更新策略" description="应用的检查与安装方式。">
      <SystemSettingsRow title="应用更新" description="启动时检查新版本，安装前由你确认。" icon={<RefreshCw size={19} strokeWidth={1.7} />}><span className="system-settings__value">检查后手动安装</span></SystemSettingsRow>
      <SystemSettingsRow feature="updates.sourcePack" title="Source Pack" description="随应用发布的内置来源包版本。" icon={<Library size={19} strokeWidth={1.7} />}><span className="system-settings__value">{app.loading ? "读取中…" : app.info?.sourcePackVersion ?? "未配置"}</span></SystemSettingsRow>
      <SystemSettingsRow title="更新包校验" description="签名校验通过后才会安装。" icon={<ShieldCheck size={19} strokeWidth={1.7} />}><span className="system-settings__value">自动校验</span></SystemSettingsRow>
    </SystemSettingsSection>
  </div>
}

const APP_DIRECTORY_KINDS: AppDirectoryKindDto[] = ["data", "logs", "cache"]
const DIRECTORY_DESCRIPTIONS = { data: "媒体库记录、阅读进度与应用设置", logs: "应用运行日志与问题排查记录", cache: "可重新生成的临时缓存" }

export function AboutSettings({ diagnostics }: { diagnostics: ReactNode }) {
  const { info, loading, error, opening, openError, reload, openDirectory } = useAppInfo()
  const preview = getHavenClientMode() === "mock"
  const directories = new Map(info?.directories.map((directory) => [directory.kind, directory]))
  const unavailable = loading ? "读取中…" : "不可用"
  return <div className="system-settings">
    <SystemSettingsIntro section="About" title="关于栖阅" description="了解当前版本、数据位置，以及构成栖阅的开源项目。" />
    <section aria-label="栖阅应用信息" className="system-settings__brand system-settings__surface">
      <img src="/logo.png" alt="栖阅 Haven 项目 Logo" width={76} height={76} />
      <div className="system-settings__brand-copy"><p className="system-settings__eyebrow">HAVEN</p><h3>栖阅</h3><p>让所有故事，在一个地方继续。</p></div>
      <div className="system-settings__brand-version"><span>{preview ? "浏览器预览" : "桌面应用"}</span><strong>{loading ? "读取中…" : info?.appVersion ?? "读取失败"}</strong><small>{loading ? "" : preview ? "界面与交互预览" : info?.buildChannel ?? ""}</small></div>
    </section>
    {error && <div role="alert" className="system-settings__error"><TriangleAlert size={17} aria-hidden="true" /><span>{error.dto.userMessage || "应用信息暂时不可用"}</span><button type="button" onClick={() => void reload()} className="system-settings__button system-settings__button--secondary">重试</button></div>}
    <SystemSettingsSection title="版本与许可">
      <SystemSettingsRow title="Source Pack 版本"><span className="system-settings__value">{info ? info.sourcePackVersion ?? "未配置" : unavailable}</span></SystemSettingsRow>
      <SystemSettingsRow title="IPC 协议版本"><span className="system-settings__value system-settings__mono">{info?.protocolVersion ?? unavailable}</span></SystemSettingsRow>
      <SystemSettingsRow title="数据库版本"><span className="system-settings__value system-settings__mono">{info?.databaseVersion ?? unavailable}</span></SystemSettingsRow>
      <SystemSettingsRow title="项目许可证" description="Haven 自有代码的许可证；依赖项目保留各自的许可约定。"><span className="system-settings__value">{info?.appLicense ?? unavailable}</span></SystemSettingsRow>
      <details className="system-settings__notices">
        <summary><span>开源依赖与致谢</span><small>{info ? `${info.thirdPartyNotices.length} 项许可摘要` : unavailable}</small></summary>
        <p className="system-settings__description">感谢这些开源项目。许可摘要来自随应用发布的第三方许可清单。</p>
        {info?.thirdPartyNotices.length ? <dl>{info.thirdPartyNotices.map((notice) => <div key={`${notice.name}-${notice.license}`}><dt>{notice.name}</dt><dd>{notice.license}</dd></div>)}</dl> : <p className="system-settings__description">{loading ? "正在读取许可摘要…" : "暂无已登记摘要"}</p>}
      </details>
    </SystemSettingsSection>
    <SystemSettingsSection feature="about.directories" title="本地目录" description="查看数据保存位置，或在文件管理器中打开。">
      {APP_DIRECTORY_KINDS.map((kind) => {
        const directory = directories.get(kind)
        const label = directory?.displayName ?? (kind === "data" ? "应用数据目录" : kind === "logs" ? "日志目录" : "缓存目录")
        return <div className="system-settings__directory" key={kind}>
          <span aria-hidden="true" className="system-settings__row-icon"><Folder size={19} strokeWidth={1.7} /></span>
          <div className="system-settings__directory-copy"><p className="system-settings__label">{label}</p><p className="system-settings__description">{DIRECTORY_DESCRIPTIONS[kind]}</p><p className="system-settings__path">{directory?.displayPath ?? unavailable}</p></div>
          <button type="button" aria-label={`打开${label}`} disabled={!directory?.canOpen || opening !== null} onClick={() => void openDirectory(kind)} className="system-settings__button system-settings__button--secondary"><Folder size={14} aria-hidden="true" />{opening === kind ? "打开中…" : "打开"}</button>
        </div>
      })}
      {openError && <div role="alert" className="system-settings__error"><TriangleAlert size={17} aria-hidden="true" /><span>{openError.dto.userMessage || "无法打开目录，请稍后重试"}</span></div>}
    </SystemSettingsSection>
    {diagnostics}
  </div>
}
