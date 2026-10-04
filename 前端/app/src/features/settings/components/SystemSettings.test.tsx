// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { HavenError } from "@/lib/ipc/errors"
import { applySettingsPatch, defaultSettingsValue } from "@/lib/ipc/settings-wire"
import type { AppInfoDto, ErrorReportPreviewDto } from "@/lib/ipc/generated/wire"
import type { UpdaterCheckResult } from "@/lib/ipc/client"
import type { SettingsGateway } from "../ipc/gateway"
import { useSettingsForm } from "../lib/useSettingsForm"
import { AboutSettings, GeneralSettings, UpdateSettings } from "./SystemSettings"
import { ErrorReportSettings } from "../pages/SettingsPage"

const mocks = vi.hoisted(() => ({ get: vi.fn(), openDirectory: vi.fn(), check: vi.fn(), install: vi.fn(), preview: vi.fn(), confirm: vi.fn(), export: vi.fn(), openIssue: vi.fn(), push: vi.fn() }))
vi.mock("../ipc/app-info-gateway", () => ({ appInfoGateway: { get: mocks.get, openDirectory: mocks.openDirectory } }))
vi.mock("../ipc/updater-gateway", () => ({ updaterGateway: { check: mocks.check, install: mocks.install } }))
vi.mock("../ipc/error-report-gateway", async (importOriginal) => ({
  ...await importOriginal<typeof import("../ipc/error-report-gateway")>(),
  errorReportGateway: { preview: mocks.preview, confirm: mocks.confirm, export: mocks.export, openIssue: mocks.openIssue },
}))
vi.mock("@/app/notice-center/notice-context", () => ({ useNotice: () => ({ push: mocks.push }) }))
vi.mock("@/lib/ipc/runtime", () => ({ getHavenClientMode: () => "mock" }))

const appInfo: AppInfoDto = {
  schemaVersion: 1, appVersion: "0.1.0-test", buildChannel: "test", sourcePackVersion: null,
  protocolVersion: "test-v1", databaseVersion: "test-v2", appLicense: "MIT",
  thirdPartyNotices: [{ name: "React", license: "MIT" }],
  directories: [{ kind: "data", displayName: "应用数据目录", displayPath: "应用数据位置", exists: true, canOpen: true }],
}
const preview: ErrorReportPreviewDto = {
  schemaVersion: 1, reportId: "test-report", level: "standard", createdAt: "2026-10-04T00:00:00Z", appVersion: "test",
  operatingSystem: "test", runtimeMode: "mock", stableErrorCodes: [], errorSummary: "诊断摘要", details: null,
  redaction: { status: "passed", removedFields: [], containsSensitiveData: false }, requiresConfirmation: true,
}

beforeEach(() => {
  vi.resetAllMocks()
  mocks.get.mockResolvedValue(appInfo)
  mocks.openDirectory.mockResolvedValue(undefined)
  mocks.check.mockResolvedValue({ status: "up_to_date", currentVersion: null, availableVersion: null, releaseNotes: null, publishedAt: null } satisfies UpdaterCheckResult)
  mocks.install.mockResolvedValue({ status: "installed" })
  mocks.preview.mockResolvedValue(preview)
  mocks.confirm.mockResolvedValue({ schemaVersion: 1, reportId: "test-report", confirmed: true })
})
afterEach(cleanup)

function formGateway(): SettingsGateway {
  let saved = defaultSettingsValue("general")
  return {
    settingsGet: vi.fn(async () => ({ value: saved, revision: "test-revision" })),
    settingsUpdate: vi.fn(async (request) => {
      saved = applySettingsPatch(saved, request.patch)
      return { value: saved, revision: "next-revision", changed: true }
    }),
    preferenceGet: vi.fn(), preferenceUpdate: vi.fn(),
  }
}
function GeneralHarness({ gateway }: { gateway: SettingsGateway }) {
  return <GeneralSettings form={useSettingsForm("general", gateway)} />
}

describe("system settings interactions", () => {
  it("合并同义启动选项，旧 last_session 值显示为继续上次内容但不自动写回", async () => {
    const gateway = formGateway()
    const legacy = defaultSettingsValue("general")
    if (legacy.section !== "general") throw new Error("预期通用设置契约")
    vi.mocked(gateway.settingsGet).mockResolvedValue({ value: { ...legacy, launchPage: "last_session" }, revision: "legacy" })
    render(<GeneralHarness gateway={gateway} />)
    await screen.findByText("所有设置已保存")
    expect(screen.getAllByRole("radio")).toHaveLength(3)
    expect((screen.getByRole("radio", { name: /继续上次内容/ }) as HTMLInputElement).checked).toBe(true)
    expect(screen.queryByRole("radio", { name: /上次打开位置/ })).toBeNull()
    expect(gateway.settingsUpdate).not.toHaveBeenCalled()
  })
  it("saves the launch choice and restore switch through the existing CAS form, and reads them back on remount", async () => {
    const gateway = formGateway()
    const mounted = render(<GeneralHarness gateway={gateway} />)
    await screen.findByText("所有设置已保存")
    expect((screen.getByRole("button", { name: "保存修改" }) as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(screen.getByRole("radio", { name: /媒体库/ }))
    fireEvent.click(screen.getByRole("switch", { name: "恢复上次状态" }))
    expect(screen.getByText("有未保存的修改")).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: "保存修改" }))
    await screen.findByText("所有设置已保存")
    expect(gateway.settingsUpdate).toHaveBeenCalledWith({ section: "general", expectedRevision: "test-revision", patch: { section: "general", launchPage: "library", restoreSession: true } })
    mounted.unmount()
    render(<GeneralHarness gateway={gateway} />)
    await screen.findByText("所有设置已保存")
    expect((screen.getByRole("radio", { name: /媒体库/ }) as HTMLInputElement).checked).toBe(true)
    expect(screen.getByRole("switch").getAttribute("aria-checked")).toBe("true")
  })

  it("shows a load failure without claiming saved state, and disables editing until retry succeeds", async () => {
    const gateway = formGateway()
    vi.mocked(gateway.settingsGet).mockRejectedValueOnce(new HavenError({ code: "TEMPORARY", userMessage: "读取设置失败", retryable: true }))
    render(<GeneralHarness gateway={gateway} />)
    await screen.findByRole("alert")
    expect(screen.queryByText("所有设置已保存")).toBeNull()
    expect((screen.getByRole("radio", { name: /媒体库/ }).closest("fieldset") as HTMLFieldSetElement).disabled).toBe(true)
    fireEvent.click(screen.getByRole("button", { name: "重试" }))
    await screen.findByText("所有设置已保存")
    expect(gateway.settingsUpdate).not.toHaveBeenCalled()
  })

  it("keeps restore-defaults as a draft until it is saved", async () => {
    const gateway = formGateway()
    render(<GeneralHarness gateway={gateway} />)
    await screen.findByText("所有设置已保存")
    fireEvent.click(screen.getByRole("radio", { name: /媒体库/ }))
    fireEvent.click(screen.getByRole("button", { name: "保存修改" }))
    await screen.findByText("所有设置已保存")
    fireEvent.click(screen.getByRole("button", { name: "恢复默认" }))
    expect(screen.getByText("有未保存的修改")).toBeTruthy()
    expect(gateway.settingsUpdate).toHaveBeenCalledTimes(1)
  })

  it("renders pending and up-to-date results from the updater without allowing duplicate check clicks", async () => {
    let finish!: (value: UpdaterCheckResult) => void
    mocks.check.mockImplementationOnce(() => new Promise<UpdaterCheckResult>((resolve) => { finish = resolve }))
    render(<UpdateSettings showNotice={vi.fn()} />)
    fireEvent.click(screen.getByRole("button", { name: "检查更新" }))
    expect((screen.getByRole("button", { name: "检查中…" }) as HTMLButtonElement).disabled).toBe(true)
    await act(async () => finish({ status: "up_to_date", currentVersion: null, availableVersion: null, releaseNotes: null, publishedAt: null }))
    expect(await screen.findByText("已是最新版本")).toBeTruthy()
    expect(mocks.check).toHaveBeenCalledTimes(1)
  })

  it("only offers installation after a successful check, and treats release notes as plain text", async () => {
    mocks.check.mockResolvedValueOnce({ status: "available", availableVersion: "test-next", releaseNotes: "<script>bad()</script>", currentVersion: "test", publishedAt: null })
    render(<UpdateSettings showNotice={vi.fn()} />)
    expect(screen.queryByRole("button", { name: "安装更新" })).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "检查更新" }))
    await screen.findByRole("button", { name: "安装更新" })
    expect(screen.getByText("<script>bad()</script>")).toBeTruthy()
    expect(document.querySelector(".system-settings__release-notes script")).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "安装更新" }))
    await waitFor(() => expect(mocks.install).toHaveBeenCalledTimes(1))
  })

  it("exposes a readable updater error and can recover on retry", async () => {
    mocks.check.mockRejectedValueOnce(new HavenError({ code: "NETWORK", userMessage: "连接更新服务失败", retryable: true }))
    render(<UpdateSettings showNotice={vi.fn()} />)
    fireEvent.click(screen.getByRole("button", { name: "检查更新" }))
    expect((await screen.findByRole("alert")).textContent).toContain("连接更新服务失败")
    fireEvent.click(screen.getByRole("button", { name: "重新检查" }))
    await screen.findByText("已是最新版本")
    expect(screen.queryByRole("alert")).toBeNull()
  })

  it("shows real download progress and installer handoff without claiming the running process is current", async () => {
    mocks.check.mockResolvedValueOnce({ status: "available", availableVersion: "0.1.0", releaseNotes: null, currentVersion: "0.1.0-1", publishedAt: null })
    let finish!: () => void
    mocks.install.mockImplementationOnce((onProgress) => new Promise((resolve) => {
      onProgress({ phase: "downloading", downloadedBytes: 1048576, totalBytes: 2097152 })
      finish = () => resolve({ status: "installed" })
    }))
    render(<UpdateSettings showNotice={vi.fn()} />)
    fireEvent.click(screen.getByRole("button", { name: "检查更新" }))
    fireEvent.click(await screen.findByRole("button", { name: "安装更新" }))
    expect(await screen.findByText("1.0 MB / 2.0 MB")).toBeTruthy()
    expect((screen.getByRole("progressbar", { name: "更新下载进度" }) as HTMLProgressElement).value).toBe(1048576)
    expect((screen.getByRole("button", { name: "检查更新" }) as HTMLButtonElement).disabled).toBe(true)
    await act(async () => finish())
    expect(await screen.findByText("更新已安装，请重新打开栖阅")).toBeTruthy()
    expect(screen.queryByText("已是最新版本")).toBeNull()
  })

  it("allows a failed installer to retry but blocks signature failure until a new check", async () => {
    mocks.check.mockResolvedValueOnce({ status: "available", availableVersion: "0.1.0", releaseNotes: null, currentVersion: "0.1.0-1", publishedAt: null })
    mocks.install.mockRejectedValueOnce(new HavenError({ code: "UPDATER_INSTALL_FAILED", userMessage: "安装器未启动", retryable: true }))
    mocks.install.mockRejectedValueOnce(new HavenError({ code: "UPDATER_SIGNATURE_INVALID", userMessage: "签名校验失败", retryable: false }))
    render(<UpdateSettings showNotice={vi.fn()} />)
    fireEvent.click(screen.getByRole("button", { name: "检查更新" }))
    fireEvent.click(await screen.findByRole("button", { name: "安装更新" }))
    fireEvent.click(await screen.findByRole("button", { name: "重试安装" }))
    await screen.findByText("签名校验失败")
    expect(screen.queryByRole("button", { name: "重试安装" })).toBeNull()
    expect(screen.getByRole("button", { name: "重新检查" })).toBeTruthy()
  })

  it("keeps the available state readable when the version label is absent", async () => {
    mocks.check.mockResolvedValueOnce({ status: "available", availableVersion: null, releaseNotes: null, currentVersion: null, publishedAt: null })
    render(<UpdateSettings showNotice={vi.fn()} />)
    expect(screen.getByText("浏览器中可查看更新界面；检查与安装请在桌面应用中进行。")).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: "检查更新" }))
    await screen.findByText("发现新版本")
    expect(screen.getByRole("heading", { name: "更新说明" })).toBeTruthy()
  })

  it("keeps the browser label honest and opens only a backend-provided directory kind", async () => {
    render(<AboutSettings diagnostics={null} />)
    await screen.findByText("0.1.0-test")
    expect(screen.getByText("浏览器预览")).toBeTruthy()
    expect((screen.getByRole("button", { name: "打开日志目录" }) as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(screen.getByRole("button", { name: "打开应用数据目录" }))
    await waitFor(() => expect(mocks.openDirectory).toHaveBeenCalledWith("data"))
    expect(screen.getByText("React")).toBeTruthy()
    expect(screen.getByText("开源依赖与致谢").closest("details")).toBeTruthy()
  })

  it("shows report generation errors even before any preview exists", async () => {
    mocks.preview.mockRejectedValueOnce(new HavenError({ code: "REPORT", userMessage: "诊断报告生成失败", retryable: true }))
    render(<div className="system-settings"><ErrorReportSettings /></div>)
    fireEvent.click(screen.getByRole("button", { name: "生成预览" }))
    expect((await screen.findByRole("alert")).textContent).toContain("诊断报告生成失败")
    fireEvent.click(screen.getByRole("button", { name: "重试" }))
    await screen.findByText("诊断摘要")
  })

  it("requires report confirmation, freezes level changes during confirmation, and restores the control afterward", async () => {
    let finish!: () => void
    mocks.confirm.mockImplementationOnce(() => new Promise((resolve) => { finish = () => resolve({ confirmed: true }) }))
    render(<div className="system-settings"><ErrorReportSettings /></div>)
    fireEvent.click(screen.getByRole("button", { name: "生成预览" }))
    await screen.findByText("诊断摘要")
    expect(screen.queryByRole("button", { name: "导出诊断报告" })).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "我已检查，允许使用" }))
    expect((screen.getByRole("combobox", { name: "报告等级" }) as HTMLSelectElement).disabled).toBe(true)
    await act(async () => finish())
    expect(await screen.findByRole("button", { name: "导出诊断报告" })).toBeTruthy()
    expect((screen.getByRole("combobox", { name: "报告等级" }) as HTMLSelectElement).disabled).toBe(false)
    expect(mocks.export).not.toHaveBeenCalled()
  })
})
