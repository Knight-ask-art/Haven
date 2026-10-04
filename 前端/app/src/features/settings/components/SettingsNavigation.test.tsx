// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"
import { SettingsNavigation } from "./SettingsNavigation"

afterEach(cleanup)

describe("设置导航与真实能力搜索", () => {
  it("图标与文字使用独立布局槽，不让遮罩图标分走文字宽度", () => {
    render(<SettingsNavigation activeId="sources" onSelectSection={vi.fn()} onSelectFeature={vi.fn()} />)
    const navigation = screen.getByRole("navigation", { name: "设置分类" })
    const items = navigation.querySelectorAll(".settings-navigation__item")
    expect(items.length).toBeGreaterThan(0)
    for (const item of items) {
      const icon = item.querySelector(":scope > .settings-navigation__icon") as HTMLElement
      const text = item.querySelector(":scope > .settings-navigation__text")
      expect(icon?.getAttribute("aria-hidden")).toBe("true")
      expect(icon?.style.width).toBe("17px")
      expect(text?.querySelector("strong")?.textContent).toBeTruthy()
      expect(text?.querySelector("small")?.textContent).toBeTruthy()
    }
  })

  it("找到实际设置项，点击或回车交给表单定位，不展示规划能力", () => {
    const select = vi.fn()
    render(<SettingsNavigation activeId="sources" onSelectSection={vi.fn()} onSelectFeature={select} />)
    const input = screen.getByRole("textbox", { name: "搜索设置" })
    fireEvent.change(input, { target: { value: "API Key" } })
    fireEvent.click(screen.getByRole("button", { name: /定位设置：API Key/ }))
    expect(select).toHaveBeenLastCalledWith(expect.objectContaining({ section: "ai", featureId: "ai.providerCredential" }))
    expect((input as HTMLInputElement).value).toBe("")
    fireEvent.change(input, { target: { value: "字号" } })
    fireEvent.keyDown(input, { key: "Enter" })
    expect(select).toHaveBeenLastCalledWith(expect.objectContaining({ section: "reading", featureId: "reading.typography" }))
    fireEvent.change(input, { target: { value: "OCR" } })
    expect(screen.getByText("没有找到匹配的设置。")).toBeTruthy()
  })

  it("普通快捷键可搜索，但模态窗口存在时不抢焦点；卸载后移除监听", () => {
    const view = render(<SettingsNavigation activeId="reading" onSelectSection={vi.fn()} onSelectFeature={vi.fn()} />)
    const input = screen.getByRole("textbox", { name: "搜索设置" })
    const focus = vi.spyOn(input, "focus")
    fireEvent.keyDown(document, { key: "k", ctrlKey: true })
    expect(focus).toHaveBeenCalledTimes(1)
    const modal = document.createElement("dialog")
    modal.setAttribute("open", "")
    document.body.appendChild(modal)
    fireEvent.keyDown(document, { key: "k", ctrlKey: true })
    expect(focus).toHaveBeenCalledTimes(1)
    modal.remove()
    view.unmount()
    fireEvent.keyDown(document, { key: "k", ctrlKey: true })
    expect(focus).toHaveBeenCalledTimes(1)
    focus.mockRestore()
  })

  it("窄窗口分类使用同一真实导航列表，不展示未登记的同步入口或隐藏侧栏按钮", () => {
    const select = vi.fn()
    render(<SettingsNavigation activeId="sources" onSelectSection={select} onSelectFeature={vi.fn()} />)
    fireEvent.change(screen.getByRole("combobox", { name: "设置分类选择" }), { target: { value: "reading" } })
    expect(select).toHaveBeenCalledWith("reading")
    expect(screen.queryByRole("option", { name: "同步与备份" })).toBeNull()
    expect(screen.queryByRole("button", { name: /收起|隐藏侧栏/ })).toBeNull()
  })
})
