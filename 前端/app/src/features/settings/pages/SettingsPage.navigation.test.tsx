// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { StrictMode } from "react"
import { MemoryRouter, Route, Routes } from "react-router"
import { afterEach, describe, expect, it } from "vitest"
import { SettingsPage } from "./SettingsPage"
import { SETTINGS_REGISTRY } from "../lib/settings-registry"

afterEach(cleanup)
function renderSettings(initialEntry = "/settings/reading") {
  return render(<StrictMode><MemoryRouter initialEntries={[initialEntry]}><Routes>
    <Route path="/settings/:section" element={<SettingsPage />} />
  </Routes></MemoryRouter></StrictMode>)
}

describe("设置搜索到表单的真实页面路径", () => {
  it.each(SETTINGS_REGISTRY.filter((entry) => entry.id !== "ai"))("$id 的所有可搜索能力都有真实定位锚点", async (entry) => {
    const view = renderSettings(`/settings/${entry.id}`)
    await waitFor(() => {
      for (const feature of entry.features.filter((id) => id !== "reading.overview")) {
        expect(view.container.querySelector(`[data-settings-features~="${feature}"]`), feature).not.toBeNull()
      }
    })
  })

  it("搜索字号后定位排版区域，并继续使用真实阅读表单", async () => {
    renderSettings()
    fireEvent.change(screen.getByRole("textbox", { name: "搜索设置" }), { target: { value: "字号" } })
    fireEvent.keyDown(screen.getByRole("textbox", { name: "搜索设置" }), { key: "Enter" })
    await waitFor(() => expect(document.activeElement?.getAttribute("data-settings-features")).toBe("reading.typography"))
    expect(screen.getByRole("group", { name: "字号" })).toBeTruthy()
    expect(screen.queryByRole("dialog")).toBeNull()
  })

  it("搜索 API Key 会打开 AI 大表单；Ctrl+K 不会把焦点送到背景", async () => {
    renderSettings()
    const search = screen.getByRole("textbox", { name: "搜索设置" })
    fireEvent.change(search, { target: { value: "API Key" } })
    fireEvent.click(screen.getByRole("button", { name: /定位设置：API Key/ }))
    const dialog = await screen.findByRole("dialog", { name: "AI 设置" })
    await waitFor(() => expect(document.activeElement?.getAttribute("data-settings-features")).toContain("ai.providerCredential"))
    fireEvent.keyDown(document.activeElement!, { key: "k", ctrlKey: true })
    expect(dialog.contains(document.activeElement)).toBe(true)
    expect(document.activeElement).not.toBe(search)
    expect((search as HTMLInputElement).value).toBe("")
    fireEvent.click(screen.getByRole("button", { name: "关闭 AI 设置" }))
    expect(document.activeElement).toBe(search)
  })

  it("在 AI 工作台内搜索也保持目标焦点，不被 StrictMode 的弹窗生命周期覆盖", async () => {
    renderSettings("/settings/ai")
    const search = screen.getByRole("textbox", { name: "搜索设置" })
    fireEvent.change(search, { target: { value: "API Key" } })
    fireEvent.keyDown(search, { key: "Enter" })
    const dialog = await screen.findByRole("dialog", { name: "AI 设置" })
    await waitFor(() => expect(document.activeElement?.getAttribute("data-settings-features")).toContain("ai.providerCredential"))
    expect(document.activeElement).not.toBe(dialog)
    expect(dialog.contains(document.activeElement)).toBe(true)
  })

  it("搜索限速定位实际下载策略行，而不是只停在页面标题", async () => {
    renderSettings()
    const search = screen.getByRole("textbox", { name: "搜索设置" })
    fireEvent.change(search, { target: { value: "限速" } })
    fireEvent.keyDown(search, { key: "Enter" })
    await waitFor(() => expect(document.activeElement?.getAttribute("data-settings-features")).toBe("downloads.speedLimit"))
    expect(document.activeElement?.textContent).toContain("下载速度限制")
  })

  it("只读阅读统计搜索转到真正的总览消费者", async () => {
    renderSettings()
    const search = screen.getByRole("textbox", { name: "搜索设置" })
    fireEvent.change(search, { target: { value: "阅读总览" } })
    fireEvent.keyDown(search, { key: "Enter" })
    await waitFor(() => expect(document.activeElement?.getAttribute("data-settings-features")).toBe("reading.overview"))
    expect(screen.getByRole("heading", { name: "总览", level: 2 })).toBeTruthy()
  })
})
