// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { StrictMode } from "react"
import { afterEach, describe, expect, it, vi } from "vitest"
import { SettingsDialog } from "./SettingsDialog"

afterEach(cleanup)

describe("设置大表单窗口", () => {
  it("可访问标题、描述，进入窗口时聚焦并循环 Tab，卸载时恢复原焦点", () => {
    const opener = document.createElement("button")
    document.body.appendChild(opener)
    opener.focus()
    const view = render(<SettingsDialog title="来源设置" description="表单说明" onClose={vi.fn()}>
      <input aria-label="名称" /><button type="button">保存</button>
    </SettingsDialog>)
    const dialog = screen.getByRole("dialog", { name: "来源设置" })
    const first = screen.getByRole("button", { name: "取消" })
    const last = screen.getByRole("button", { name: "保存" })
    expect(dialog.getAttribute("aria-modal")).toBe("true")
    expect(document.getElementById(dialog.getAttribute("aria-describedby")!)?.textContent).toBe("表单说明")
    expect(document.activeElement).toBe(dialog)
    fireEvent.keyDown(dialog, { key: "Tab" })
    expect(document.activeElement).toBe(first)
    fireEvent.keyDown(first, { key: "Tab", shiftKey: true })
    expect(document.activeElement).toBe(last)
    fireEvent.keyDown(last, { key: "Tab" })
    expect(document.activeElement).toBe(first)
    view.unmount()
    expect(document.activeElement).toBe(opener)
    opener.remove()
  })

  it("Escape、取消与背景关闭，点击表单内部不关闭", () => {
    const close = vi.fn()
    render(<SettingsDialog title="添加来源" onClose={close}><input aria-label="名称" /></SettingsDialog>)
    fireEvent.mouseDown(screen.getByRole("textbox").closest(".settings-dialog")!)
    expect(close).not.toHaveBeenCalled()
    fireEvent(screen.getByRole("dialog"), new Event("cancel", { cancelable: true }))
    fireEvent.click(screen.getByRole("button", { name: "取消" }))
    fireEvent.mouseDown(document.querySelector(".settings-dialog-backdrop")!)
    expect(close).toHaveBeenCalledTimes(3)
  })

  it("提交期间禁止取消，操作结束后使用最新关闭回调，卸载移除监听", () => {
    const before = vi.fn()
    const after = vi.fn()
    const view = render(<SettingsDialog title="配置" busy onClose={before}><input /></SettingsDialog>)
    fireEvent(screen.getByRole("dialog"), new Event("cancel", { cancelable: true }))
    fireEvent.mouseDown(document.querySelector(".settings-dialog-backdrop")!)
    expect((screen.getByRole("button", { name: "取消" }) as HTMLButtonElement).disabled).toBe(true)
    expect(before).not.toHaveBeenCalled()
    view.rerender(<SettingsDialog title="配置" onClose={after}><input /></SettingsDialog>)
    fireEvent(screen.getByRole("dialog"), new Event("cancel", { cancelable: true }))
    expect(after).toHaveBeenCalledTimes(1)
    view.unmount()
    fireEvent.keyDown(document, { key: "Escape" })
    expect(after).toHaveBeenCalledTimes(1)
  })

  it("阻止全局搜索快捷键进入背景，但让内部控件先处理 Escape", () => {
    const close = vi.fn()
    const search = vi.fn()
    document.addEventListener("keydown", search)
    render(<SettingsDialog title="AI 设置" onClose={close}><input aria-label="名称" onKeyDown={(event) => { if (event.key === "Escape") event.preventDefault() }} /></SettingsDialog>)
    const dialog = screen.getByRole("dialog")
    expect(dialog.tagName).toBe("DIALOG")
    expect(dialog.hasAttribute("open")).toBe(true)
    fireEvent.keyDown(dialog, { key: "k", ctrlKey: true })
    fireEvent.keyDown(dialog, { key: "k", metaKey: true })
    expect(search).not.toHaveBeenCalled()
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Escape" })
    expect(close).not.toHaveBeenCalled()
    fireEvent(dialog, new Event("cancel", { cancelable: true }))
    expect(close).toHaveBeenCalledTimes(1)
    document.removeEventListener("keydown", search)
  })

  it("初始定位由模态窗口完成，StrictMode 重挂载后保持在指定表单内", () => {
    render(<StrictMode><SettingsDialog title="AI 设置" initialFocusSelector='[data-settings-features="ai.providerCredential"]' onClose={vi.fn()}>
      <section tabIndex={-1} data-settings-features="ai.providerCredential"><input aria-label="名称" /></section>
    </SettingsDialog></StrictMode>)
    expect(document.activeElement?.getAttribute("data-settings-features")).toBe("ai.providerCredential")
    expect(screen.getByRole("dialog").contains(document.activeElement)).toBe(true)
  })
})
