// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"
import { SettingsToggle } from "./SettingsToggle"

afterEach(cleanup)

describe("设置开关", () => {
  it("关闭态使用独立轨道配色，点击只发送切换意图", () => {
    const change = vi.fn()
    render(<SettingsToggle checked={false} label="自动继续" onChange={change} />)
    const toggle = screen.getByRole("switch", { name: "自动继续" })
    expect(toggle.getAttribute("aria-checked")).toBe("false")
    expect(toggle.className).toContain("bg-[var(--haven-settings-toggle-off)]")
    fireEvent.click(toggle)
    expect(change).toHaveBeenCalledExactlyOnceWith(true)
  })

  it("开启态使用强调色及对应前景滑块，不改变紧凑尺寸", () => {
    render(<SettingsToggle compact checked label="恢复上次状态" onChange={vi.fn()} />)
    const toggle = screen.getByRole("switch", { name: "恢复上次状态" })
    expect(toggle.getAttribute("aria-checked")).toBe("true")
    expect(toggle.className).toContain("bg-[var(--haven-settings-primary)]")
    expect(toggle.className).toContain("h-[24px] w-[42px]")
    expect(toggle.firstElementChild?.className).toContain("bg-[var(--haven-settings-toggle-thumb-on)]")
    expect(toggle.firstElementChild?.className).toContain("translate-x-[18px]")
  })

  it("禁用态保持不可点击，焦点样式不会增加持久化行为", () => {
    const change = vi.fn()
    render(<SettingsToggle disabled checked={false} label="关闭项" onChange={change} />)
    const toggle = screen.getByRole("switch", { name: "关闭项" })
    expect((toggle as HTMLButtonElement).disabled).toBe(true)
    expect(toggle.className).toContain("focus-visible:outline-[var(--haven-settings-primary)]")
    expect(toggle.className).toContain("enabled:hover:bg-[var(--haven-settings-toggle-off-hover)]")
    fireEvent.click(toggle)
    expect(change).not.toHaveBeenCalled()
  })
})
