import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react"
import type { FormEvent, KeyboardEvent as ReactKeyboardEvent } from "react"
import { Link } from "react-router"
import { BootstrapIcon, type BootstrapIconName } from "@/components/ui/haven/BootstrapIcon"
import {
  SETTINGS_NAV_GROUPS,
  SETTINGS_OVERVIEW,
  SETTINGS_REGISTRY,
} from "@/features/settings/lib/settings-registry"
import { filterSettingsNavigationGroups } from "@/features/settings/lib/settings-navigation"
import "./AIWorkbenchPreviewPage.css"

interface PreviewNavigationItem {
  id: string
  label: string
  description: string
  icon: BootstrapIconName
  to: string
}

type PreviewMessageKind = "example" | "error"
type WorkbenchView = "conversation" | "execution-records" | "settings"
type PreviewSendShortcut = "enter" | "ctrl-enter"

interface PreviewMessage {
  id: string
  role: "user" | "assistant"
  content: string
  timeLabel: string
  kind?: PreviewMessageKind
  footnote?: string
  showToolPreview?: boolean
}

interface PreviewConversation {
  id: string
  title: string
  summary: string
  messages: PreviewMessage[]
  isResponding: boolean
  retryingMessageId: string | null
  proposalDecision: "accepted" | "rejected" | null
  showProposal: boolean
}

type PreviewExecutionStatus = "completed" | "failed"
type PreviewMcpTransport = "stdio" | "remote"
type PreviewMcpSetupMode = "automatic" | "manual"
type PreviewMcpScope = "user" | "workspace"
type PreviewSkillFilter = "all" | "enabled" | "disabled"

interface PreviewExecutionStep {
  name: string
  status: PreviewExecutionStatus
  durationLabel: string
  description: string
}

interface PreviewExecutionRecord {
  id: string
  title: string
  status: PreviewExecutionStatus
  timeLabel: string
  durationLabel: string
  summary: string
  steps: PreviewExecutionStep[]
  resultSummary?: string
  errorMessage?: string
}

interface PreviewMcpService {
  id: number
  name: string
  transport: PreviewMcpTransport
  setupMode: PreviewMcpSetupMode
  scope: PreviewMcpScope
}

interface PreviewMcpCatalogEntry {
  id: string
  name: string
  description: string
  transport: PreviewMcpTransport
  authorization: string
  permissionSummary: string
}

interface PreviewSkill {
  id: string
  name: string
  description: string
  enabled: boolean
}

const PREVIEW_MCP_CATALOG: PreviewMcpCatalogEntry[] = [
  {
    id: "github",
    name: "GitHub",
    description: "连接代码仓库与协作内容",
    transport: "remote",
    authorization: "OAuth 授权",
    permissionSummary: "授权前查看将访问的账号、组织与仓库范围；可随时在服务方撤销授权。",
  },
  {
    id: "playwright",
    name: "Playwright",
    description: "让 AI 使用浏览器自动化工具",
    transport: "stdio",
    authorization: "本机运行 · 需信任确认",
    permissionSummary: "启动本地服务前确认来源、运行命令与浏览器操作权限。",
  },
]

const AI_PREVIEW_NAVIGATION_ITEM: PreviewNavigationItem = {
  id: "ai-workbench-preview",
  label: "AI 工作台",
  description: "交互预览 · 未接入模型",
  icon: "stars",
  to: "/dev/ai-workbench-preview",
}

const PREVIEW_NAVIGATION_GROUPS = SETTINGS_NAV_GROUPS.map((group) => {
  const entries = group.sectionIds
    .map((sectionId) => SETTINGS_REGISTRY.find((entry) => entry.id === sectionId))
    .filter((entry) => entry !== undefined)
    .map((entry) => ({
      id: entry.id,
      label: entry.label,
      description: entry.description,
      icon: entry.icon,
      to: "/settings/" + entry.id,
    }))

  const items = group.id === "configure"
    ? [{ ...SETTINGS_OVERVIEW, to: "/settings/overview" }, ...entries]
    : entries

  return {
    label: group.id === "system" ? "系统" : group.label,
    items,
  }
}).flatMap((group) => group.label === "配置"
  ? [group, { label: "智能", items: [AI_PREVIEW_NAVIGATION_ITEM] }]
  : [group])

const PREVIEW_EXECUTION_RECORDS: PreviewExecutionRecord[] = [
  {
    id: "sample-reading-adjustment",
    title: "调整长篇阅读字号",
    status: "completed",
    timeLabel: "示例时间 · 10:42",
    durationLabel: "示意耗时 2.4 秒",
    summary: "已整理一份等待确认的阅读建议。",
    steps: [
      {
        name: "读取阅读偏好",
        status: "completed",
        durationLabel: "示意 0.3 秒",
        description: "读取了示例中的字号与行距偏好。",
      },
      {
        name: "整理调整建议",
        status: "completed",
        durationLabel: "示意 2.1 秒",
        description: "整理出一份可供你确认的建议，没有应用设置。",
      },
    ],
    resultSummary: "建议正文使用 20 px 字号、1.8 倍行距；当前没有应用任何改动。",
  },
  {
    id: "sample-font-suggestion",
    title: "查找适合长文的字体",
    status: "failed",
    timeLabel: "示例时间 · 10:38",
    durationLabel: "示意耗时 0.8 秒",
    summary: "字体建议示例未能完成，阅读设置保持不变。",
    steps: [
      {
        name: "读取字体清单",
        status: "completed",
        durationLabel: "示意 0.3 秒",
        description: "读取了预先准备的字体示例。",
      },
      {
        name: "生成字体建议",
        status: "failed",
        durationLabel: "示意 0.5 秒",
        description: "此处展示失败步骤的状态样式。",
      },
    ],
    errorMessage: "示例错误：字体建议无法生成；没有更改任何阅读设置。",
  },
]

function createInitialPreviewConversations(): PreviewConversation[] {
  return [
    {
      id: "sample-reading",
      title: "长篇阅读字号调整",
      summary: "字号与行间距建议 · 固定示例",
      isResponding: false,
      retryingMessageId: null,
      proposalDecision: null,
      showProposal: true,
      messages: [
        {
          id: "sample-reading-user",
          role: "user",
          content: "帮我把长篇阅读的字号调大一点，保留舒适行高。",
          timeLabel: "示例对话",
        },
        {
          id: "sample-reading-assistant",
          role: "assistant",
          content: "下面是为界面展示准备的固定提案示例。真实接入后，建议会基于你授权的设置生成，并由你确认后再应用。",
          timeLabel: "示例对话",
          kind: "example",
          footnote: "设计预览：示例内容不会读取本机设置或调用模型。",
          showToolPreview: true,
        },
      ],
    },
    {
      id: "sample-fonts",
      title: "阅读字体建议",
      summary: "固定回复示例 · 未调用模型",
      isResponding: false,
      retryingMessageId: null,
      proposalDecision: null,
      showProposal: false,
      messages: [
        {
          id: "sample-fonts-user",
          role: "user",
          content: "有没有适合长篇阅读的字体？",
          timeLabel: "示例对话",
        },
        {
          id: "sample-fonts-assistant",
          role: "assistant",
          content: "这里展示的是预先写好的界面样例。模型服务接入后，回复才会根据你提供的内容生成。",
          timeLabel: "示例对话",
          kind: "example",
          footnote: "示例回复 · 未调用模型，也未读取设置。",
        },
      ],
    },
    {
      id: "sample-error",
      title: "失败反馈示例",
      summary: "错误提示与重试状态 · 固定示例",
      isResponding: false,
      retryingMessageId: null,
      proposalDecision: null,
      showProposal: false,
      messages: [
        {
          id: "sample-error-user",
          role: "user",
          content: "帮我整理一份阅读设置建议。",
          timeLabel: "示例对话",
        },
        {
          id: "sample-error-assistant",
          role: "assistant",
          content: "这是发送失败状态的界面示例。当前预览没有连接模型服务，这条内容没有离开本页面。",
          timeLabel: "失败状态示例",
          kind: "error",
          footnote: "失败状态预览 · 没有发起真实请求。",
        },
      ],
    },
  ]
}

function createBlankPreviewConversation(id: string): PreviewConversation {
  return {
    id,
    title: "新对话",
    summary: "空白对话 · 仅本页预览",
    messages: [],
    isResponding: false,
    retryingMessageId: null,
    proposalDecision: null,
    showProposal: false,
  }
}

function handleSessionDialogKeyDown(
  event: ReactKeyboardEvent<HTMLDivElement>,
  onDismiss: () => void,
) {
  if (event.key === "Escape") {
    event.preventDefault()
    onDismiss()
    return
  }

  if (event.key !== "Tab") return

  const focusableElements = Array.from(
    event.currentTarget.querySelectorAll<HTMLElement>("button:not([disabled]), input:not([disabled])"),
  )
  const firstElement = focusableElements[0]
  const lastElement = focusableElements[focusableElements.length - 1]
  if (!firstElement || !lastElement) return

  if (event.shiftKey && document.activeElement === firstElement) {
    event.preventDefault()
    lastElement.focus()
  } else if (!event.shiftKey && document.activeElement === lastElement) {
    event.preventDefault()
    firstElement.focus()
  }
}

export function AIWorkbenchPreviewPage() {
  const [searchQuery, setSearchQuery] = useState("")
  const [prompt, setPrompt] = useState("")
  const [notice, setNotice] = useState("")
  const [historyOpen, setHistoryOpen] = useState(false)
  const [sessionMenuOpen, setSessionMenuOpen] = useState(false)
  const [renameDialogOpen, setRenameDialogOpen] = useState(false)
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false)
  const [renameDraft, setRenameDraft] = useState("")
  const [activeView, setActiveView] = useState<WorkbenchView>("conversation")
  const [sendShortcut, setSendShortcut] = useState<PreviewSendShortcut>("enter")
  const [showContextUsage, setShowContextUsage] = useState(true)
  const [conversations, setConversations] = useState(createInitialPreviewConversations)
  const [activeConversationId, setActiveConversationId] = useState("sample-reading")
  const sessionMenuRef = useRef<HTMLDivElement>(null)
  const sessionMenuTriggerRef = useRef<HTMLButtonElement>(null)
  const firstSessionMenuItemRef = useRef<HTMLButtonElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)
  const threadEndRef = useRef<HTMLDivElement>(null)
  const responseTimersRef = useRef(new Map<string, number>())
  const messageSequenceRef = useRef(0)
  const draftSequenceRef = useRef(0)

  const activeConversation = conversations.find(({ id }) => id === activeConversationId) ?? conversations[0]!

  useEffect(() => {
    const handleShortcut = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLocaleLowerCase() === "k") {
        event.preventDefault()
        searchRef.current?.focus()
        searchRef.current?.select()
      }
      if (event.key === "Escape" && document.activeElement === searchRef.current) {
        setSearchQuery("")
        searchRef.current?.blur()
      }
    }

    document.addEventListener("keydown", handleShortcut)
    return () => document.removeEventListener("keydown", handleShortcut)
  }, [])

  useEffect(() => {
    if (!historyOpen) return

    const closeHistory = (event: KeyboardEvent) => {
      if (event.key === "Escape") setHistoryOpen(false)
    }
    document.addEventListener("keydown", closeHistory)
    return () => document.removeEventListener("keydown", closeHistory)
  }, [historyOpen])

  useEffect(() => {
    if (!sessionMenuOpen) return

    firstSessionMenuItemRef.current?.focus()
    const closeMenu = (event: PointerEvent) => {
      if (sessionMenuRef.current?.contains(event.target as Node)) return
      setSessionMenuOpen(false)
    }
    const handleMenuShortcut = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return
      event.preventDefault()
      setSessionMenuOpen(false)
      sessionMenuTriggerRef.current?.focus()
    }

    document.addEventListener("pointerdown", closeMenu)
    document.addEventListener("keydown", handleMenuShortcut)
    return () => {
      document.removeEventListener("pointerdown", closeMenu)
      document.removeEventListener("keydown", handleMenuShortcut)
    }
  }, [sessionMenuOpen])

  useEffect(() => () => {
    responseTimersRef.current.forEach((timer) => window.clearTimeout(timer))
    responseTimersRef.current.clear()
  }, [])

  useEffect(() => {
    threadEndRef.current?.scrollIntoView?.({ block: "end", behavior: "auto" })
  }, [activeConversationId, activeConversation.messages.length, activeConversation.isResponding])

  const filteredNavigation = useMemo(() => {
    const searchableGroups = PREVIEW_NAVIGATION_GROUPS.filter((group) => group.label !== "智能")
    const filteredGroups = filterSettingsNavigationGroups(searchableGroups, searchQuery)
    const aiGroup = PREVIEW_NAVIGATION_GROUPS.find((group) => group.label === "智能")
    if (!aiGroup) return filteredGroups

    const configureIndex = filteredGroups.findIndex((group) => group.label === "配置")
    const insertAt = configureIndex >= 0 ? configureIndex + 1 : Math.min(1, filteredGroups.length)
    return [
      ...filteredGroups.slice(0, insertAt),
      aiGroup,
      ...filteredGroups.slice(insertAt),
    ]
  }, [searchQuery])

  const updateConversation = (
    conversationId: string,
    update: (conversation: PreviewConversation) => PreviewConversation,
  ) => {
    setConversations((current) => current.map((conversation) =>
      conversation.id === conversationId ? update(conversation) : conversation,
    ))
  }

  const startNewConversation = () => {
    if (activeConversation.title === "新对话" && activeConversation.messages.length === 0) {
      setPrompt("")
      setHistoryOpen(false)
      setNotice("已经是一个新的空白对话。输入内容只会在本页预览。")
      return
    }

    const draftId = "draft-" + draftSequenceRef.current
    draftSequenceRef.current += 1
    setConversations((current) => [createBlankPreviewConversation(draftId), ...current])
    setActiveConversationId(draftId)
    setPrompt("")
    setHistoryOpen(false)
    setNotice("已新建空白预览对话；内容不会发送给模型或保存。")
  }

  const selectConversation = (conversation: PreviewConversation) => {
    setActiveConversationId(conversation.id)
    setPrompt("")
    setHistoryOpen(false)
    setNotice("已切换到示例对话「" + conversation.title + "」。")
  }

  const openRenameDialog = () => {
    setRenameDraft(activeConversation.title)
    setSessionMenuOpen(false)
    setRenameDialogOpen(true)
  }

  const saveConversationTitle = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const title = renameDraft.trim()
    if (!title) return

    updateConversation(activeConversation.id, (conversation) => ({ ...conversation, title }))
    setRenameDialogOpen(false)
    setNotice("已在预览中重命名为「" + title + "」；更改仅保留在当前页面。")
    sessionMenuTriggerRef.current?.focus()
  }

  const deleteActiveConversation = () => {
    const deletedConversation = activeConversation
    const responseTimer = responseTimersRef.current.get(deletedConversation.id)
    if (responseTimer !== undefined) {
      window.clearTimeout(responseTimer)
      responseTimersRef.current.delete(deletedConversation.id)
    }

    const remainingConversations = conversations.filter(
      (conversation) => conversation.id !== deletedConversation.id,
    )
    if (remainingConversations.length === 0) {
      const fallbackId = "draft-" + draftSequenceRef.current
      draftSequenceRef.current += 1
      remainingConversations.push(createBlankPreviewConversation(fallbackId))
    }

    setConversations(remainingConversations)
    setActiveConversationId(remainingConversations[0]!.id)
    setActiveView("conversation")
    setPrompt("")
    setHistoryOpen(false)
    setDeleteDialogOpen(false)
    setNotice("已从当前预览中删除会话「" + deletedConversation.title + "」；没有删除或保存任何真实对话。")
    sessionMenuTriggerRef.current?.focus()
  }

  const showFailureExample = () => {
    const failedConversation = conversations.find(({ id }) => id === "sample-error")
    if (failedConversation) selectConversation(failedConversation)
  }

  const handleSendMessage = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const content = prompt.trim()
    if (!content || activeConversation.isResponding) return

    const conversationId = activeConversation.id
    const timer = responseTimersRef.current.get(conversationId)
    if (timer !== undefined) return

    const messageId = "preview-message-" + messageSequenceRef.current
    messageSequenceRef.current += 1
    const nextTitle = content.length > 28 ? content.slice(0, 28) + "…" : content
    updateConversation(conversationId, (conversation) => ({
      ...conversation,
      title: conversation.title === "新对话" ? nextTitle : conversation.title,
      summary: conversation.title === "新对话" ? "本地消息 · 未保存" : conversation.summary,
      isResponding: true,
      retryingMessageId: null,
      messages: [
        ...conversation.messages,
        { id: messageId, role: "user", content, timeLabel: "本地预览" },
      ],
    }))
    setPrompt("")
    setNotice("正在演示发送状态；消息只显示在本页，没有发送给模型。")

    const responseTimer = window.setTimeout(() => {
      responseTimersRef.current.delete(conversationId)
      updateConversation(conversationId, (conversation) => ({
        ...conversation,
        isResponding: false,
        messages: [
          ...conversation.messages,
          {
            id: "preview-message-" + messageSequenceRef.current,
            role: "assistant",
            content: "这条消息已在当前页面本地展示。模型服务尚未接入，因此不会生成真实回复，也没有读取或更改设置。",
            timeLabel: "本地预览",
            kind: "example",
            footnote: "预览反馈 · 未调用模型。",
          },
        ],
      }))
      messageSequenceRef.current += 1
      setNotice("本地交互演示完成：没有调用模型，设置也没有更改。")
    }, 650)
    responseTimersRef.current.set(conversationId, responseTimer)
  }

  const handleComposerKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (event.key !== "Enter" || event.nativeEvent.isComposing) return

    const hasCommandModifier = event.ctrlKey || event.metaKey
    if (sendShortcut === "ctrl-enter") {
      event.preventDefault()
      if (hasCommandModifier) event.currentTarget.form?.requestSubmit()
      return
    }

    if (hasCommandModifier) {
      event.preventDefault()
      event.currentTarget.form?.requestSubmit()
    }
  }

  const retryExample = (conversationId: string, messageId: string) => {
    const timer = responseTimersRef.current.get(conversationId)
    if (timer !== undefined) return

    updateConversation(conversationId, (conversation) => ({
      ...conversation,
      isResponding: true,
      retryingMessageId: messageId,
    }))
    setNotice("正在演示本地重试状态；没有向模型服务发送请求。")

    const responseTimer = window.setTimeout(() => {
      responseTimersRef.current.delete(conversationId)
      updateConversation(conversationId, (conversation) => ({
        ...conversation,
        isResponding: false,
        retryingMessageId: null,
        messages: conversation.messages.map((message) => message.id === messageId
          ? {
              ...message,
              content: "本地重试状态演示已结束；没有发送请求，也没有读取或更改设置。",
              kind: "example",
              footnote: "重试预览完成 · 未调用模型。",
            }
          : message),
      }))
      setNotice("重试状态预览完成：没有调用模型，设置也没有更改。")
    }, 650)
    responseTimersRef.current.set(conversationId, responseTimer)
  }

  const decideProposal = (decision: "accepted" | "rejected") => {
    updateConversation(activeConversation.id, (conversation) => ({
      ...conversation,
      proposalDecision: decision,
    }))
    setNotice(
      decision === "accepted"
        ? "已在预览中模拟确认；本机阅读设置没有更改。"
        : "已在预览中模拟拒绝；本机阅读设置没有更改。",
    )
  }

  return (
    <div className="ai-workbench-preview">
      <aside
        className="ai-workbench-preview__sidebar"
        aria-label="AI 工作台导航"
      >
        <div className="ai-workbench-preview__brand">
          <span className="ai-workbench-preview__logo" aria-hidden="true">
            <img src="/logo.png" alt="" />
          </span>
          <span className="ai-workbench-preview__brand-name">栖阅</span>
        </div>

        <label className="ai-workbench-preview__search">
          <BootstrapIcon name="search" size={16} />
          <input
            ref={searchRef}
            type="text"
            value={searchQuery}
            onChange={(event) => setSearchQuery(event.target.value)}
            placeholder="搜索设置"
            aria-label="搜索设置"
            aria-keyshortcuts="Control+K Meta+K"
          />
          <kbd>⌘ K</kbd>
        </label>

        <nav className="ai-workbench-preview__navigation" aria-label="工作台与设置导航">
          {filteredNavigation.length > 0 ? filteredNavigation.map((group) => (
            <section className="ai-workbench-preview__nav-group" key={group.label}>
              <h2>{group.label}</h2>
              <div className="ai-workbench-preview__nav-items">
                {group.items.map((item) => {
                  const isPreview = item.id === AI_PREVIEW_NAVIGATION_ITEM.id
                  return (
                    <Link
                      key={item.id}
                      to={item.to}
                      aria-current={isPreview ? "page" : undefined}
                      className={"ai-workbench-preview__nav-item" + (isPreview ? " is-active" : "")}
                    >
                      <BootstrapIcon name={item.icon} size={16} />
                      <span className="ai-workbench-preview__nav-copy">
                        <span className="ai-workbench-preview__nav-label">{item.label}</span>
                        <span className="ai-workbench-preview__nav-description">{item.description}</span>
                      </span>
                    </Link>
                  )
                })}
              </div>
            </section>
          )) : (
            <p className="ai-workbench-preview__empty-search">没有找到匹配的设置。</p>
          )}
        </nav>
      </aside>

      <section className="ai-workbench-preview__stage" aria-label="AI 工作台设计预览">
        <div className="ai-workbench-preview__panel">
          <header className="ai-workbench-preview__toolbar">
            <div className="ai-workbench-preview__title-wrap">
              <h1>
                {activeView === "execution-records"
                  ? "执行记录"
                  : activeView === "settings"
                    ? "AI 工作台设置"
                    : activeConversation.title}
              </h1>
              <div className="ai-workbench-preview__session-menu-wrap" ref={sessionMenuRef}>
                <button
                  ref={sessionMenuTriggerRef}
                  className="ai-workbench-preview__icon-button ai-workbench-preview__menu-button"
                  type="button"
                  aria-label="会话操作"
                  aria-haspopup="menu"
                  aria-expanded={sessionMenuOpen}
                  aria-controls={sessionMenuOpen ? "ai-workbench-preview-session-menu" : undefined}
                  onClick={() => setSessionMenuOpen((open) => !open)}
                  title="会话操作"
                >
                  <img src="/icons/ai-workbench/conversation-menu.svg" width="16" height="16" alt="" />
                </button>
                {sessionMenuOpen && (
                  <div
                    id="ai-workbench-preview-session-menu"
                    className="ai-workbench-preview__session-menu"
                    role="menu"
                    aria-label={`会话操作：${activeConversation.title}`}
                    onKeyDown={(event) => {
                      const menuItems = Array.from(
                        event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'),
                      )
                      const currentIndex = menuItems.indexOf(document.activeElement as HTMLButtonElement)
                      let nextIndex = currentIndex
                      if (event.key === "ArrowDown") nextIndex = (currentIndex + 1) % menuItems.length
                      else if (event.key === "ArrowUp") nextIndex = (currentIndex - 1 + menuItems.length) % menuItems.length
                      else if (event.key === "Home") nextIndex = 0
                      else if (event.key === "End") nextIndex = menuItems.length - 1
                      else return

                      event.preventDefault()
                      menuItems[nextIndex]?.focus()
                    }}
                  >
                    <button
                      ref={firstSessionMenuItemRef}
                      className="ai-workbench-preview__session-menu-item"
                      type="button"
                      role="menuitem"
                      onClick={openRenameDialog}
                    >
                      <span aria-hidden="true">✎</span>
                      重命名会话
                    </button>
                    <button
                      className="ai-workbench-preview__session-menu-item ai-workbench-preview__session-menu-item--danger"
                      type="button"
                      role="menuitem"
                      onClick={() => {
                        setSessionMenuOpen(false)
                        setDeleteDialogOpen(true)
                      }}
                    >
                      <span aria-hidden="true">⌫</span>
                      删除会话
                    </button>
                  </div>
                )}
              </div>
              <span className="ai-workbench-preview__preview-badge">交互预览</span>
            </div>
            <div
              className="ai-workbench-preview__toolbar-actions"
              aria-label={activeView === "conversation" ? "会话工具" : "页面工具"}
            >
              {activeView === "conversation" ? (
                <>
                  <PreviewToolbarButton
                    label="新建对话"
                    icon="plus-lg"
                    onClick={startNewConversation}
                  />
                  <PreviewToolbarButton
                    label="会话历史"
                    icon="clock-history"
                    onClick={() => setHistoryOpen((open) => !open)}
                    ariaExpanded={historyOpen}
                    ariaControls="ai-workbench-preview-history"
                  />
                  <PreviewToolbarButton
                    label="执行记录"
                    icon="activity"
                    onClick={() => {
                      setHistoryOpen(false)
                      setActiveView("execution-records")
                    }}
                  />
                  <PreviewToolbarButton
                    label="AI 设置"
                    icon="gear"
                    onClick={() => {
                      setHistoryOpen(false)
                      setActiveView("settings")
                    }}
                  />
                </>
              ) : (
                <button
                  className="ai-workbench-preview__view-back"
                  type="button"
                  onClick={() => {
                    setHistoryOpen(false)
                    setActiveView("conversation")
                  }}
                >
                  <span aria-hidden="true">←</span>
                  返回对话
                </button>
              )}
              {historyOpen && (
                <div
                  id="ai-workbench-preview-history"
                  className="ai-workbench-preview__history"
                  role="region"
                  aria-label="示例会话历史"
                >
                  <div className="ai-workbench-preview__history-heading">
                    <span>近期对话</span>
                    <small>仅供预览 · 不会保存</small>
                  </div>
                  <div className="ai-workbench-preview__history-list">
                    {conversations.map((conversation) => (
                      <button
                        key={conversation.id}
                        type="button"
                        className={
                          "ai-workbench-preview__history-item" +
                          (conversation.id === activeConversation.id ? " is-active" : "")
                        }
                        aria-pressed={conversation.id === activeConversation.id}
                        onClick={() => selectConversation(conversation)}
                      >
                        <span>{conversation.title}</span>
                        <small>{conversation.summary}</small>
                      </button>
                    ))}
                  </div>
                </div>
              )}
            </div>
          </header>

          {activeView === "settings" ? (
            <AIWorkbenchSettingsView
              sendShortcut={sendShortcut}
              onSendShortcutChange={setSendShortcut}
              showContextUsage={showContextUsage}
              onShowContextUsageChange={setShowContextUsage}
            />
          ) : activeView === "execution-records" ? (
            <ExecutionRecordsView
              conversationTitle={activeConversation.title}
              onBackToConversation={() => setActiveView("conversation")}
            />
          ) : (
            <>
              <div className="ai-workbench-preview__conversation">
                <div
                  className="ai-workbench-preview__thread"
                  role="log"
                  aria-label="对话内容"
                  aria-live="polite"
                  aria-busy={activeConversation.isResponding}
                >
                  {activeConversation.messages.length === 0 ? (
                    <EmptyConversation
                      onUsePrompt={setPrompt}
                      onShowFailureExample={showFailureExample}
                    />
                  ) : (
                    <>
                      {activeConversation.messages.map((message) => (
                        <PreviewMessageBubble
                          key={message.id}
                          message={message}
                          retrying={activeConversation.retryingMessageId === message.id}
                          onRetry={() => retryExample(activeConversation.id, message.id)}
                        />
                      ))}

                      {activeConversation.showProposal && (
                        <ProposalReview
                          decision={activeConversation.proposalDecision}
                          onDecide={decideProposal}
                        />
                      )}

                      {activeConversation.isResponding && activeConversation.retryingMessageId === null && (
                        <div className="ai-workbench-preview__loading" role="status">
                          <span className="ai-workbench-preview__loading-dots" aria-hidden="true">
                            <i />
                            <i />
                            <i />
                          </span>
                          <span>正在准备示例反馈…</span>
                          <small>仅在本页演示，不会调用模型。</small>
                        </div>
                      )}
                      <div ref={threadEndRef} aria-hidden="true" />
                    </>
                  )}
                </div>
              </div>

              <footer className="ai-workbench-preview__composer">
                <p className="ai-workbench-preview__review-note">
                  交互预览：消息与反馈仅在本页面展示，不调用模型，也不会读取或更改设置。
                </p>
                <form className="ai-workbench-preview__input" onSubmit={handleSendMessage}>
                  <input
                    type="text"
                    value={prompt}
                    onChange={(event) => setPrompt(event.target.value)}
                    onKeyDown={handleComposerKeyDown}
                    placeholder="描述你希望调整的设置…"
                    aria-label="描述你想调整的设置"
                    aria-describedby="ai-workbench-preview-composer-note"
                    autoComplete="off"
                  />
                  <button
                    type="submit"
                    aria-label="发送预览消息"
                    title="只在当前页面演示，不会发送给模型"
                    disabled={!prompt.trim() || activeConversation.isResponding}
                  >
                    {activeConversation.isResponding ? "请稍候" : "发送"}
                  </button>
                </form>
                <p id="ai-workbench-preview-composer-note" className="ai-workbench-preview__composer-disclaimer">
                  输入内容不会发送给模型或保存；此处只演示消息、加载和错误状态。
                </p>
                {showContextUsage && (
                  <div className="ai-workbench-preview__context" aria-label="示意上下文用量 42%">
                    <span className="ai-workbench-preview__context-label">当前上下文：</span>
                    <div className="ai-workbench-preview__context-track" aria-hidden="true">
                      <span />
                    </div>
                    <span className="ai-workbench-preview__context-value">示意 42%</span>
                    <span className="ai-workbench-preview__compression-indicator">
                      <img src="/icons/ai-workbench/context-auto-compression.svg" width="6" height="6" alt="" />
                    </span>
                    <span className="ai-workbench-preview__compression-label">自动压缩示意</span>
                  </div>
                )}
                <p className="ai-workbench-preview__notice" role="status" aria-live="polite">{notice}</p>
              </footer>
            </>
          )}
        </div>
      </section>

      {renameDialogOpen && (
        <div
          className="ai-workbench-preview__dialog-backdrop"
          onClick={(event) => {
            if (event.target !== event.currentTarget) return
            setRenameDialogOpen(false)
            sessionMenuTriggerRef.current?.focus()
          }}
        >
          <div
            className="ai-workbench-preview__dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="ai-workbench-preview-rename-title"
            aria-describedby="ai-workbench-preview-rename-note"
            onKeyDown={(event) => handleSessionDialogKeyDown(event, () => {
              setRenameDialogOpen(false)
              sessionMenuTriggerRef.current?.focus()
            })}
          >
            <h2 id="ai-workbench-preview-rename-title">重命名会话</h2>
            <p id="ai-workbench-preview-rename-note">只修改当前页面的预览标题，不会保存真实会话。</p>
            <form onSubmit={saveConversationTitle}>
              <label htmlFor="ai-workbench-preview-rename-input">会话名称</label>
              <input
                id="ai-workbench-preview-rename-input"
                autoFocus
                value={renameDraft}
                maxLength={60}
                onChange={(event) => setRenameDraft(event.target.value)}
              />
              <div className="ai-workbench-preview__dialog-actions">
                <button
                  type="button"
                  className="ai-workbench-preview__dialog-secondary"
                  onClick={() => {
                    setRenameDialogOpen(false)
                    sessionMenuTriggerRef.current?.focus()
                  }}
                >
                  取消
                </button>
                <button type="submit" className="ai-workbench-preview__dialog-primary" disabled={!renameDraft.trim()}>
                  保存名称
                </button>
              </div>
            </form>
          </div>
        </div>
      )}

      {deleteDialogOpen && (
        <div
          className="ai-workbench-preview__dialog-backdrop"
          onClick={(event) => {
            if (event.target !== event.currentTarget) return
            setDeleteDialogOpen(false)
            sessionMenuTriggerRef.current?.focus()
          }}
        >
          <div
            className="ai-workbench-preview__dialog"
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="ai-workbench-preview-delete-title"
            aria-describedby="ai-workbench-preview-delete-note"
            onKeyDown={(event) => handleSessionDialogKeyDown(event, () => {
              setDeleteDialogOpen(false)
              sessionMenuTriggerRef.current?.focus()
            })}
          >
            <h2 id="ai-workbench-preview-delete-title">删除此会话？</h2>
            <p id="ai-workbench-preview-delete-note">
              将从当前预览中移除「{activeConversation.title}」。这不会删除或保存真实对话。
            </p>
            <div className="ai-workbench-preview__dialog-actions">
              <button
                type="button"
                className="ai-workbench-preview__dialog-secondary"
                autoFocus
                onClick={() => {
                  setDeleteDialogOpen(false)
                  sessionMenuTriggerRef.current?.focus()
                }}
              >
                取消删除
              </button>
              <button
                type="button"
                className="ai-workbench-preview__dialog-danger"
                onClick={deleteActiveConversation}
              >
                删除会话
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}

function ExecutionRecordsView({
  conversationTitle,
  onBackToConversation,
}: {
  conversationTitle: string
  onBackToConversation: () => void
}) {
  const [selectedRecordId, setSelectedRecordId] = useState<string | null>(null)
  const selectedRecord = PREVIEW_EXECUTION_RECORDS.find(({ id }) => id === selectedRecordId) ?? null

  return (
    <section className="ai-workbench-preview__execution-page" aria-label="执行记录">
      <div className="ai-workbench-preview__execution-heading">
        <div>
          <p>当前对话</p>
          <h2>{conversationTitle}</h2>
          <span>工具调用与状态 · 设计预览</span>
        </div>
      </div>

      <p className="ai-workbench-preview__execution-preview-note" role="note">
        设计预览：以下为固定示例，不代表真实工具调用，也不会修改或保存设置。
      </p>

      {selectedRecord ? (
        <section className="ai-workbench-preview__execution-detail" aria-label="执行记录详情">
          <header className="ai-workbench-preview__execution-detail-header">
            <button
              className="ai-workbench-preview__execution-back-list"
              type="button"
              aria-label="返回执行记录列表"
              onClick={() => setSelectedRecordId(null)}
            >
              <span aria-hidden="true">←</span>
              返回记录列表
            </button>
            <span
              className="ai-workbench-preview__execution-status"
              data-state={selectedRecord.status}
            >
              示例 · {selectedRecord.status === "completed" ? "已完成" : "未完成"}
            </span>
            <h3>{selectedRecord.title}</h3>
            <div className="ai-workbench-preview__execution-meta">
              <span>{selectedRecord.timeLabel}</span>
              <span>{selectedRecord.durationLabel}</span>
            </div>
          </header>

          <ol className="ai-workbench-preview__execution-steps" aria-label="示例工具执行步骤">
            {selectedRecord.steps.map((step, index) => (
              <li
                className="ai-workbench-preview__execution-step"
                data-state={step.status}
                key={step.name}
              >
                <span className="ai-workbench-preview__execution-step-marker" aria-hidden="true">
                  {step.status === "completed" ? "✓" : "!"}
                </span>
                <div className="ai-workbench-preview__execution-step-copy">
                  <div className="ai-workbench-preview__execution-step-heading">
                    <div>
                      <small>示例步骤 {index + 1} · 工具调用</small>
                      <strong>{step.name}</strong>
                    </div>
                    <span>{step.status === "completed" ? "已完成" : "未完成"}</span>
                  </div>
                  <p>{step.description}</p>
                  <small>{step.durationLabel}</small>
                </div>
              </li>
            ))}
          </ol>

          {selectedRecord.status === "completed" ? (
            <section className="ai-workbench-preview__execution-outcome" aria-labelledby="ai-workbench-preview-result-title">
              <h4 id="ai-workbench-preview-result-title">结果摘要</h4>
              <p>{selectedRecord.resultSummary ?? "固定示例结果；没有应用任何设置。"}</p>
            </section>
          ) : (
            <section
              className="ai-workbench-preview__execution-outcome ai-workbench-preview__execution-outcome--error"
              aria-labelledby="ai-workbench-preview-error-title"
              role="alert"
            >
              <h4 id="ai-workbench-preview-error-title">执行未完成</h4>
              <p>{selectedRecord.errorMessage ?? "示例执行未完成；没有更改任何阅读设置。"}</p>
            </section>
          )}

          <footer className="ai-workbench-preview__execution-detail-footer">
            <p>固定预览样例，不是真实运行记录；不会更改或保存阅读设置。</p>
            <button type="button" onClick={onBackToConversation}>回到对话</button>
          </footer>
        </section>
      ) : (
        <section className="ai-workbench-preview__execution-list" aria-label="执行记录列表">
          <div className="ai-workbench-preview__execution-list-heading">
            <h3>示例记录</h3>
            <span>{PREVIEW_EXECUTION_RECORDS.length} 条固定示例</span>
          </div>
          <ol className="ai-workbench-preview__execution-records">
            {PREVIEW_EXECUTION_RECORDS.map((record) => (
              <li key={record.id}>
                <button
                  className="ai-workbench-preview__execution-record"
                  type="button"
                  aria-label={`查看示例记录：${record.title}`}
                  onClick={() => setSelectedRecordId(record.id)}
                >
                  <span
                    className="ai-workbench-preview__execution-status"
                    data-state={record.status}
                  >
                    示例 · {record.status === "completed" ? "已完成" : "未完成"}
                  </span>
                  <strong>{record.title}</strong>
                  <span className="ai-workbench-preview__execution-record-summary">{record.summary}</span>
                  <span className="ai-workbench-preview__execution-meta">
                    <span>{record.timeLabel}</span>
                    <span>{record.durationLabel}</span>
                  </span>
                  <span className="ai-workbench-preview__execution-record-arrow" aria-hidden="true">→</span>
                </button>
              </li>
            ))}
          </ol>
        </section>
      )}
    </section>
  )
}

function AIWorkbenchSettingsView({
  sendShortcut,
  onSendShortcutChange,
  showContextUsage,
  onShowContextUsageChange,
}: {
  sendShortcut: PreviewSendShortcut
  onSendShortcutChange: (shortcut: PreviewSendShortcut) => void
  showContextUsage: boolean
  onShowContextUsageChange: (visible: boolean) => void
}) {
  const settingsScrollRef = useRef<HTMLElement>(null)
  const automaticMcpTabRef = useRef<HTMLButtonElement>(null)
  const manualMcpTabRef = useRef<HTMLButtonElement>(null)
  const mcpSequenceRef = useRef(0)
  const skillSequenceRef = useRef(0)
  const [mcpServices, setMcpServices] = useState<PreviewMcpService[]>([])
  const [mcpSetupMode, setMcpSetupMode] = useState<PreviewMcpSetupMode>("automatic")
  const [mcpScope, setMcpScope] = useState<PreviewMcpScope>("user")
  const [selectedMcpCatalogId, setSelectedMcpCatalogId] = useState<string | null>(null)
  const [mcpDraftName, setMcpDraftName] = useState("")
  const [mcpTransport, setMcpTransport] = useState<PreviewMcpTransport>("stdio")
  const [mcpDraftConnection, setMcpDraftConnection] = useState("")
  const [skills, setSkills] = useState<PreviewSkill[]>([
    {
      id: "reading-guide",
      name: "阅读建议（示例）",
      description: "演示技能启用与停用状态",
      enabled: true,
    },
    {
      id: "long-form-summary",
      name: "长文摘要（示例）",
      description: "演示技能列表的管理方式",
      enabled: false,
    },
  ])
  const [skillQuery, setSkillQuery] = useState("")
  const [skillFilter, setSkillFilter] = useState<PreviewSkillFilter>("all")
  const [skillFormOpen, setSkillFormOpen] = useState(false)
  const [skillDraftName, setSkillDraftName] = useState("")
  const [skillDraftDescription, setSkillDraftDescription] = useState("")

  useLayoutEffect(() => {
    if (settingsScrollRef.current) settingsScrollRef.current.scrollTop = 0
  }, [])

  const selectedMcpCatalogEntry = PREVIEW_MCP_CATALOG.find(
    ({ id }) => id === selectedMcpCatalogId,
  ) ?? null

  const handleMcpTabKeyDown = (
    event: ReactKeyboardEvent<HTMLButtonElement>,
    currentMode: PreviewMcpSetupMode,
  ) => {
    if (!(["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"] as string[]).includes(event.key)) {
      return
    }

    event.preventDefault()
    const nextMode: PreviewMcpSetupMode = event.key === "Home"
      ? "automatic"
      : event.key === "End"
        ? "manual"
        : currentMode === "automatic" ? "manual" : "automatic"
    setMcpSetupMode(nextMode)
    if (nextMode === "automatic") automaticMcpTabRef.current?.focus()
    else manualMcpTabRef.current?.focus()
  }

  const handleAddMcpService = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const name = mcpDraftName.trim()
    if (!name || !mcpDraftConnection.trim()) return

    setMcpServices((current) => [
      ...current,
      {
        id: mcpSequenceRef.current++,
        name,
        transport: mcpTransport,
        setupMode: "manual",
        scope: mcpScope,
      },
    ])
    setMcpDraftName("")
    setMcpDraftConnection("")
  }

  const handleAddCatalogPreview = () => {
    if (!selectedMcpCatalogEntry) return

    setMcpServices((current) => [
      ...current,
      {
        id: mcpSequenceRef.current++,
        name: selectedMcpCatalogEntry.name,
        transport: selectedMcpCatalogEntry.transport,
        setupMode: "automatic",
        scope: mcpScope,
      },
    ])
    setSelectedMcpCatalogId(null)
  }

  const toggleSkill = (skillId: string) => {
    setSkills((current) => current.map((skill) => skill.id === skillId
      ? { ...skill, enabled: !skill.enabled }
      : skill,
    ))
  }

  const handleAddSkill = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const name = skillDraftName.trim()
    const description = skillDraftDescription.trim()
    if (!name || !description) return

    setSkills((current) => [
      ...current,
      {
        id: "preview-added-" + skillSequenceRef.current++,
        name,
        description,
        enabled: false,
      },
    ])
    setSkillDraftName("")
    setSkillDraftDescription("")
    setSkillFormOpen(false)
  }

  const filteredSkills = skills.filter((skill) => {
    const matchesQuery = (skill.name + " " + skill.description).toLocaleLowerCase().includes(skillQuery.toLocaleLowerCase())
    const matchesFilter = skillFilter === "all"
      || (skillFilter === "enabled" ? skill.enabled : !skill.enabled)
    return matchesQuery && matchesFilter
  })

  return (
    <section
      ref={settingsScrollRef}
      className="ai-workbench-preview__settings-page"
      aria-label="AI 工作台设置"
    >
      <div className="ai-workbench-preview__settings-content">
        <header className="ai-workbench-preview__settings-heading">
          <span>工作台偏好</span>
          <h2>设置你的 AI 工作方式</h2>
          <p>查看模型连接、工具服务与技能管理，并预览常用对话偏好。</p>
        </header>

        <p className="ai-workbench-preview__settings-preview-note" role="note">
          设计预览：以下选项只影响当前页面，不会连接模型、读取密钥或保存配置。
        </p>

        <section className="ai-workbench-preview__service-status" aria-label="模型服务配置预览">
          <div className="ai-workbench-preview__service-status-heading">
            <span className="ai-workbench-preview__service-mark" aria-hidden="true">
              <BootstrapIcon name="stars" size={18} />
            </span>
            <div className="ai-workbench-preview__service-copy">
              <span>模型服务</span>
              <strong>API Key 配置</strong>
              <p>服务接入尚未实现；当前不会接收密钥或发起连接。</p>
            </div>
            <span className="ai-workbench-preview__service-badge">尚未接入</span>
          </div>
          <div className="ai-workbench-preview__api-key-preview">
            <label htmlFor="ai-workbench-preview-api-key">API Key</label>
            <input
              id="ai-workbench-preview-api-key"
              type="password"
              placeholder="配置流程尚未接入"
              aria-describedby="ai-workbench-preview-api-key-note"
              disabled
            />
            <small id="ai-workbench-preview-api-key-note">
              此处只展示配置入口的位置，请勿向预览输入真实密钥。
            </small>
          </div>
        </section>

        <div className="ai-workbench-preview__settings-grid">
          <section
            className="ai-workbench-preview__settings-card ai-workbench-preview__settings-card--wide"
            aria-labelledby="ai-workbench-preview-mcp-title"
          >
            <div className="ai-workbench-preview__settings-card-heading">
              <div>
                <h3 id="ai-workbench-preview-mcp-title">MCP 服务</h3>
                <p>从推荐配置快速开始，或手动添加已有服务；连接前先确认作用范围与所需权限。</p>
              </div>
              <BootstrapIcon name="link-45deg" size={17} />
            </div>
            <div className="ai-workbench-preview__mcp-setup-tabs" role="tablist" aria-label="添加 MCP 的方式">
              <button
                ref={automaticMcpTabRef}
                id="ai-workbench-preview-mcp-auto-tab"
                type="button"
                role="tab"
                aria-selected={mcpSetupMode === "automatic"}
                aria-controls="ai-workbench-preview-mcp-setup-panel"
                tabIndex={mcpSetupMode === "automatic" ? 0 : -1}
                onKeyDown={(event) => handleMcpTabKeyDown(event, "automatic")}
                onClick={() => setMcpSetupMode("automatic")}
              >
                自动配置 <span>推荐</span>
              </button>
              <button
                ref={manualMcpTabRef}
                id="ai-workbench-preview-mcp-manual-tab"
                type="button"
                role="tab"
                aria-selected={mcpSetupMode === "manual"}
                aria-controls="ai-workbench-preview-mcp-setup-panel"
                tabIndex={mcpSetupMode === "manual" ? 0 : -1}
                onKeyDown={(event) => handleMcpTabKeyDown(event, "manual")}
                onClick={() => setMcpSetupMode("manual")}
              >
                手动配置
              </button>
            </div>
            <div
              className="ai-workbench-preview__mcp-setup-panel"
              id="ai-workbench-preview-mcp-setup-panel"
              role="tabpanel"
              aria-labelledby={mcpSetupMode === "automatic"
                ? "ai-workbench-preview-mcp-auto-tab"
                : "ai-workbench-preview-mcp-manual-tab"}
            >
              {mcpSetupMode === "automatic" ? (
                <div className="ai-workbench-preview__mcp-automatic">
                  <div className="ai-workbench-preview__mcp-automatic-intro">
                    <div>
                      <h4>选择一个服务配置</h4>
                      <p>栖阅会先准备连接信息，再展示权限和授权步骤供你确认。</p>
                    </div>
                    <span>先审阅，再添加</span>
                  </div>
                  <div className="ai-workbench-preview__mcp-directory" aria-label="推荐配置示例">
                    {PREVIEW_MCP_CATALOG.map((entry) => (
                      <div
                        className="ai-workbench-preview__mcp-directory-row"
                        data-selected={selectedMcpCatalogId === entry.id}
                        key={entry.id}
                      >
                        <div className="ai-workbench-preview__mcp-directory-copy">
                          <strong>{entry.name}</strong>
                          <span>{entry.description}</span>
                        </div>
                        <span className="ai-workbench-preview__mcp-directory-meta">
                          {entry.transport === "stdio" ? "本机服务" : "远程服务"} · {entry.authorization}
                        </span>
                        <button
                          type="button"
                          aria-pressed={selectedMcpCatalogId === entry.id}
                          onClick={() => setSelectedMcpCatalogId(entry.id)}
                        >
                          {selectedMcpCatalogId === entry.id ? "已选择" : "选择配置"}
                        </button>
                      </div>
                    ))}
                  </div>
                  {selectedMcpCatalogEntry && (
                    <section className="ai-workbench-preview__mcp-review" aria-label="自动配置确认">
                      <div className="ai-workbench-preview__mcp-review-heading">
                        <div>
                          <span>配置前确认</span>
                          <h4>{selectedMcpCatalogEntry.name}</h4>
                        </div>
                        <span className="ai-workbench-preview__mcp-preview-badge">仅流程预览</span>
                      </div>
                      <div className="ai-workbench-preview__mcp-review-facts">
                        <div>
                          <span>连接方式</span>
                          <strong>{selectedMcpCatalogEntry.transport === "stdio" ? "本机进程" : "远程服务"}</strong>
                        </div>
                        <div>
                          <span>授权步骤</span>
                          <strong>{selectedMcpCatalogEntry.authorization}</strong>
                        </div>
                        <div>
                          <span>权限说明</span>
                          <strong>{selectedMcpCatalogEntry.permissionSummary}</strong>
                        </div>
                      </div>
                      <div className="ai-workbench-preview__mcp-scope-row">
                        <span>添加范围</span>
                        <div role="radiogroup" aria-label="自动配置范围">
                          <button
                            type="button"
                            role="radio"
                            aria-checked={mcpScope === "user"}
                            onClick={() => setMcpScope("user")}
                          >
                            个人配置
                          </button>
                          <button
                            type="button"
                            role="radio"
                            aria-checked={mcpScope === "workspace"}
                            onClick={() => setMcpScope("workspace")}
                          >
                            当前工作区
                          </button>
                        </div>
                      </div>
                      <div className="ai-workbench-preview__mcp-review-footer">
                        <p>真实连接前应由你确认服务来源、授权内容与工具权限；此预览不会执行这些步骤。</p>
                        <button className="ai-workbench-preview__form-primary-action" type="button" onClick={handleAddCatalogPreview}>
                          添加到预览配置
                        </button>
                      </div>
                    </section>
                  )}
                </div>
              ) : (
                <form className="ai-workbench-preview__mcp-form" onSubmit={handleAddMcpService}>
                  <div className="ai-workbench-preview__mcp-form-fields">
                    <label className="ai-workbench-preview__form-field">
                      <span>服务名称</span>
                      <input
                        type="text"
                        value={mcpDraftName}
                        onChange={(event) => setMcpDraftName(event.target.value)}
                        placeholder="例如：本地资料检索"
                        aria-label="服务名称"
                        required
                      />
                    </label>
                    <label className="ai-workbench-preview__form-field">
                      <span>连接方式</span>
                      <select
                        value={mcpTransport}
                        onChange={(event) => setMcpTransport(event.target.value as PreviewMcpTransport)}
                        aria-label="连接方式"
                      >
                        <option value="stdio">本地进程</option>
                        <option value="remote">远程服务</option>
                      </select>
                    </label>
                    <label className="ai-workbench-preview__form-field">
                      <span>添加范围</span>
                      <select
                        value={mcpScope}
                        onChange={(event) => setMcpScope(event.target.value as PreviewMcpScope)}
                        aria-label="配置范围"
                      >
                        <option value="user">个人配置</option>
                        <option value="workspace">当前工作区</option>
                      </select>
                    </label>
                    <label className="ai-workbench-preview__form-field ai-workbench-preview__form-field--wide">
                      <span>{mcpTransport === "stdio" ? "启动命令" : "服务地址"}</span>
                      <input
                        type="text"
                        value={mcpDraftConnection}
                        onChange={(event) => setMcpDraftConnection(event.target.value)}
                        placeholder={mcpTransport === "stdio"
                          ? "预览输入，不会执行命令"
                          : "预览输入，不会发起连接"}
                        aria-label="连接信息"
                        required
                      />
                    </label>
                  </div>
                  <div className="ai-workbench-preview__form-actions">
                    <button
                      className="ai-workbench-preview__settings-secondary-action"
                      type="button"
                      onClick={() => {
                        setMcpDraftName("")
                        setMcpDraftConnection("")
                        setMcpTransport("stdio")
                        setMcpScope("user")
                      }}
                    >
                      清空
                    </button>
                    <button className="ai-workbench-preview__form-primary-action" type="submit">
                      添加到预览配置
                    </button>
                  </div>
                </form>
              )}
            </div>
            <div className="ai-workbench-preview__mcp-list" aria-label="MCP 服务列表">
              <div className="ai-workbench-preview__management-list-heading">
                <strong>已添加配置</strong>
                <span>{mcpServices.length} 项 · 当前预览</span>
              </div>
              {mcpServices.length > 0 ? (
                <ul>
                  {mcpServices.map((service) => (
                    <li className="ai-workbench-preview__mcp-row" key={service.id}>
                      <span className="ai-workbench-preview__mcp-row-name">{service.name}</span>
                      <span>{service.transport === "stdio" ? "本机进程" : "远程服务"}</span>
                      <span>{service.scope === "user" ? "个人配置" : "当前工作区"}</span>
                      <span className="ai-workbench-preview__mcp-state">
                        {service.setupMode === "automatic" ? "仅预览 · 待授权" : "仅预览 · 未连接"}
                      </span>
                      <button
                        type="button"
                        aria-label={`移除 ${service.name}`}
                        onClick={() => setMcpServices((current) => current.filter(({ id }) => id !== service.id))}
                      >
                        移除
                      </button>
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="ai-workbench-preview__mcp-empty">还没有添加 MCP 服务。可以从推荐配置开始，或切换到手动配置。</p>
              )}
            </div>
            <p className="ai-workbench-preview__settings-card-footnote">
              目录内容和连接流程目前仅作交互预览；不会扫描本机、执行命令、连接服务器或保存授权信息。
            </p>
          </section>

          <section
            className="ai-workbench-preview__settings-card ai-workbench-preview__settings-card--wide"
            aria-labelledby="ai-workbench-preview-skills-title"
          >
            <div className="ai-workbench-preview__settings-card-heading">
              <div>
                <h3 id="ai-workbench-preview-skills-title">Skills 管理</h3>
                <p>搜索、筛选并管理工作台可用的技能</p>
              </div>
              <BootstrapIcon name="grid" size={17} />
            </div>
            <div className="ai-workbench-preview__skills-toolbar">
              <label className="ai-workbench-preview__skills-search">
                <BootstrapIcon name="search" size={15} />
                <input
                  type="search"
                  value={skillQuery}
                  onChange={(event) => setSkillQuery(event.target.value)}
                  placeholder="搜索 Skills"
                  aria-label="搜索 Skills"
                />
              </label>
              <div className="ai-workbench-preview__skill-filters" role="radiogroup" aria-label="技能状态筛选">
                {([
                  ["all", "全部"],
                  ["enabled", "已启用"],
                  ["disabled", "已停用"],
                ] as const).map(([value, label]) => (
                  <button
                    type="button"
                    role="radio"
                    aria-checked={skillFilter === value}
                    key={value}
                    onClick={() => setSkillFilter(value)}
                  >
                    {label}
                  </button>
                ))}
              </div>
              <button
                className="ai-workbench-preview__settings-secondary-action"
                type="button"
                aria-expanded={skillFormOpen}
                aria-controls="ai-workbench-preview-add-skill-form"
                onClick={() => setSkillFormOpen((open) => !open)}
              >
                {skillFormOpen ? "收起表单" : "添加示例 Skill"}
              </button>
            </div>
            {skillFormOpen && (
              <form
                id="ai-workbench-preview-add-skill-form"
                className="ai-workbench-preview__skill-form"
                onSubmit={handleAddSkill}
              >
                <label className="ai-workbench-preview__form-field">
                  <span>技能名称</span>
                  <input
                    value={skillDraftName}
                    onChange={(event) => setSkillDraftName(event.target.value)}
                    placeholder="为示例 Skill 命名"
                    aria-label="技能名称"
                    required
                  />
                </label>
                <label className="ai-workbench-preview__form-field">
                  <span>说明</span>
                  <input
                    value={skillDraftDescription}
                    onChange={(event) => setSkillDraftDescription(event.target.value)}
                    placeholder="简要说明技能用途"
                    aria-label="技能说明"
                    required
                  />
                </label>
                <div className="ai-workbench-preview__form-actions">
                  <button className="ai-workbench-preview__settings-secondary-action" type="button" onClick={() => setSkillFormOpen(false)}>
                    取消
                  </button>
                  <button className="ai-workbench-preview__form-primary-action" type="submit">
                    添加到示例列表
                  </button>
                </div>
              </form>
            )}
            <div className="ai-workbench-preview__skill-list" aria-label="技能示例列表">
              <div className="ai-workbench-preview__management-list-heading">
                <strong>技能列表</strong>
                <span>{skills.length} 个示例</span>
              </div>
              {filteredSkills.length > 0 ? filteredSkills.map((skill) => (
                <div className="ai-workbench-preview__skill-row" key={skill.id}>
                  <span className="ai-workbench-preview__skill-copy">
                    <strong>{skill.name}</strong>
                    <small>{skill.description}</small>
                  </span>
                  <span className="ai-workbench-preview__skill-row-actions">
                    <button
                      className="ai-workbench-preview__setting-switch"
                      type="button"
                      role="switch"
                      aria-label={`启用 ${skill.name}`}
                      aria-checked={skill.enabled}
                      onClick={() => toggleSkill(skill.id)}
                    >
                      <span aria-hidden="true" />
                    </button>
                    <button
                      className="ai-workbench-preview__skill-remove"
                      type="button"
                      aria-label={`移除 ${skill.name}`}
                      onClick={() => setSkills((current) => current.filter(({ id }) => id !== skill.id))}
                    >
                      移除
                    </button>
                  </span>
                </div>
              )) : (
                <p className="ai-workbench-preview__skills-empty">没有符合条件的 Skill 示例。</p>
              )}
            </div>
            <p className="ai-workbench-preview__settings-card-footnote">
              当前只演示搜索、筛选、添加与启停；示例技能不会执行，也不会保存。
            </p>
          </section>

          <section className="ai-workbench-preview__settings-card" aria-labelledby="ai-workbench-preview-send-title">
            <div className="ai-workbench-preview__settings-card-heading">
              <div>
                <h3 id="ai-workbench-preview-send-title">发送消息</h3>
                <p>选择你习惯的发送快捷键</p>
              </div>
              <BootstrapIcon name="activity" size={17} />
            </div>
            <div className="ai-workbench-preview__shortcut-options" role="radiogroup" aria-label="发送快捷键">
              <button
                className="ai-workbench-preview__shortcut-option"
                type="button"
                role="radio"
                aria-label="Enter 发送"
                aria-checked={sendShortcut === "enter"}
                onClick={() => onSendShortcutChange("enter")}
              >
                <kbd>Enter</kbd>
                <span>按 Enter 发送</span>
              </button>
              <button
                className="ai-workbench-preview__shortcut-option"
                type="button"
                role="radio"
                aria-label="Ctrl 或 Command + Enter 发送"
                aria-checked={sendShortcut === "ctrl-enter"}
                onClick={() => onSendShortcutChange("ctrl-enter")}
              >
                <kbd>Ctrl / ⌘ + Enter</kbd>
                <span>使用组合键发送</span>
              </button>
            </div>
            <p className="ai-workbench-preview__settings-card-footnote">
              更改后可立即在对话输入框中试用。
            </p>
          </section>

          <section className="ai-workbench-preview__settings-card" aria-labelledby="ai-workbench-preview-display-title">
            <div className="ai-workbench-preview__settings-card-heading">
              <div>
                <h3 id="ai-workbench-preview-display-title">对话显示</h3>
                <p>控制工作台中的辅助信息</p>
              </div>
              <BootstrapIcon name="grid" size={17} />
            </div>
            <div className="ai-workbench-preview__setting-row">
              <span className="ai-workbench-preview__setting-row-copy">
                <strong>上下文用量</strong>
                <small>在对话底部显示用量示意</small>
              </span>
              <button
                className="ai-workbench-preview__setting-switch"
                type="button"
                role="switch"
                aria-label="显示上下文用量"
                aria-checked={showContextUsage}
                onClick={() => onShowContextUsageChange(!showContextUsage)}
              >
                <span aria-hidden="true" />
              </button>
            </div>
            <div className="ai-workbench-preview__context-sample" aria-hidden="true">
              <span>上下文</span>
              <span className="ai-workbench-preview__context-sample-track"><i /></span>
              <strong>42%</strong>
            </div>
          </section>
        </div>

        <p className="ai-workbench-preview__settings-reset-note">
          示例状态只作用于当前预览，不会写入栖阅设置。
        </p>
      </div>
    </section>
  )
}

function EmptyConversation({
  onUsePrompt,
  onShowFailureExample,
}: {
  onUsePrompt: (prompt: string) => void
  onShowFailureExample: () => void
}) {
  return (
    <div className="ai-workbench-preview__empty-state">
      <span className="ai-workbench-preview__empty-kicker">本地交互预览</span>
      <h2>从一个想法开始</h2>
      <p>描述你希望在栖阅中调整的体验。这里可以预览消息流程，但不会连接模型或更改设置。</p>
      <div className="ai-workbench-preview__prompt-suggestions" aria-label="示例问题">
        <button type="button" onClick={() => onUsePrompt("帮我调整长篇阅读的字号")}>
          调整阅读字号
        </button>
        <button type="button" onClick={() => onUsePrompt("帮我挑选适合长文的字体")}>
          挑选阅读字体
        </button>
      </div>
      <button
        type="button"
        className="ai-workbench-preview__show-error-example"
        onClick={onShowFailureExample}
      >
        查看失败反馈示例
      </button>
    </div>
  )
}

function PreviewMessageBubble({
  message,
  retrying,
  onRetry,
}: {
  message: PreviewMessage
  retrying: boolean
  onRetry: () => void
}) {
  if (message.role === "user") {
    return (
      <article className="ai-workbench-preview__user-message">
        <p>{message.content}</p>
        <time>{message.timeLabel}</time>
      </article>
    )
  }

  const isError = message.kind === "error"
  return (
    <article
      role={isError ? "alert" : undefined}
      className={
        "ai-workbench-preview__assistant-message" +
        (isError ? " ai-workbench-preview__assistant-message--error" : "")
      }
    >
      {message.showToolPreview && (
        <div className="ai-workbench-preview__tool-chips" aria-label="示例能力状态">
          <span className="ai-workbench-preview__tool-chip is-tinted">
            <BootstrapIcon name="check" size={14} />
            阅读设置 · 未读取
          </span>
          <span className="ai-workbench-preview__tool-chip">
            <BootstrapIcon name="clock" size={14} />
            设备能力 · 未执行
          </span>
        </div>
      )}
      <p className="ai-workbench-preview__assistant-copy">{message.content}</p>
      {isError && !retrying && (
        <button
          type="button"
          className="ai-workbench-preview__retry"
          onClick={onRetry}
        >
          重试示例
        </button>
      )}
      {retrying && (
        <div className="ai-workbench-preview__retrying" role="status">
          <span className="ai-workbench-preview__loading-dots" aria-hidden="true">
            <i />
            <i />
            <i />
          </span>
          正在演示重试…
        </div>
      )}
      <p className="ai-workbench-preview__assistant-footnote">
        {message.footnote ?? "示例回复 · 未调用模型。"}
      </p>
    </article>
  )
}

function ProposalReview({
  decision,
  onDecide,
}: {
  decision: "accepted" | "rejected" | null
  onDecide: (decision: "accepted" | "rejected") => void
}) {
  return (
    <section className="ai-workbench-preview__proposal-review" aria-label="提案审阅示例">
      <header className="ai-workbench-preview__proposal-review-header">
        <div>
          <p className="ai-workbench-preview__proposal-review-scope">阅读设置 · 本机</p>
          <h2>阅读体验微调</h2>
        </div>
        <span
          className="ai-workbench-preview__proposal-review-status"
          data-state={decision ?? "pending"}
          aria-live="polite"
        >
          {decision === "accepted"
            ? "已确认 · 预览"
            : decision === "rejected"
              ? "已拒绝 · 预览"
              : "待你确认"}
        </span>
      </header>

      <div className="ai-workbench-preview__proposal-changes" aria-label="建议改动示意">
        <ProposalChange label="正文字号" from="18 px" to="20 px" />
        <ProposalChange label="行间距" from="1.7" to="1.8" />
      </div>

      <footer className="ai-workbench-preview__proposal-review-footer">
        <p>固定示意数值，不来自本机设置；操作只改变预览状态。</p>
        {decision === null ? (
          <div className="ai-workbench-preview__proposal-actions">
            <button type="button" className="ai-workbench-preview__proposal-reject" onClick={() => onDecide("rejected")}>
              拒绝建议
            </button>
            <button type="button" className="ai-workbench-preview__proposal-confirm" onClick={() => onDecide("accepted")}>
              确认应用
            </button>
          </div>
        ) : null}
      </footer>
    </section>
  )
}

function ProposalChange({ label, from, to }: { label: string; from: string; to: string }) {
  return (
    <div className="ai-workbench-preview__proposal-change">
      <span className="ai-workbench-preview__proposal-change-label">{label}</span>
      <span className="ai-workbench-preview__proposal-change-values">
        <span className="ai-workbench-preview__proposal-old-value">{from}</span>
        <span className="ai-workbench-preview__proposal-arrow" aria-hidden="true">→</span>
        <strong className="ai-workbench-preview__proposal-new-value">{to}</strong>
      </span>
    </div>
  )
}

function PreviewToolbarButton({
  label,
  icon,
  onClick,
  ariaExpanded,
  ariaControls,
}: {
  label: string
  icon: BootstrapIconName
  onClick: () => void
  ariaExpanded?: boolean
  ariaControls?: string
}) {
  return (
    <button
      className="ai-workbench-preview__icon-button"
      type="button"
      aria-label={label}
      aria-expanded={ariaExpanded}
      aria-controls={ariaControls}
      title={label}
      onClick={onClick}
    >
      <BootstrapIcon name={icon} size={16} />
    </button>
  )
}
