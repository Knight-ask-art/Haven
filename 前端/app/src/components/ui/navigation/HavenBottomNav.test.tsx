// @vitest-environment jsdom

import { afterEach, describe, expect, it } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { MemoryRouter } from "react-router"
import { HavenBottomNav } from "./HavenBottomNav"

afterEach(cleanup)

function renderNav(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <HavenBottomNav />
    </MemoryRouter>,
  )
}

/** 主导航项 / 「更多」按钮：图标本身没有可访问名，用文案定位最稳。 */
function itemByLabel(label: string): HTMLElement {
  const element = screen.getByText(label).closest("a, button")
  if (!(element instanceof HTMLElement)) throw new Error(`${label} 必须渲染成链接或按钮`)
  return element
}

const HARDCODED_BLUE = "#1f8fff"

/**
 * Dock 的材质面 token：半透明底、描边与静止态文字都必须走它们。
 *
 * 这几个 token 之所以存在，是因为它们的取值带 alpha，而 Tailwind 对值是裸 `var(...)`
 * 的颜色会把带 `/NN` 修饰符的工具类整条丢掉（生成不出 CSS）。写成 token 之后，
 * appearance-runtime 在自定义主题下能用用户调色板重写它们。
 */
const DOCK_SURFACE = "bg-[var(--haven-floating-surface)]"
const DOCK_SURFACE_RAISED = "bg-[var(--haven-floating-surface-raised)]"
const DOCK_BORDER = "border-[var(--haven-floating-border)]"
const DOCK_IDLE_TEXT = "text-[var(--haven-floating-idle)]"
const DOCK_IDLE_HOVER = "hover:bg-[var(--haven-floating-idle-hover)]"

/**
 * 写死颜色的工具类：调色板类前缀 + 任意值里的颜色字面量。
 *
 * 刻意不含 `shadow-`：投影阴影表达的是景深，不属于调色板（深浅两套主题本来就有各自的
 * 阴影取值），把它算进来只会得到一条无法满足的断言。
 */
const HARDCODED_COLOR_UTILITY =
  /(?:^|[\s:])(?:bg|text|border|ring|fill|stroke|from|to|via|divide|outline|caret|decoration|placeholder|accent)-\[[^\]]*(?:#|rgba?\(|hsla?\()/

describe("floating dock accent", () => {
  it("paints the active main item with the theme primary token, not a hardcoded blue", () => {
    renderNav("/library")

    const active = itemByLabel("媒体库")
    expect(active.className).toContain("text-primary")
    // 写死蓝色的活跃态会让 Dock 成为唯一不跟随自定义强调色的一块。
    expect(active.className).not.toContain(HARDCODED_BLUE)

    // 未激活项保持原来的中性灰，不跟着强调色走。
    const inactive = itemByLabel("首页")
    expect(inactive.className).not.toContain("text-primary")
    expect(inactive.className).toContain(DOCK_IDLE_TEXT)
  })

  it("uses the same token for the active “更多” state", () => {
    renderNav("/settings")

    const more = itemByLabel("更多")
    expect(more.className).toContain("text-primary")
    expect(more.className).not.toContain(HARDCODED_BLUE)
  })

  it("does not mark “更多” active outside its own routes", () => {
    renderNav("/library")
    expect(itemByLabel("更多").className).not.toContain("text-primary")
  })

  it("keeps the dock a transparent, blurred floating bar", () => {
    const { container } = renderNav("/library")

    // 原来的透明底部导航行为：浮在内容之上、半透明底 + 背景模糊。
    const nav = container.querySelector('nav[aria-label="全局浮动导航栏"]')
    if (!(nav instanceof HTMLElement)) throw new Error("必须渲染出浮动导航栏")
    expect(nav.className).toContain("fixed")
    expect(nav.className).toContain("z-50")

    const dock = nav.querySelector("div")
    if (!(dock instanceof HTMLElement)) throw new Error("必须渲染出 Dock 主体")
    expect(dock.className).toContain("backdrop-blur-2xl")
    expect(dock.className).toContain(DOCK_SURFACE)
    expect(dock.className).toContain(DOCK_BORDER)
  })

  it("leaves no hardcoded color on any dock surface", () => {
    // 浮动 Dock 是改动前整屏唯一不跟随主题的一块：材质面一旦重新写死颜色，自定义
    // 调色板就再也覆盖不到它。这条守住「Dock 的每一层都来自 token」——菜单展开时
    // 抬升面也在 DOM 里，因此两态一起检查。
    const { container } = renderNav("/library")
    fireEvent.click(itemByLabel("更多"))

    const nav = container.querySelector('nav[aria-label="全局浮动导航栏"]')
    if (!(nav instanceof HTMLElement)) throw new Error("必须渲染出浮动导航栏")

    for (const element of Array.from(nav.querySelectorAll<HTMLElement>("[class]"))) {
      const match = HARDCODED_COLOR_UTILITY.exec(element.className)
      expect(match?.[0] ?? null, `${element.tagName} 不得带写死的颜色`).toBeNull()
    }

    // 「更多」菜单的抬升面用的是同一套材质里的另一层，不是另一份写死的颜色。
    const raised = Array.from(nav.querySelectorAll<HTMLElement>("div")).find((element) =>
      element.className.includes(DOCK_SURFACE_RAISED),
    )
    expect(raised, "展开的「更多」菜单必须用抬升材质 token").toBeDefined()
  })

  it("keeps the “更多” menu working with theme tokens inside", () => {
    renderNav("/downloads")

    fireEvent.click(itemByLabel("更多"))

    const settings = itemByLabel("设置")
    expect(settings.className).toContain("text-foreground")
    expect(settings.className).toContain(DOCK_IDLE_HOVER)
  })
})
