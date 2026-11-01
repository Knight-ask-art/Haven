// @vitest-environment jsdom
import { StrictMode } from "react"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { EpisodeDrawer } from "./EpisodeDrawer"

const cast = vi.hoisted(() => ({ play: vi.fn(), stop: vi.fn() }))
vi.mock("@/features/cast/ipc/cast-gateway", () => ({ playCast: cast.play, stopCast: cast.stop }))
vi.mock("@/features/player/hooks/useCastDiscovery", () => ({
  useCastDiscovery: () => ({ devices: [{ deviceId: "tv", friendlyName: "Test TV", protocol: "dlna", ip: "192.0.2.1" }], error: null }),
}))

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}
const props = { isOpen: true, onClose: vi.fn(), episodes: [{ id: "episode", number: 1, title: "Episode" }], currentEpisodeId: "episode", castMediaItemId: "media", onSelectEpisode: vi.fn() }
const result = { schemaVersion: 1, castSessionId: "grant-a", lanUrl: "http://192.0.2.1/video", deviceName: "TV" }
beforeEach(() => { cast.play.mockReset(); cast.stop.mockReset().mockResolvedValue({ stopped: true }); localStorage.clear() })
afterEach(cleanup)

function selectTv() {
  fireEvent.click(screen.getByRole("button", { name: "投屏" }))
  fireEvent.click(screen.getByText("Test TV"))
}

it.each([
  ["resume", "继续下载"],
  ["retry", "重试下载"],
] as const)("keeps the %s action available for the existing episode task", async (action, label) => {
  const onDownloadCurrent = vi.fn(async () => null)
  render(<EpisodeDrawer {...props}
    downloadStateOf={() => ({ state: "recoverable", action, label })}
    onDownloadAll={vi.fn(async () => null)}
    onDownloadCurrent={onDownloadCurrent}
  />)
  const control = screen.getByRole("button", { name: label })
  expect(control.hasAttribute("disabled")).toBe(false)
  fireEvent.click(control)
  fireEvent.click(screen.getByRole("button", { name: new RegExp(`${label}本集`) }))
  await waitFor(() => expect(onDownloadCurrent).toHaveBeenCalledTimes(1))
})

it("serializes double start and revokes a late successful grant after close, including StrictMode replay", async () => {
  const start = deferred<typeof result>()
  const stop = deferred<{ stopped: boolean }>()
  cast.play.mockReturnValue(start.promise)
  cast.stop.mockReturnValue(stop.promise)
  const view = render(<StrictMode><EpisodeDrawer {...props} /></StrictMode>)
  selectTv()
  const button = screen.getByRole("button", { name: "开始投屏" })
  act(() => { fireEvent.click(button); fireEvent.click(button) })
  expect(cast.play).toHaveBeenCalledTimes(1)
  view.rerender(<StrictMode><EpisodeDrawer {...props} isOpen={false} /></StrictMode>)
  await act(async () => { start.resolve(result); await start.promise })
  await waitFor(() => expect(cast.stop).toHaveBeenCalledWith("grant-a"))
  view.rerender(<StrictMode><EpisodeDrawer {...props} /></StrictMode>)
  expect(screen.getByRole("button", { name: /正在.*投屏/ }).hasAttribute("disabled")).toBe(true)
  await act(async () => { stop.resolve({ stopped: true }); await stop.promise })
  await waitFor(() => expect(screen.getByRole("button", { name: "开始投屏" }).hasAttribute("disabled")).toBe(false))
})

it("revokes a successful start that returns after the drawer unmounts", async () => {
  const start = deferred<typeof result>()
  cast.play.mockReturnValue(start.promise)
  const view = render(<EpisodeDrawer {...props} />)
  selectTv()
  fireEvent.click(screen.getByRole("button", { name: "开始投屏" }))
  view.unmount()
  await act(async () => { start.resolve(result); await start.promise })
  await waitFor(() => expect(cast.stop).toHaveBeenCalledWith("grant-a"))
})

it("keeps an established grant retryable after a stop fails", async () => {
  cast.play.mockResolvedValue(result)
  cast.stop.mockRejectedValueOnce(new Error("TV offline"))
  render(<EpisodeDrawer {...props} />)
  selectTv()
  fireEvent.click(screen.getByRole("button", { name: "开始投屏" }))
  await waitFor(() => expect(screen.getByRole("button", { name: "停止投屏" })).toBeTruthy())
  fireEvent.click(screen.getByRole("button", { name: "停止投屏" }))
  await screen.findByText("停止投屏失败，请重试")
  fireEvent.click(screen.getByRole("button", { name: "停止投屏" }))
  await waitFor(() => expect(screen.getByRole("button", { name: "开始投屏" })).toBeTruthy())
  expect(cast.stop).toHaveBeenCalledTimes(2)
})
