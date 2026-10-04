import { beforeEach, describe, expect, it, vi } from "vitest"
import type { DownloadEvent } from "@tauri-apps/plugin-updater"
import type { UpdaterProgress } from "./client"
const mocks = vi.hoisted(() => ({ check: vi.fn(), invoke: vi.fn() }))
vi.mock("@tauri-apps/plugin-updater", () => ({ check: mocks.check }))
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }))
import { TauriUpdaterClient } from "./updater-client"

function update() {
  return { currentVersion: "0.1.0-1", version: "0.1.0", body: "改进设置与搜索", date: "2026-10-04T00:00:00Z",
    download: vi.fn().mockResolvedValue(undefined), install: vi.fn().mockResolvedValue(undefined), close: vi.fn().mockResolvedValue(undefined) }
}
function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (cause: unknown) => void
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}
beforeEach(() => { vi.resetAllMocks(); mocks.invoke.mockResolvedValue(undefined) })

describe("native updater ownership and signature boundary", () => {
  it("deduplicates overlapping startup and manual checks", async () => {
    const pending = deferred<ReturnType<typeof update>>()
    mocks.check.mockReturnValueOnce(pending.promise)
    const client = new TauriUpdaterClient()
    const first = client.check()
    const second = client.check()
    expect(first).toBe(second)
    pending.resolve(update())
    await expect(first).resolves.toMatchObject({ status: "available" })
    expect(mocks.check).toHaveBeenCalledOnce()
  })

  it("retains the last candidate when a subsequent check fails and never leaks raw errors", async () => {
    const candidate = update()
    mocks.check.mockResolvedValueOnce(candidate).mockRejectedValueOnce("network https://example.invalid/?token=secret")
    const client = new TauriUpdaterClient()
    await client.check()
    await expect(client.check()).rejects.toMatchObject({ code: "UPDATER_CHECK_FAILED", retryable: true })
    expect(candidate.close).not.toHaveBeenCalled()
    await client.install()
    expect(candidate.install).toHaveBeenCalledOnce()
  })

  it("closes superseded candidates and clears a candidate on a successful up-to-date result", async () => {
    const first = update(), second = update()
    mocks.check.mockResolvedValueOnce(first).mockResolvedValueOnce(second).mockResolvedValueOnce(null)
    const client = new TauriUpdaterClient()
    await client.check(); await client.check()
    expect(first.close).toHaveBeenCalledOnce()
    expect(second.close).not.toHaveBeenCalled()
    await expect(client.check()).resolves.toMatchObject({ status: "up_to_date" })
    expect(second.close).toHaveBeenCalledOnce()
    await expect(client.install()).rejects.toMatchObject({ code: "UPDATER_NO_UPDATE" })
  })

  it("blocks checks and duplicate installation while a verified download is in flight", async () => {
    const pending = deferred<void>(), candidate = update()
    candidate.download.mockReturnValueOnce(pending.promise)
    mocks.check.mockResolvedValueOnce(candidate)
    const client = new TauriUpdaterClient()
    await client.check()
    const first = client.install(), second = client.install()
    expect(first).toBe(second)
    await expect(client.check()).rejects.toMatchObject({ code: "UPDATER_BUSY" })
    expect(candidate.close).not.toHaveBeenCalled()
    expect(mocks.invoke).not.toHaveBeenCalled()
    pending.resolve()
    await first
    expect(candidate.download).toHaveBeenCalledOnce()
    expect(candidate.install).toHaveBeenCalledOnce()
  })

  it("does not prepare, install, or expose sensitive transport data when signature verification fails", async () => {
    const candidate = update()
    candidate.download.mockRejectedValueOnce("Signature verification failed: https://example.invalid/?token=secret")
    mocks.check.mockResolvedValueOnce(candidate)
    const client = new TauriUpdaterClient()
    await client.check()
    const error = await client.install().catch((cause: unknown) => cause)
    expect(error).toMatchObject({ code: "UPDATER_SIGNATURE_INVALID", retryable: false })
    expect(JSON.stringify(error)).not.toContain("token")
    expect(candidate.install).not.toHaveBeenCalled()
    expect(mocks.invoke).not.toHaveBeenCalled()
  })

  it("projects real bytes and verification phase, and drains only after download resolves", async () => {
    const candidate = update(), pending = deferred<void>()
    let onEvent!: (event: DownloadEvent) => void
    candidate.download.mockImplementationOnce((callback: typeof onEvent) => { onEvent = callback; return pending.promise })
    mocks.check.mockResolvedValueOnce(candidate)
    const client = new TauriUpdaterClient(), progress: UpdaterProgress[] = []
    await client.check()
    const installing = client.install((value) => progress.push(value))
    onEvent({ event: "Started", data: { contentLength: 100 } })
    onEvent({ event: "Progress", data: { chunkLength: 40 } })
    onEvent({ event: "Progress", data: { chunkLength: 60 } })
    onEvent({ event: "Finished" })
    expect(progress.at(-1)).toEqual({ phase: "verifying", downloadedBytes: 100, totalBytes: 100 })
    expect(mocks.invoke).not.toHaveBeenCalled()
    pending.resolve()
    await installing
    expect(progress.at(-1)?.phase).toBe("installing")
    expect(mocks.invoke).toHaveBeenCalledWith("app_update_prepare")
    expect(candidate.install).toHaveBeenCalledWith({ restartAfterInstall: true })
    expect(candidate.close).toHaveBeenCalledOnce()
  })

  it("retries installation using already verified bytes without re-downloading", async () => {
    const candidate = update()
    candidate.install.mockRejectedValueOnce("installer denied").mockResolvedValueOnce(undefined)
    mocks.check.mockResolvedValueOnce(candidate)
    const client = new TauriUpdaterClient()
    await client.check()
    await expect(client.install()).rejects.toMatchObject({ code: "UPDATER_INSTALL_FAILED", retryable: true })
    expect(candidate.close).not.toHaveBeenCalled()
    await client.install()
    expect(candidate.download).toHaveBeenCalledOnce()
    expect(candidate.install).toHaveBeenCalledTimes(2)
    expect(candidate.close).toHaveBeenCalledOnce()
    expect(mocks.invoke.mock.calls.map(([command]) => command)).toEqual([
      "app_update_prepare", "app_update_cancel", "app_update_prepare", "app_update_cancel",
    ])
  })

  it("preserves a structured preparation error and does not cancel someone else's close", async () => {
    const candidate = update()
    mocks.check.mockResolvedValueOnce(candidate)
    mocks.invoke.mockRejectedValueOnce({ code: "UPDATER_BUSY", userMessage: "应用正在关闭", retryable: true })
    const client = new TauriUpdaterClient()
    await client.check()
    await expect(client.install()).rejects.toMatchObject({ code: "UPDATER_BUSY", retryable: true })
    expect(candidate.install).not.toHaveBeenCalled()
    expect(mocks.invoke).toHaveBeenCalledOnce()
  })

  it("fails closed if the preparation barrier cannot be released after installer failure", async () => {
    const candidate = update()
    candidate.install.mockRejectedValueOnce("installer denied")
    mocks.check.mockResolvedValueOnce(candidate)
    mocks.invoke.mockResolvedValueOnce(undefined).mockRejectedValueOnce("transport lost")
    const client = new TauriUpdaterClient()
    await client.check()
    await expect(client.install()).rejects.toMatchObject({ code: "UPDATER_RECOVERY_FAILED", retryable: false })
    expect(candidate.close).not.toHaveBeenCalled()
  })

  it("a UI progress callback cannot cancel native verification or installation", async () => {
    const candidate = update()
    mocks.check.mockResolvedValueOnce(candidate)
    const client = new TauriUpdaterClient()
    await client.check()
    await expect(client.install(() => { throw new Error("view unmounted") })).resolves.toEqual({ status: "installed" })
  })
})
