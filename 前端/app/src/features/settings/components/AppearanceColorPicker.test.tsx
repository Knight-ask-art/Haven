// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"
import { AppearanceColorPicker } from "./AppearanceColorPicker"

afterEach(cleanup)

function renderPicker({
  value = "#f5f5f1",
  recentColors = [],
  contrastTargets = [{ label: "文字", color: "#2c2e29" }],
}: {
  value?: string
  recentColors?: string[]
  contrastTargets?: { label: string; color: string }[]
} = {}) {
  const onDraftChange = vi.fn<(color: string | null) => void>()
  const onApply = vi.fn<(color: string) => void>()
  const onRecentColor = vi.fn<(color: string) => void>()
  const rendered = render(
    <AppearanceColorPicker
      label="页面底色"
      value={value}
      contrastTargets={contrastTargets}
      recentColors={recentColors}
      onDraftChange={onDraftChange}
      onApply={onApply}
      onRecentColor={onRecentColor}
    />,
  )
  return { ...rendered, onDraftChange, onApply, onRecentColor }
}

describe("AppearanceColorPicker", () => {
  it("opens an in-context dialog below the swatch instead of the native system picker", () => {
    const { container } = renderPicker()
    const trigger = screen.getByRole("button", { name: "页面底色" })

    fireEvent.click(trigger)

    const dialog = screen.getByRole("dialog")
    expect(container.querySelector('input[type="color"]')).toBeNull()
    expect(screen.getByLabelText("Hex 色值")).toBeTruthy()
    expect(dialog.parentElement?.parentElement).toBe(trigger.closest("[data-color-picker-control]"))
    expect(container.querySelector("span[aria-hidden='true']")).toBeTruthy()
    expect(screen.getByText("本次外观页会话中应用过的颜色会显示在这里。")).toBeTruthy()
    expect(screen.getByRole("button", { name: "取消" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "应用颜色" })).toBeTruthy()
  })

  it("keeps invalid Hex drafts out of the settings callback", () => {
    const { onApply } = renderPicker()
    fireEvent.click(screen.getByRole("button", { name: "页面底色" }))
    fireEvent.change(screen.getByLabelText("Hex 色值"), { target: { value: "#12xz45" } })

    expect(screen.getByRole("alert").textContent).toContain("有效的")
    expect((screen.getByRole("button", { name: "应用颜色" }) as HTMLButtonElement).disabled).toBe(true)
    expect(onApply).not.toHaveBeenCalled()
  })

  it("accepts RGB values and applies the normalized six-digit color", () => {
    const { onApply } = renderPicker()
    fireEvent.click(screen.getByRole("button", { name: "页面底色" }))
    fireEvent.change(screen.getByLabelText("颜色格式"), { target: { value: "rgb" } })

    expect((screen.getByLabelText("R") as HTMLInputElement).value).toBe("245")
    fireEvent.change(screen.getByLabelText("R"), { target: { value: "18" } })
    fireEvent.change(screen.getByLabelText("G"), { target: { value: "52" } })
    fireEvent.change(screen.getByLabelText("B"), { target: { value: "86" } })
    fireEvent.click(screen.getByRole("button", { name: "应用颜色" }))

    expect(onApply).toHaveBeenCalledWith("#123456")
  })

  it("accepts HSL values and updates the live draft while editing", () => {
    const { onApply, onDraftChange } = renderPicker()
    fireEvent.click(screen.getByRole("button", { name: "页面底色" }))
    fireEvent.change(screen.getByLabelText("颜色格式"), { target: { value: "hsl" } })
    fireEvent.change(screen.getByLabelText("H"), { target: { value: "0" } })
    fireEvent.change(screen.getByLabelText("S"), { target: { value: "100" } })
    fireEvent.change(screen.getByLabelText("L"), { target: { value: "50" } })

    expect(onDraftChange).toHaveBeenLastCalledWith("#ff0000")
    fireEvent.click(screen.getByRole("button", { name: "应用颜色" }))
    expect(onApply).toHaveBeenCalledWith("#ff0000")
  })

  it("offers calm recommendations and selectable recent colors without blocking low-contrast choices", () => {
    const { onApply, onDraftChange } = renderPicker({
      value: "#f5f5f1",
      recentColors: ["#123456"],
      contrastTargets: [{ label: "文字", color: "#123456" }],
    })
    fireEvent.click(screen.getByRole("button", { name: "页面底色" }))
    fireEvent.click(screen.getByRole("button", { name: /羊皮纸/ }))

    expect(onDraftChange).toHaveBeenLastCalledWith("#f5f1e8")
    fireEvent.click(screen.getByRole("button", { name: "最近使用色 #123456" }))
    expect(screen.getByText(/与文字的对比度偏低/)).toBeTruthy()
    expect((screen.getByRole("button", { name: "应用颜色" }) as HTMLButtonElement).disabled).toBe(false)
    fireEvent.click(screen.getByRole("button", { name: "应用颜色" }))
    expect(onApply).toHaveBeenCalledWith("#123456")
  })

  it("cancels on outside click and Escape without applying a color", () => {
    const { onApply, onDraftChange } = renderPicker()
    const trigger = screen.getByRole("button", { name: "页面底色" })
    fireEvent.click(trigger)
    fireEvent.change(screen.getByLabelText("Hex 色值"), { target: { value: "#2c2e29" } })
    fireEvent.mouseDown(document.body)

    expect(screen.queryByRole("dialog")).toBeNull()
    expect(onApply).not.toHaveBeenCalled()
    expect(onDraftChange).toHaveBeenLastCalledWith(null)

    fireEvent.click(trigger)
    fireEvent.keyDown(window, { key: "Escape" })
    expect(screen.queryByRole("dialog")).toBeNull()
    expect(onApply).not.toHaveBeenCalled()
    expect(document.activeElement).toBe(trigger)
  })
})
