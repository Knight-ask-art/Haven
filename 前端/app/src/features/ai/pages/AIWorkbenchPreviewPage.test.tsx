// @vitest-environment jsdom

import { afterEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react"
import { MemoryRouter, Route, Routes } from "react-router"
import { AIWorkbenchPreviewPage } from "./AIWorkbenchPreviewPage"

afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

function renderPreview() {
  return render(
    <MemoryRouter initialEntries={["/dev/ai-workbench-preview"]}>
      <Routes>
        <Route path="/dev/ai-workbench-preview" element={<AIWorkbenchPreviewPage />} />
        <Route path="/settings/:section" element={<p>已打开真实设置分区</p>} />
      </Routes>
    </MemoryRouter>,
  )
}

function openHistory() {
  fireEvent.click(screen.getByRole("button", { name: "会话历史" }))
  return screen.getByRole("region", { name: "示例会话历史" })
}

function openSessionMenu() {
  fireEvent.click(screen.getByRole("button", { name: "会话操作" }))
  return screen.getByRole("menu")
}

describe("AIWorkbenchPreviewPage", () => {
  it("renders a fixed sample and makes the preview-only boundary explicit", () => {
    renderPreview()

    expect(screen.getByRole("heading", { name: "长篇阅读字号调整" })).toBeTruthy()
    expect(screen.getByText("帮我把长篇阅读的字号调大一点，保留舒适行高。")).toBeTruthy()
    expect(screen.getByText("设计预览：示例内容不会读取本机设置或调用模型。")).toBeTruthy()
    expect(screen.getByRole("region", { name: "提案审阅示例" })).toBeTruthy()
    expect(screen.getByText("正文字号")).toBeTruthy()
    expect(screen.getByText("18 px")).toBeTruthy()
    expect(screen.getByText("20 px")).toBeTruthy()
    expect(screen.getByText("固定示意数值，不来自本机设置；操作只改变预览状态。")).toBeTruthy()
    expect(screen.getByText("交互预览：消息与反馈仅在本页面展示，不调用模型，也不会读取或更改设置。")).toBeTruthy()
  })

  it("places the navigation rail inside the AI workbench layout", () => {
    renderPreview()

    const workbench = screen.getByRole("region", { name: "AI 工作台设计预览" })
    const navigationRail = screen.getByRole("complementary", { name: "AI 工作台导航" })

    expect(navigationRail.parentElement).toBe(workbench.parentElement)
    expect(workbench.parentElement?.firstElementChild).toBe(navigationRail)
  })

  it("leaves the shared Dock to the app shell", () => {
    renderPreview()

    const workbench = screen.getByRole("region", { name: "AI 工作台设计预览" })

    expect(workbench.querySelector('nav[aria-label="全局浮动导航栏"]')).toBeNull()
  })

  it("shows fixed preview records without presenting them as real executions", () => {
    renderPreview()

    fireEvent.click(screen.getByRole("button", { name: "执行记录" }))

    const recordsPage = screen.getByRole("region", { name: "执行记录" })
    expect(screen.getByRole("heading", { name: "执行记录" })).toBeTruthy()
    expect(within(recordsPage).getByRole("heading", { name: "长篇阅读字号调整" })).toBeTruthy()
    expect(within(recordsPage).getByText(
      "设计预览：以下为固定示例，不代表真实工具调用，也不会修改或保存设置。",
    )).toBeTruthy()
    expect(within(recordsPage).getByRole("button", { name: "查看示例记录：调整长篇阅读字号" })).toBeTruthy()
    expect(within(recordsPage).getByRole("button", { name: "查看示例记录：查找适合长文的字体" })).toBeTruthy()
    expect(within(recordsPage).queryByRole("heading", { name: "暂无执行记录" })).toBeNull()
    expect(screen.queryByRole("textbox", { name: "描述你想调整的设置" })).toBeNull()
  })

  it("opens the AI workbench settings preview and returns to the same conversation", () => {
    renderPreview()

    fireEvent.click(screen.getByRole("button", { name: "AI 设置" }))

    const settingsPage = screen.getByRole("region", { name: "AI 工作台设置" })
    expect(screen.getByRole("heading", { name: "AI 工作台设置" })).toBeTruthy()
    const sessionMenu = openSessionMenu()
    expect(within(sessionMenu).getByRole("menuitem", { name: "重命名会话" })).toBeTruthy()
    expect(within(sessionMenu).getByRole("menuitem", { name: "删除会话" })).toBeTruthy()
    fireEvent.keyDown(sessionMenu, { key: "Escape" })
    expect(screen.queryByRole("menu")).toBeNull()
    expect(within(settingsPage).getByText("模型服务")).toBeTruthy()
    expect(within(settingsPage).getByText("尚未接入")).toBeTruthy()
    expect((within(settingsPage).getByLabelText("API Key") as HTMLInputElement).disabled).toBe(true)
    expect(within(settingsPage).getByRole("heading", { name: "MCP 服务" })).toBeTruthy()
    expect(within(settingsPage).getByRole("heading", { name: "Skills 管理" })).toBeTruthy()
    expect(settingsPage.querySelector('[aria-labelledby="ai-workbench-preview-mcp-title"]')?.classList)
      .toContain("ai-workbench-preview__settings-card--wide")
    expect(settingsPage.querySelector('[aria-labelledby="ai-workbench-preview-skills-title"]')?.classList)
      .toContain("ai-workbench-preview__settings-card--wide")
    const autoSetupTab = within(settingsPage).getByRole("tab", { name: /自动配置/ })
    const manualSetupTab = within(settingsPage).getByRole("tab", { name: "手动配置" })
    expect(autoSetupTab.getAttribute("aria-selected")).toBe("true")
    expect(within(settingsPage).getByText("选择一个服务配置")).toBeTruthy()
    expect(within(settingsPage).getByRole("searchbox", { name: "搜索 Skills" })).toBeTruthy()
    fireEvent.keyDown(autoSetupTab, { key: "ArrowRight" })
    expect(manualSetupTab.getAttribute("aria-selected")).toBe("true")
    expect(document.activeElement).toBe(manualSetupTab)
    fireEvent.click(manualSetupTab)
    expect(within(settingsPage).getByRole("textbox", { name: "服务名称" })).toBeTruthy()
    expect(within(settingsPage).getByRole("combobox", { name: "连接方式" })).toBeTruthy()
    expect(within(settingsPage).getByRole("combobox", { name: "配置范围" })).toBeTruthy()
    expect(within(settingsPage).getByRole("textbox", { name: "连接信息" })).toBeTruthy()
    expect(within(settingsPage).getByText(
      "设计预览：以下选项只影响当前页面，不会连接模型、读取密钥或保存配置。",
    )).toBeTruthy()
    expect(within(settingsPage).queryByRole("button", { name: /保存|连接/ })).toBeNull()
    expect(screen.queryByRole("textbox", { name: "描述你想调整的设置" })).toBeNull()

    fireEvent.click(screen.getByRole("button", { name: "返回对话" }))

    expect(screen.getByRole("log", { name: "对话内容" })).toBeTruthy()
    expect(screen.getByRole("heading", { name: "长篇阅读字号调整" })).toBeTruthy()
  })

  it("manages MCP and Skills in full-width preview forms without connecting tools", () => {
    renderPreview()
    fireEvent.click(screen.getByRole("button", { name: "AI 设置" }))

    const mcpDirectory = screen.getByLabelText("推荐配置示例")
    fireEvent.click(within(mcpDirectory).getAllByRole("button", { name: "选择配置" })[0])
    expect(screen.getByRole("heading", { name: "GitHub" })).toBeTruthy()
    expect(screen.getByText("OAuth 授权")).toBeTruthy()
    fireEvent.click(screen.getByRole("radio", { name: "当前工作区" }))
    expect(screen.getByRole("radio", { name: "当前工作区" }).getAttribute("aria-checked"))
      .toBe("true")
    fireEvent.click(screen.getByRole("button", { name: "添加到预览配置" }))

    const automaticRow = screen.getAllByRole("listitem")[0]
    expect(within(automaticRow).getByText("GitHub")).toBeTruthy()
    expect(within(automaticRow).getByText("当前工作区")).toBeTruthy()
    expect(within(automaticRow).getByText("仅预览 · 待授权")).toBeTruthy()
    fireEvent.click(within(automaticRow).getByRole("button", { name: "移除 GitHub" }))

    fireEvent.click(screen.getByRole("tab", { name: "手动配置" }))
    fireEvent.change(screen.getByRole("textbox", { name: "服务名称" }), {
      target: { value: "资料检索服务" },
    })
    fireEvent.change(screen.getByRole("combobox", { name: "连接方式" }), {
      target: { value: "remote" },
    })
    fireEvent.change(screen.getByRole("textbox", { name: "连接信息" }), {
      target: { value: "https://example.invalid/mcp" },
    })
    expect(screen.getByText("服务地址")).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: "添加到预览配置" }))

    expect(screen.getByText("资料检索服务")).toBeTruthy()
    expect(within(screen.getAllByRole("listitem")[0]).getByText("远程服务")).toBeTruthy()
    expect(screen.getByText("仅预览 · 未连接")).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: "移除 资料检索服务" }))
    expect(screen.queryByText("资料检索服务")).toBeNull()

    fireEvent.change(screen.getByRole("searchbox", { name: "搜索 Skills" }), {
      target: { value: "长文摘要" },
    })
    expect(screen.getByText("长文摘要（示例）")).toBeTruthy()
    expect(screen.queryByText("阅读建议（示例）")).toBeNull()
    fireEvent.click(screen.getByRole("radio", { name: "已启用" }))
    expect(screen.queryByText("长文摘要（示例）")).toBeNull()

    fireEvent.change(screen.getByRole("searchbox", { name: "搜索 Skills" }), {
      target: { value: "" },
    })
    fireEvent.click(screen.getByRole("radio", { name: "全部" }))
    fireEvent.click(screen.getByRole("button", { name: "添加示例 Skill" }))
    fireEvent.change(screen.getByRole("textbox", { name: "技能名称" }), {
      target: { value: "自定义摘要" },
    })
    fireEvent.change(screen.getByRole("textbox", { name: "技能说明" }), {
      target: { value: "演示如何管理自定义技能" },
    })
    fireEvent.click(screen.getByRole("button", { name: "添加到示例列表" }))

    const customSkillSwitch = screen.getByRole("switch", { name: "启用 自定义摘要" })
    expect(customSkillSwitch.getAttribute("aria-checked")).toBe("false")
    fireEvent.click(customSkillSwitch)
    expect(screen.getByRole("switch", { name: "启用 自定义摘要" }).getAttribute("aria-checked"))
      .toBe("true")
    fireEvent.click(screen.getByRole("button", { name: "移除 自定义摘要" }))
    expect(screen.queryByText("自定义摘要")).toBeNull()
    expect(screen.getByText("当前只演示搜索、筛选、添加与启停；示例技能不会执行，也不会保存。"))
      .toBeTruthy()
  })

  it("previews the send shortcut and context visibility in the conversation", () => {
    renderPreview()

    fireEvent.click(screen.getByRole("button", { name: "AI 设置" }))
    fireEvent.click(screen.getByRole("switch", { name: "启用 阅读建议（示例）" }))
    expect(screen.getByRole("switch", { name: "启用 阅读建议（示例）" }).getAttribute("aria-checked"))
      .toBe("false")
    fireEvent.click(screen.getByRole("radio", { name: "Ctrl 或 Command + Enter 发送" }))
    fireEvent.click(screen.getByRole("switch", { name: "显示上下文用量" }))
    expect(screen.getByRole("switch", { name: "显示上下文用量" }).getAttribute("aria-checked"))
      .toBe("false")
    fireEvent.click(screen.getByRole("button", { name: "返回对话" }))

    expect(screen.queryByText("当前上下文：")).toBeNull()
    const composer = screen.getByRole("textbox", { name: "描述你想调整的设置" })
    fireEvent.change(composer, { target: { value: "预览快捷键交互" } })
    fireEvent.keyDown(composer, { key: "Enter" })
    expect(screen.getByRole("log", { name: "对话内容" }).textContent).not.toContain("预览快捷键交互")

    fireEvent.keyDown(composer, { key: "Enter", ctrlKey: true })
    expect(screen.getByRole("log", { name: "对话内容" }).textContent).toContain("预览快捷键交互")

    fireEvent.click(screen.getByRole("button", { name: "AI 设置" }))
    fireEvent.click(screen.getByRole("switch", { name: "显示上下文用量" }))
    fireEvent.click(screen.getByRole("button", { name: "返回对话" }))
    expect(screen.getByText("当前上下文：")).toBeTruthy()
  })

  it("opens a successful record, returns to the list, and preserves the conversation", () => {
    renderPreview()

    fireEvent.click(screen.getByRole("button", { name: "执行记录" }))
    fireEvent.click(screen.getByRole("button", { name: "查看示例记录：调整长篇阅读字号" }))

    const detail = screen.getByRole("region", { name: "执行记录详情" })
    expect(within(detail).getByRole("heading", { name: "调整长篇阅读字号" })).toBeTruthy()
    expect(detail.querySelector(".ai-workbench-preview__execution-status")?.getAttribute("data-state"))
      .toBe("completed")
    expect(within(detail).getByText("示例时间 · 10:42")).toBeTruthy()
    expect(within(detail).getByText("示意耗时 2.4 秒")).toBeTruthy()
    expect(within(detail).getByText("读取阅读偏好")).toBeTruthy()
    expect(within(detail).getByText("整理调整建议")).toBeTruthy()
    expect(within(detail).getByText(
      "建议正文使用 20 px 字号、1.8 倍行距；当前没有应用任何改动。",
    )).toBeTruthy()
    expect(within(detail).getByText(/固定预览样例/)).toBeTruthy()

    fireEvent.click(within(detail).getByRole("button", { name: "返回执行记录列表" }))
    expect(screen.getByRole("button", { name: "查看示例记录：调整长篇阅读字号" })).toBeTruthy()

    fireEvent.click(screen.getByRole("button", { name: "返回对话" }))

    expect(screen.getByRole("log", { name: "对话内容" })).toBeTruthy()
    expect(screen.getByRole("heading", { name: "长篇阅读字号调整" })).toBeTruthy()
    expect(screen.getByRole("textbox", { name: "描述你想调整的设置" })).toBeTruthy()
  })

  it("shows a failed example with an explicit error and no settings changes", () => {
    renderPreview()

    fireEvent.click(screen.getByRole("button", { name: "执行记录" }))
    fireEvent.click(screen.getByRole("button", { name: "查看示例记录：查找适合长文的字体" }))

    const detail = screen.getByRole("region", { name: "执行记录详情" })
    expect(within(detail).getByRole("heading", { name: "查找适合长文的字体" })).toBeTruthy()
    expect(within(detail).getByRole("heading", { name: "执行未完成" })).toBeTruthy()
    expect(within(detail).getByText("示例时间 · 10:38")).toBeTruthy()
    expect(within(detail).getByText("示意耗时 0.8 秒")).toBeTruthy()
    expect(within(detail).getByText(
      "示例错误：字体建议无法生成；没有更改任何阅读设置。",
    )).toBeTruthy()
    expect(within(detail).getByRole("button", { name: "返回执行记录列表" })).toBeTruthy()

    fireEvent.click(within(detail).getByRole("button", { name: "回到对话" }))
    expect(screen.getByRole("log", { name: "对话内容" })).toBeTruthy()
    expect(screen.getByRole("heading", { name: "长篇阅读字号调整" })).toBeTruthy()
  })

  it("keeps the AI workbench navigation visible without a collapse control", () => {
    renderPreview()

    expect(screen.getByRole("complementary", { name: "AI 工作台导航" })).toBeTruthy()
    expect(screen.queryByRole("button", { name: /收起工作台导航|展开工作台导航|关闭工作台导航/ })).toBeNull()
    expect(document.querySelector(".ai-workbench-preview--nav-collapsed")).toBeNull()
    expect(document.querySelector(".ai-workbench-preview__nav-backdrop")).toBeNull()
  })

  it("opens a keyboard-accessible session action menu", () => {
    renderPreview()

    const sessionMenuButton = screen.getByRole("button", { name: "会话操作" })
    const menu = openSessionMenu()
    const renameItem = within(menu).getByRole("menuitem", { name: "重命名会话" })
    const deleteItem = within(menu).getByRole("menuitem", { name: "删除会话" })

    expect(sessionMenuButton.getAttribute("aria-haspopup")).toBe("menu")
    expect(sessionMenuButton.getAttribute("aria-expanded")).toBe("true")
    expect(document.activeElement).toBe(renameItem)

    fireEvent.keyDown(menu, { key: "ArrowDown" })
    expect(document.activeElement).toBe(deleteItem)
    fireEvent.keyDown(menu, { key: "Escape" })

    expect(screen.queryByRole("menu")).toBeNull()
    expect(document.activeElement).toBe(sessionMenuButton)
  })

  it("renames the active preview conversation and reflects the title in history", () => {
    renderPreview()

    const menu = openSessionMenu()
    fireEvent.click(within(menu).getByRole("menuitem", { name: "重命名会话" }))

    const dialog = screen.getByRole("dialog", { name: "重命名会话" })
    const nameInput = within(dialog).getByRole("textbox", { name: "会话名称" }) as HTMLInputElement
    expect(nameInput.value).toBe("长篇阅读字号调整")
    expect(document.activeElement).toBe(nameInput)

    fireEvent.change(nameInput, { target: { value: "  我的阅读助手  " } })
    fireEvent.click(within(dialog).getByRole("button", { name: "保存名称" }))

    expect(screen.getByRole("heading", { name: "我的阅读助手" })).toBeTruthy()
    expect(screen.getByRole("status").textContent).toContain("更改仅保留在当前页面")
    expect(within(openHistory()).getByRole("button", { name: /我的阅读助手/ })).toBeTruthy()
  })

  it("cancels a rename without changing the conversation title", () => {
    renderPreview()

    fireEvent.click(within(openSessionMenu()).getByRole("menuitem", { name: "重命名会话" }))
    const dialog = screen.getByRole("dialog", { name: "重命名会话" })
    fireEvent.change(within(dialog).getByRole("textbox", { name: "会话名称" }), {
      target: { value: "不应保存的标题" },
    })
    fireEvent.keyDown(dialog, { key: "Escape" })

    expect(screen.getByRole("heading", { name: "长篇阅读字号调整" })).toBeTruthy()
    expect(screen.queryByRole("dialog")).toBeNull()
  })

  it("confirms session deletion, keeps it on cancel, and falls back to a blank conversation", () => {
    renderPreview()

    fireEvent.click(within(openSessionMenu()).getByRole("menuitem", { name: "删除会话" }))
    let confirmation = screen.getByRole("alertdialog", { name: "删除此会话？" })
    expect(within(confirmation).getByText(/长篇阅读字号调整/)).toBeTruthy()
    fireEvent.click(within(confirmation).getByRole("button", { name: "取消删除" }))
    expect(screen.getByRole("heading", { name: "长篇阅读字号调整" })).toBeTruthy()

    const deleteCurrentConversation = () => {
      fireEvent.click(within(openSessionMenu()).getByRole("menuitem", { name: "删除会话" }))
      confirmation = screen.getByRole("alertdialog", { name: "删除此会话？" })
      fireEvent.click(within(confirmation).getByRole("button", { name: "删除会话" }))
    }

    deleteCurrentConversation()
    expect(screen.getByRole("heading", { name: "阅读字体建议" })).toBeTruthy()
    expect(within(openHistory()).queryByRole("button", { name: /长篇阅读字号调整/ })).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "会话历史" }))

    deleteCurrentConversation()
    expect(screen.getByRole("heading", { name: "失败反馈示例" })).toBeTruthy()
    deleteCurrentConversation()

    expect(screen.getByRole("heading", { name: "新对话" })).toBeTruthy()
    expect(screen.getByRole("heading", { name: "从一个想法开始" })).toBeTruthy()
    expect(screen.getByRole("status").textContent).toContain("没有删除或保存任何真实对话")
  })

  it("filters registered settings navigation and keeps AI itself as a preview-only item", () => {
    renderPreview()

    const search = screen.getByRole("textbox", { name: "搜索设置" })
    fireEvent.change(search, { target: { value: "阅读" } })

    expect(screen.getByRole("link", { name: /^阅读/ })).toBeTruthy()
    expect(screen.queryByRole("link", { name: /播放/ })).toBeNull()
    expect(screen.queryByText("资源与存储")).toBeNull()
    expect(screen.getByRole("link", { name: /AI 工作台/ })).toBeTruthy()
  })

  it("opens a true empty state for a new conversation and fills prompts from suggestions", () => {
    renderPreview()

    fireEvent.click(screen.getByRole("button", { name: "新建对话" }))

    expect(screen.getByRole("heading", { name: "新对话" })).toBeTruthy()
    expect(screen.getByRole("heading", { name: "从一个想法开始" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "发送预览消息" }).hasAttribute("disabled")).toBe(true)

    fireEvent.click(screen.getByRole("button", { name: "调整阅读字号" }))
    expect((screen.getByRole("textbox", { name: "描述你想调整的设置" }) as HTMLInputElement).value)
      .toBe("帮我调整长篇阅读的字号")
    expect(screen.getByRole("button", { name: "发送预览消息" }).hasAttribute("disabled")).toBe(false)
  })

  it("keeps sent text local and shows a clearly labeled loading and preview-feedback state", async () => {
    vi.useFakeTimers()
    renderPreview()

    fireEvent.click(screen.getByRole("button", { name: "新建对话" }))
    fireEvent.change(screen.getByRole("textbox", { name: "描述你想调整的设置" }), {
      target: { value: "帮我调整长篇阅读的字号" },
    })
    fireEvent.click(screen.getByRole("button", { name: "发送预览消息" }))

    expect(screen.getByRole("log").textContent).toContain("帮我调整长篇阅读的字号")
    expect(screen.getByText("正在准备示例反馈…")).toBeTruthy()
    expect(screen.getByText("正在演示发送状态；消息只显示在本页，没有发送给模型。")).toBeTruthy()

    await act(async () => {
      await vi.advanceTimersByTimeAsync(650)
    })

    expect(screen.getByText(/这条消息已在当前页面本地展示/)).toBeTruthy()
    expect(screen.getByText("预览反馈 · 未调用模型。")).toBeTruthy()
    expect(screen.getByText("本地交互演示完成：没有调用模型，设置也没有更改。")).toBeTruthy()
  })

  it("switches among local sample conversations from the history panel", () => {
    renderPreview()

    const history = openHistory()
    fireEvent.click(within(history).getByRole("button", { name: /阅读字体建议/ }))

    expect(screen.getByRole("heading", { name: "阅读字体建议" })).toBeTruthy()
    expect(screen.getByText("有没有适合长篇阅读的字体？")).toBeTruthy()
    expect(screen.getByText("示例回复 · 未调用模型，也未读取设置。")).toBeTruthy()
    expect(screen.queryByRole("region", { name: "示例会话历史" })).toBeNull()
  })

  it("exposes a failure example and simulates retry feedback without a model request", async () => {
    vi.useFakeTimers()
    renderPreview()

    const history = openHistory()
    fireEvent.click(within(history).getByRole("button", { name: /失败反馈示例/ }))

    expect(screen.getByRole("heading", { name: "失败反馈示例" })).toBeTruthy()
    expect(screen.getByRole("alert").textContent).toContain("这条内容没有离开本页面")
    fireEvent.click(screen.getByRole("button", { name: "重试示例" }))
    expect(screen.getByText("正在演示重试…")).toBeTruthy()
    expect(screen.getByText("正在演示本地重试状态；没有向模型服务发送请求。")).toBeTruthy()

    await act(async () => {
      await vi.advanceTimersByTimeAsync(650)
    })

    expect(screen.getByText(/本地重试状态演示已结束/)).toBeTruthy()
    expect(screen.getByText("重试预览完成 · 未调用模型。")).toBeTruthy()
    expect(screen.getByText("重试状态预览完成：没有调用模型，设置也没有更改。")).toBeTruthy()
    expect(screen.queryByRole("button", { name: "重试示例" })).toBeNull()
  })

  it("simulates proposal confirmation without changing reading settings", () => {
    renderPreview()

    fireEvent.click(screen.getByRole("button", { name: "确认应用" }))

    expect(screen.getByText("已确认 · 预览")).toBeTruthy()
    expect(screen.getByRole("status").textContent).toContain("本机阅读设置没有更改")
    expect(screen.queryByRole("button", { name: "确认应用" })).toBeNull()
  })

  it("simulates proposal rejection without changing reading settings", () => {
    renderPreview()

    fireEvent.click(screen.getByRole("button", { name: "拒绝建议" }))

    expect(screen.getByText("已拒绝 · 预览")).toBeTruthy()
    expect(screen.getByRole("status").textContent).toContain("本机阅读设置没有更改")
    expect(screen.queryByRole("button", { name: "拒绝建议" })).toBeNull()
  })

  it("navigates from the preview rail to a real settings section", () => {
    renderPreview()

    fireEvent.click(screen.getByRole("link", { name: /外观/ }))
    expect(screen.getByText("已打开真实设置分区")).toBeTruthy()
  })
})
