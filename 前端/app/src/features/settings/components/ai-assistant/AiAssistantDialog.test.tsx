// @vitest-environment jsdom
//
// 栖伴（Haven 智能体，FE-AI-ASSISTANT-001）测试：
// - 覆盖栖伴及其设置入口，不调用真实网络或桌面能力。
// - 用户可见名称是「栖伴」；组件名/文件名 ai-assistant / AiAssistantDialog 保持不变。
// - 生产设置页将 AI 工作台嵌入 AI 设置导航，下面也覆盖该正式入口和设置覆盖层。
// - 安全边界：组件不发起网络请求、不写 Settings、不写 localStorage、不调用 Tauri；
//   只有读写回调同时提供时才走真实应用层，否则一律本地预览，
//   本地预览草稿永远不会进入写入回调。

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { MemoryRouter, Route, Routes } from "react-router"
import { afterEach, describe, expect, it, vi } from "vitest"

import { SettingsPage } from "../../pages/SettingsPage"
import { HavenError } from "@/lib/ipc/errors"
import type {
  AgentSettingsProposalApproveRequest,
  AgentSettingsProposalCreateRequest,
  AgentSettingsProposalRejectRequest,
  AiSettingsRecommendationGenerateRequest,
} from "@/lib/ipc/generated/wire"
import {
  AiAssistantDialog,
  createPreviewProposal,
  proposalDigest,
  traceNodeStatuses,
  wantsReadingProposal,
  type AiAssistantAgentClient,
  type AiAssistantDialogProps,
} from "./AiAssistantDialog"

// 设置分区的可用性回归：没有真实绑定/消费者的能力不再以禁用控件或路线图文案出现在
// 生产设置页上；栖伴的模型服务尚未接入，因此它的入口在生产设置页上同样不存在。
vi.mock("@/lib/ipc/runtime", () => ({
  getHavenClientMode: () => "mock",
  // 「智能功能」分区会真的读 Provider 列表与 Broker 状态：空态用例必须提供一个
  // 真实的"没有 Provider / 默认关闭"响应，读取错误不能被伪装成空配置。
  getHavenClient: () => ({
    aiProviderProfileList: async () => ({ schemaVersion: 1, profiles: [] }),
    agentBrokerStatus: async () => ({
      schemaVersion: 1,
      status: "disabled",
      endpoint: null,
      reason: null,
    }),
  }),
  isTauriRuntime: () => false,
}))
// 分区表单与智能体逻辑无关：只保留一个空控制器，避免用例依赖各分区快照形状。
// displayValue 给一份合法的 general 快照——分区展示层各取所需（形状不匹配时回落到
// 该分区自己的默认值），页面因此可以正常渲染，而不需要为每个分区造假数据。
vi.mock("@/features/settings/lib/useSettingsForm", () => ({
  useSettingsForm: () => ({
    section: "general",
    state: { status: "loading" },
    displayValue: { section: "general", launchPage: "home", restoreSession: false, language: "zh_cn", notifications: true },
    isLoading: true,
    isSaving: false,
    isDirty: false,
    hasError: false,
    errorMessage: null,
    change: () => undefined,
    save: () => undefined,
    retry: () => undefined,
    reload: () => undefined,
    resetToDefaults: () => undefined,
  }),
}))

/**
 * 打开对话框。
 *
 * 注意本文件顶部的 `@/lib/ipc/runtime` mock 只为「智能功能分区」的空态提供一份
 * **不含任何 Agent 命令**的响应（`aiProviderProfileList` + `agentBrokerStatus`）：
 * 它不是一份完整的 `HavenClient`。因此凡是断言"本地预览（零写入）"语义的用例，
 * 都必须显式传 `previewOnly`，而不是让组件把那份残缺客户端当成应用层。
 * 需要真实应用层的用例一律显式注入 `client`。
 */
function renderDialog(props: Partial<AiAssistantDialogProps> = {}) {
  const onOpenChange = vi.fn()
  const view = render(
    <AiAssistantDialog open traceStepMs={0} {...props} onOpenChange={onOpenChange} />,
  )
  return { ...view, onOpenChange }
}

function proposalRegion() {
  return screen.getByRole("region", { name: "设置提案" })
}

/**
 * Typed IPC 假客户端：只实现组件用到的四条 Agent 命令。
 *
 * 它不伪造模型名，也不假装来自 Provider；测试用它验证
 * Context → Proposal → 用户批准 → Receipt 的契约形状与 UI 语义。
 */
const PROPOSAL_ID = "0196f0d2-0000-7000-8000-00000000e501"
/** 64 位小写十六进制，形状与 Rust canonical digest 一致。 */
const PROPOSAL_DIGEST = `${"a".repeat(63)}b`
const RECEIPT_ID = "0196f0d2-0000-7000-8000-00000000e601"
const SECOND_PROPOSAL_ID = "0196f0d2-0000-7000-8000-00000000e502"
const SECOND_PROPOSAL_DIGEST = `${"b".repeat(63)}c`

function agentContext(revision: string | null = "rev-41") {
  return {
    schemaVersion: 1 as const,
    contextId: "0196f0d2-0000-7000-8000-00000000e401",
    contextHash: "b".repeat(64),
    subject: { section: "reading" as const },
    revision,
    reading: {
      section: "reading" as const,
      fontFamily: "serif" as const,
      customFontFamily: null,
      fontSize: "medium" as const,
      lineHeight: "comfortable" as const,
      contentWidth: "medium" as const,
      theme: "warm" as const,
      customBackground: null,
      customText: null,
      fontWeight: "regular" as const,
      letterSpacing: "normal" as const,
      systemAuto: true,
      pagination: "scroll" as const,
      redactedFields: [],
    },
    capabilities: {
      agentApiVersion: 1,
      capabilities: {
        settingsRead: true,
        settingsProposal: true,
        librarySummaryRead: false,
        settingSourcesRead: false,
        resourcePreferenceRead: false,
        resourcePreferenceProposal: false,
        mediaCapabilitiesRead: false,
        onboardingRead: false,
        metadataProposal: false,
        renameProposal: false,
        secretRead: false,
        filesystemWrite: false,
      },
    },
  }
}

function proposalDto() {
  return {
    schemaVersion: 1 as const,
    proposalId: PROPOSAL_ID,
    status: "pending" as const,
    subject: { section: "reading" as const },
    targetLabel: "全局默认 · 阅读",
    baseRevision: "rev-41",
    digest: PROPOSAL_DIGEST,
    createdAt: "2026-09-16T10:00:00Z",
    expiresAt: "2026-09-17T10:00:00Z",
    changes: [
      { key: "reading.fontSize", before: "medium", after: "large" },
      { key: "reading.lineHeight", before: "comfortable", after: "airy" },
      { key: "reading.contentWidth", before: "medium", after: "narrow" },
    ],
  }
}

function proposalDtoFor(proposalId: string, digest: string) {
  return { ...proposalDto(), proposalId, digest }
}

function appliedResult() {
  return {
    proposal: { ...proposalDto(), status: "applied" as const },
    receipt: {
      schemaVersion: 1 as const,
      receiptId: RECEIPT_ID,
      proposalId: PROPOSAL_ID,
      proposalDigest: PROPOSAL_DIGEST,
      status: "applied" as const,
      appliedRevision: "rev-42",
      changed: true,
      changes: proposalDto().changes,
      appliedAt: "2026-09-16T10:00:05Z",
    },
  }
}

function fakeClient(overrides: Partial<AiAssistantAgentClient> = {}): AiAssistantAgentClient {
  return {
    agentSettingsContextGet: vi.fn(async () => agentContext()),
    agentSettingsProposalCreate: vi.fn(async (_request: AgentSettingsProposalCreateRequest) =>
      proposalDto(),
    ),
    agentSettingsProposalApprove: vi.fn(async (_request: AgentSettingsProposalApproveRequest) =>
      appliedResult(),
    ),
    agentSettingsProposalReject: vi.fn(async (_request: AgentSettingsProposalRejectRequest) => ({
      proposal: { ...proposalDto(), status: "rejected" as const },
    })),
    ...overrides,
  } as AiAssistantAgentClient
}

/** 手动放行的 promise：用来把一次请求**卡在在途状态**，再制造并发事件。 */
function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((complete) => { resolve = complete })
  return { promise, resolve }
}

/** 快捷操作是确定性入口：点击后固定进入 proposal 状态。 */
async function openProposal() {
  fireEvent.click(screen.getByRole("button", { name: "优化阅读" }))
  await screen.findByRole("button", { name: "批准并应用" })
}

async function openTwoProposals(client: AiAssistantAgentClient) {
  renderDialog({ client })
  await openProposal()
  // 首张提案生成后，快捷操作仍然可用；第二次生成用来固定“旧卡片不能
  // 操作最新提案”的回归行为。
  fireEvent.click(screen.getByRole("button", { name: "优化阅读" }))
  await waitFor(() => expect(screen.getAllByRole("region", { name: "设置提案" })).toHaveLength(2))
  return screen.getAllByRole("region", { name: "设置提案" })
}

function sequencedClient(): AiAssistantAgentClient {
  let proposalCount = 0
  return fakeClient({
    agentSettingsProposalCreate: vi.fn(async (_request: AgentSettingsProposalCreateRequest) => {
      proposalCount += 1
      return proposalCount === 1
        ? proposalDtoFor(PROPOSAL_ID, PROPOSAL_DIGEST)
        : proposalDtoFor(SECOND_PROPOSAL_ID, SECOND_PROPOSAL_DIGEST)
    }),
  })
}

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

describe("栖伴纯逻辑", () => {
  it("只对阅读排版意图生成提案，其余输入不生成", () => {
    expect(wantsReadingProposal("优化阅读")).toBe(true)
    expect(wantsReadingProposal("把正文字号调大一点")).toBe(true)
    expect(wantsReadingProposal("帮我配置下载并发")).toBe(false)
    expect(wantsReadingProposal("   ")).toBe(false)
  })

  it("digest 由提案内容派生且稳定", () => {
    const proposal = createPreviewProposal(1)
    const same = createPreviewProposal(2)
    expect(proposalDigest(proposal)).toBe(proposal.digest)
    expect(same.digest).toBe(proposal.digest)
    expect(proposal.digest).toMatch(/^[0-9a-f]{8}$/)
    expect(proposalDigest({
      target: proposal.target,
      changes: proposal.changes.map((change) => ({ ...change, to: "超大" })),
    })).not.toBe(proposal.digest)
  })

  it("轨迹节点状态覆盖 idle / 运行 / 等待 / 已应用 / 已拒绝 / 冲突 / 失败", () => {
    expect(traceNodeStatuses({ status: "idle", traceRevealed: 0 })).toEqual([
      "pending", "pending", "pending", "pending", "pending",
    ])
    expect(traceNodeStatuses({ status: "drafting", traceRevealed: 0 })).toEqual([
      "active", "pending", "pending", "pending", "pending",
    ])
    expect(traceNodeStatuses({ status: "drafting", traceRevealed: 2 })).toEqual([
      "complete", "complete", "active", "pending", "pending",
    ])
    expect(traceNodeStatuses({ status: "proposal", traceRevealed: 4 })).toEqual([
      "complete", "complete", "complete", "complete", "active",
    ])
    expect(traceNodeStatuses({ status: "applied", traceRevealed: 4 })).toEqual([
      "complete", "complete", "complete", "complete", "complete",
    ])
    expect(traceNodeStatuses({ status: "rejected", traceRevealed: 4 })).toEqual([
      "complete", "complete", "complete", "complete", "skipped",
    ])
    expect(traceNodeStatuses({ status: "conflict", traceRevealed: 4 })).toEqual([
      "complete", "complete", "complete", "complete", "conflict",
    ])
    // 失败不是冲突：提案生成失败落在提案节点，应用失败落在批准节点。
    expect(traceNodeStatuses({ status: "failed", traceRevealed: 3, failedStage: "proposal" })).toEqual([
      "complete", "complete", "complete", "failed", "pending",
    ])
    expect(traceNodeStatuses({ status: "failed", traceRevealed: 4, failedStage: "apply" })).toEqual([
      "complete", "complete", "complete", "complete", "failed",
    ])
    expect(traceNodeStatuses({ status: "failed", traceRevealed: 4, failedStage: null })).toEqual([
      "complete", "complete", "complete", "complete", "failed",
    ])
  })
})

describe("栖伴 Dialog 语义与分区导航", () => {
  it("使用 portal + role=dialog，默认只显示真实欢迎语，不预置用户对话", () => {
    renderDialog()

    const dialog = screen.getByRole("dialog")
    expect(dialog.getAttribute("aria-modal")).toBe("true")
    expect(dialog.parentElement?.parentElement).toBe(document.body)
    expect(screen.getByRole("heading", { name: "栖伴" })).toBeTruthy()

    expect(screen.getByText(/我是栖伴，栖阅的 Haven 智能体/)).toBeTruthy()
    expect(screen.queryByText("阅读时正文偏小，排版也有点挤。")).toBeNull()

    expect(screen.getByRole("tab", { name: "对话" }).getAttribute("aria-selected")).toBe("true")
    expect(screen.getByRole("tabpanel")).toBeTruthy()
  })

  it("用户可见名称统一为「栖伴」：说明里是 Haven 智能体，任何分区都不再出现旧称", () => {
    renderDialog()

    const dialog = screen.getByRole("dialog")
    // Dialog 说明把它定位成 Haven 智能体，而不是一个通用配置助手。
    expect(dialog.textContent).toContain("Haven 智能体")
    expect(dialog.textContent).not.toContain("配置助手")
    expect(screen.getByRole("button", { name: "关闭栖伴" })).toBeTruthy()
    expect(screen.getByRole("tablist", { name: "栖伴分区" })).toBeTruthy()

    // 提案记录空态同样使用新名称。
    fireEvent.click(screen.getByRole("tab", { name: "提案记录" }))
    expect(screen.getByText("还没有设置提案。回到对话，让栖伴先生成一份提案。")).toBeTruthy()
    expect(screen.queryByText(/配置助手/)).toBeNull()
  })

  it("分区导航是与内容区分隔的竖向 rail：上下方向键切换，左右方向键保持兼容", () => {
    renderDialog()

    const rail = screen.getByRole("tablist", { name: "栖伴分区" })
    expect(rail.getAttribute("aria-orientation")).toBe("vertical")
    expect(within(rail).getAllByRole("tab")).toHaveLength(3)
    // 桌面固定宽度左栏 + 右边框分隔，窄屏降级为横向分区条。
    expect(rail.className).toContain("sm:w-[172px]")
    expect(rail.className).toContain("sm:border-r")
    expect(rail.className).toContain("flex-col")
    // rail 与内容区是同级容器：tabpanel 不在 rail 内部。
    expect(rail.contains(screen.getByRole("tabpanel"))).toBe(false)

    fireEvent.keyDown(rail, { key: "ArrowDown" })
    const traceTab = screen.getByRole("tab", { name: "执行轨迹" })
    expect(traceTab.getAttribute("aria-selected")).toBe("true")
    expect(document.activeElement).toBe(traceTab)

    fireEvent.keyDown(rail, { key: "ArrowDown" })
    expect(screen.getByRole("tab", { name: "提案记录" }).getAttribute("aria-selected")).toBe("true")

    // 循环回到第一个分区。
    fireEvent.keyDown(rail, { key: "ArrowDown" })
    expect(screen.getByRole("tab", { name: "对话" }).getAttribute("aria-selected")).toBe("true")

    // 左右方向键为兼容方向，方向语义与上下一致。
    fireEvent.keyDown(rail, { key: "ArrowLeft" })
    expect(screen.getByRole("tab", { name: "提案记录" }).getAttribute("aria-selected")).toBe("true")
    fireEvent.keyDown(rail, { key: "ArrowUp" })
    expect(screen.getByRole("tab", { name: "执行轨迹" }).getAttribute("aria-selected")).toBe("true")
  })

  it("分区切换只在助手内部生效：执行轨迹显示 inspector，提案记录显示空态", () => {
    renderDialog()

    fireEvent.click(screen.getByRole("tab", { name: "执行轨迹" }))
    expect(screen.getByRole("tab", { name: "执行轨迹" }).getAttribute("aria-selected")).toBe("true")
    expect(screen.getByRole("list", { name: "执行轨迹节点" })).toBeTruthy()
    expect(screen.getByText("读取设备能力")).toBeTruthy()
    expect(screen.getByText("等待用户批准")).toBeTruthy()

    // 轨迹只包含只读步骤元数据，不出现本机路径、查询语句或密钥字段。
    const traceText = screen.getByRole("list", { name: "执行轨迹节点" }).textContent ?? ""
    expect(traceText).toMatch(/只读/)
    expect(traceText).not.toMatch(/[A-Za-z]:\\/)
    expect(traceText).not.toMatch(/SELECT |api[_-]?key/i)

    fireEvent.click(screen.getByRole("tab", { name: "提案记录" }))
    expect(screen.getByText("还没有设置提案。回到对话，让栖伴先生成一份提案。")).toBeTruthy()
  })

  it("Escape 与关闭按钮都能请求关闭", () => {
    const first = renderDialog()
    fireEvent.keyDown(document, { key: "Escape" })
    expect(first.onOpenChange).toHaveBeenCalledWith(false)
    cleanup()

    const second = renderDialog()
    fireEvent.click(screen.getByRole("button", { name: "关闭栖伴" }))
    expect(second.onOpenChange).toHaveBeenCalledWith(false)
  })

  it("open=false 时不渲染任何浮层", () => {
    render(<AiAssistantDialog open={false} onOpenChange={vi.fn()} />)
    expect(screen.queryByRole("dialog")).toBeNull()
  })
})

describe("栖伴提案状态机", () => {
  it("快捷操作进入 proposal 状态：三项改动、作用域、digest 与三个操作", async () => {
    renderDialog({ previewOnly: true })
    await openProposal()
    const region = proposalRegion()

    expect(within(region).getByText("阅读排版优化")).toBeTruthy()
    expect(within(region).getAllByRole("listitem")).toHaveLength(3)
    expect(within(region).getByText("正文字号")).toBeTruthy()
    expect(within(region).getByText("中")).toBeTruthy()
    expect(within(region).getByText("大")).toBeTruthy()
    expect(within(region).getByText(/作用域 全局默认 · digest [0-9a-f]{8}（本地预览）/)).toBeTruthy()
    expect(within(region).getByText("基线版本 未提供（本地预览）")).toBeTruthy()
    expect(within(region).getByText("待批准")).toBeTruthy()
    expect(within(region).getByRole("button", { name: "查看执行轨迹" })).toBeTruthy()
    expect(within(region).getByRole("button", { name: "拒绝" })).toBeTruthy()
    expect(within(region).getByRole("button", { name: "批准并应用" })).toBeTruthy()
  })

  it("输入框可以输入并提交，提交后同样进入 proposal 状态", async () => {
    renderDialog({ previewOnly: true })
    fireEvent.change(screen.getByLabelText("描述你想要的设置改动"), {
      target: { value: "把阅读排版调舒服一点" },
    })
    fireEvent.click(screen.getByRole("button", { name: "发送" }))

    expect(screen.getByText("把阅读排版调舒服一点")).toBeTruthy()
    await waitFor(() => expect(proposalRegion()).toBeTruthy())
  })

  it("前端按后端的 2000 字符上限阻止过长请求", () => {
    renderDialog()
    fireEvent.change(screen.getByLabelText("描述你想要的设置改动"), {
      target: { value: "字".repeat(2_001) },
    })

    expect(screen.getByText("最多 2000 个字符")).toBeTruthy()
    expect((screen.getByRole("button", { name: "发送" }) as HTMLButtonElement).disabled).toBe(true)
  })

  it("非阅读意图的输入只得到能力说明，不生成提案", async () => {
    renderDialog()
    fireEvent.change(screen.getByLabelText("描述你想要的设置改动"), { target: { value: "帮我配置下载并发" } })
    fireEvent.click(screen.getByRole("button", { name: "发送" }))

    expect(await screen.findByText(/当前阶段我只接入了阅读排版提案/)).toBeTruthy()
    expect(screen.queryByRole("region", { name: "设置提案" })).toBeNull()
    expect(screen.queryByRole("button", { name: "批准并应用" })).toBeNull()
  })

  it("轨迹在批准前停在运行/等待态，批准后才推进到完成", async () => {
    renderDialog({ previewOnly: true, traceStepMs: 100_000 })
    fireEvent.click(screen.getByRole("button", { name: "优化阅读" }))
    fireEvent.click(screen.getByRole("tab", { name: "执行轨迹" }))

    expect(screen.getByLabelText("读取设备能力：进行中")).toBeTruthy()
    expect(screen.getByLabelText("生成设置提案：待处理")).toBeTruthy()
    expect(screen.queryByRole("button", { name: "批准并应用" })).toBeNull()
    cleanup()

    renderDialog({ previewOnly: true })
    await openProposal()
    fireEvent.click(screen.getByRole("tab", { name: "执行轨迹" }))
    expect(screen.getByLabelText("读取设置快照：已完成")).toBeTruthy()
    expect(screen.getByLabelText("等待用户批准：进行中")).toBeTruthy()

    fireEvent.click(screen.getByRole("button", { name: "批准并应用" }))
    await waitFor(() => expect(screen.getByLabelText("用户批准并应用：已完成")).toBeTruthy())
  })

  it("拒绝是零写入：没有回执，也没有任何存储写入", async () => {
    const setItem = vi.spyOn(Storage.prototype, "setItem")
    renderDialog({ previewOnly: true })
    await openProposal()

    fireEvent.click(within(proposalRegion()).getByRole("button", { name: "拒绝" }))

    await waitFor(() => expect(within(proposalRegion()).getByText("已拒绝")).toBeTruthy())
    const region = proposalRegion()
    expect(within(region).getByText("本次没有写入任何设置。")).toBeTruthy()
    expect(within(region).queryByText("Setting Change Receipt")).toBeNull()
    expect(within(region).queryByRole("button", { name: "批准并应用" })).toBeNull()
    expect(screen.queryByText(/设置变更已应用/)).toBeNull()
    expect(setItem).not.toHaveBeenCalled()

    fireEvent.click(screen.getByRole("tab", { name: "执行轨迹" }))
    expect(screen.getByLabelText("用户已拒绝：已跳过")).toBeTruthy()
    expect(screen.getByText("已拒绝该提案，本次没有写入任何设置。")).toBeTruthy()
  })

  it("没有客户端时（纯预览）批准只展示带本地预览标注的 Setting Change Receipt", async () => {
    const setItem = vi.spyOn(Storage.prototype, "setItem")
    renderDialog({ previewOnly: true })
    await openProposal()

    fireEvent.click(within(proposalRegion()).getByRole("button", { name: "批准并应用" }))

    await waitFor(() => expect(within(proposalRegion()).getByText("Setting Change Receipt")).toBeTruthy())
    const region = proposalRegion()
    expect(within(region).getByText("预览完成（未写入）")).toBeTruthy()
    expect(within(region).getByText("3 项")).toBeTruthy()
    expect(within(region).getByText("local-preview")).toBeTruthy()
    expect(within(region).getByText(/Mock\/预览回执/)).toBeTruthy()
    expect(screen.getByText(/Haven 智能体 · 本地预览：尚未接入完整应用层，批准只会生成预览回执，不会写入任何设置。/)).toBeTruthy()
    // 预览回执不携带 receipt id 或领域 digest——不伪造应用层事实。
    expect(within(region).queryByText(/^回执 /)).toBeNull()
    expect(setItem).not.toHaveBeenCalled()

    fireEvent.click(screen.getByRole("tab", { name: "提案记录" }))
    expect(screen.getByText("已应用")).toBeTruthy()
    expect(screen.getByText(/全局默认 · 3 项改动 · digest [0-9a-f]{8}（本地预览）/)).toBeTruthy()
  })

  it("只有读取能力（settingsRead=false）时明确显示无可用模型，不生成伪造提案", async () => {
    const context = agentContext()
    const client = fakeClient({
      agentSettingsContextGet: vi.fn(async () => ({
        ...context,
        capabilities: {
          ...context.capabilities,
          capabilities: { ...context.capabilities.capabilities, settingsRead: false },
        },
      })),
    })
    renderDialog({ client })

    fireEvent.click(screen.getByRole("button", { name: "优化阅读" }))

    await waitFor(() => expect(screen.getByRole("alert", { name: "提案生成失败" })).toBeTruthy())
    const alert = screen.getByRole("alert", { name: "提案生成失败" })
    expect(alert.textContent).toContain("当前没有可用的模型服务")
    expect(screen.queryByRole("region", { name: "设置提案" })).toBeNull()
    expect(client.agentSettingsProposalCreate).not.toHaveBeenCalled()
  })

  it("接入 typed 客户端后：proposal 与 digest 由 IPC 提供，批准只提交提案 ID 与那份 digest", async () => {
    const client = fakeClient()
    renderDialog({ client })
    await openProposal()

    expect(client.agentSettingsContextGet).toHaveBeenCalledTimes(1)
    expect(client.agentSettingsProposalCreate).toHaveBeenCalledTimes(1)
    const createRequest = vi.mocked(client.agentSettingsProposalCreate).mock.calls[0][0]
    expect(createRequest.contextId).toBe(agentContext().contextId)
    expect(createRequest.contextHash).toBe(agentContext().contextHash)
    expect(createRequest.baseRevision).toBe("rev-41")

    const region = proposalRegion()
    expect(within(region).getByText(`作用域 全局默认 · 阅读 · digest ${PROPOSAL_DIGEST}`)).toBeTruthy()
    expect(within(region).getByText("基线版本 rev-41")).toBeTruthy()
    expect(within(region).queryByText(/本地预览/)).toBeNull()
    // 真实客户端不带 Mock 标识。
    expect(within(region).getByText("固定建议 · 非模型生成")).toBeTruthy()

    fireEvent.click(within(region).getByRole("button", { name: "批准并应用" }))

    await waitFor(() => expect(client.agentSettingsProposalApprove).toHaveBeenCalledTimes(1))
    const approveRequest = vi.mocked(client.agentSettingsProposalApprove).mock.calls[0][0]
    // 只提交提案 ID 与 UI 显示的那份 digest；没有 token 字段。
    expect(approveRequest).toEqual({ proposalId: PROPOSAL_ID, expectedDigest: PROPOSAL_DIGEST })
    expect(Object.keys(approveRequest).sort()).toEqual(["expectedDigest", "proposalId"])

    await waitFor(() => expect(within(proposalRegion()).getByText("rev-42")).toBeTruthy())
    expect(within(proposalRegion()).getByText(`回执 ${RECEIPT_ID} · digest ${PROPOSAL_DIGEST}`)).toBeTruthy()
    expect(within(proposalRegion()).queryByText(/Mock\/预览回执/)).toBeNull()
    expect(screen.getByText("设置变更已应用，回执如下。")).toBeTruthy()
  })

  it("拒绝同样只提交提案 ID 与显示的那份 digest", async () => {
    const client = fakeClient()
    renderDialog({ client })
    await openProposal()

    fireEvent.click(within(proposalRegion()).getByRole("button", { name: "拒绝" }))

    await waitFor(() => expect(client.agentSettingsProposalReject).toHaveBeenCalledTimes(1))
    expect(vi.mocked(client.agentSettingsProposalReject).mock.calls[0][0]).toEqual({
      proposalId: PROPOSAL_ID,
      expectedDigest: PROPOSAL_DIGEST,
    })
    await waitFor(() => expect(within(proposalRegion()).getByText("已拒绝")).toBeTruthy())
    expect(within(proposalRegion()).getByText("本次没有写入任何设置。")).toBeTruthy()
  })

  it("点击旧提案的拒绝只关闭旧卡片，最新提案与轨迹保持待批准", async () => {
    const client = sequencedClient()
    const [oldRegion, latestRegion] = await openTwoProposals(client)

    fireEvent.click(within(oldRegion).getByRole("button", { name: "拒绝" }))

    await waitFor(() => expect(within(oldRegion).getByText("已拒绝")).toBeTruthy())
    expect(within(oldRegion).getByText("本次没有写入任何设置。")).toBeTruthy()
    expect(within(oldRegion).queryByRole("button", { name: "批准并应用" })).toBeNull()
    expect(within(latestRegion).getByText("待批准")).toBeTruthy()
    expect(vi.mocked(client.agentSettingsProposalReject).mock.calls[0][0]).toEqual({
      proposalId: PROPOSAL_ID,
      expectedDigest: PROPOSAL_DIGEST,
    })

    fireEvent.click(screen.getByRole("tab", { name: "执行轨迹" }))
    expect(screen.getByLabelText("等待用户批准：进行中")).toBeTruthy()
  })

  it("点击旧提案的批准只应用旧卡片，不能把最新提案轨迹标成已完成", async () => {
    const client = sequencedClient()
    const [oldRegion, latestRegion] = await openTwoProposals(client)

    fireEvent.click(within(oldRegion).getByRole("button", { name: "批准并应用" }))

    await waitFor(() => expect(within(oldRegion).getAllByText("已应用").length).toBeGreaterThan(0))
    expect(within(oldRegion).getByText("Setting Change Receipt")).toBeTruthy()
    expect(within(latestRegion).getByText("待批准")).toBeTruthy()
    expect(vi.mocked(client.agentSettingsProposalApprove).mock.calls[0][0]).toEqual({
      proposalId: PROPOSAL_ID,
      expectedDigest: PROPOSAL_DIGEST,
    })

    fireEvent.click(screen.getByRole("tab", { name: "执行轨迹" }))
    expect(screen.getByLabelText("等待用户批准：进行中")).toBeTruthy()
    expect(screen.queryByLabelText("用户批准并应用：已完成")).toBeNull()
  })

  it("拒绝回调失败时提案仍是 pending，按钮保留且可以重试", async () => {
    const reject = vi.fn(async (_request: AgentSettingsProposalRejectRequest) => {
      throw new HavenError({
        code: "AGENT_GATEWAY_UNAVAILABLE",
        userMessage: "拒绝服务暂时不可用",
        retryable: true,
      })
    })
    const client = fakeClient({ agentSettingsProposalReject: reject })
    renderDialog({ client })
    await openProposal()

    const region = proposalRegion()
    fireEvent.click(within(region).getByRole("button", { name: "拒绝" }))

    await waitFor(() => expect(within(region).getByRole("alert").textContent).toContain("拒绝失败"))
    expect(within(region).getByText("待批准")).toBeTruthy()
    expect(within(region).getByText("本次没有写入任何设置，提案仍待批准，可以重试。")).toBeTruthy()
    expect((within(region).getByRole("button", { name: "拒绝" }) as HTMLButtonElement).disabled).toBe(false)
    expect((within(region).getByRole("button", { name: "批准并应用" }) as HTMLButtonElement).disabled).toBe(false)

    fireEvent.click(within(region).getByRole("button", { name: "拒绝" }))
    await waitFor(() => expect(reject).toHaveBeenCalledTimes(2))
    expect(within(region).getByText("待批准")).toBeTruthy()
  })

  it("提案生成后客户端被移除：批准被拦下，不会把 IPC 提案当预览处理", async () => {
    const client = fakeClient()
    const view = renderDialog({ client })
    await openProposal()

    // 提案已经生成后才失去客户端：以 proposal record 的来源标记为准再次防护。
    view.rerender(<AiAssistantDialog open traceStepMs={0} previewOnly onOpenChange={vi.fn()} />)

    fireEvent.click(within(proposalRegion()).getByRole("button", { name: "批准并应用" }))

    await waitFor(() => expect(screen.getByRole("alert", { name: "应用失败" })).toBeTruthy())
    expect(client.agentSettingsProposalApprove).not.toHaveBeenCalled()
    expect(within(proposalRegion()).queryByText("Setting Change Receipt")).toBeNull()
    expect(screen.getByRole("alert", { name: "应用失败" }).textContent).toContain("应用层不可用，未确认写入结果。")
  })

  it("提案生成失败时显示失败节点与业务提示，不回退成冲突或等待批准", async () => {
    const client = fakeClient({
      agentSettingsContextGet: vi.fn(async () => {
        throw new Error("snapshot unavailable")
      }),
    })
    renderDialog({ client })

    fireEvent.click(screen.getByRole("button", { name: "优化阅读" }))

    await waitFor(() => expect(screen.getByRole("alert", { name: "提案生成失败" })).toBeTruthy())
    const alert = screen.getByRole("alert", { name: "提案生成失败" })
    expect(alert.textContent).toContain("读取本机设置失败，未生成提案。")
    expect(alert.textContent).toContain("本次没有写入任何设置")
    expect(screen.queryByRole("region", { name: "设置提案" })).toBeNull()
    expect(client.agentSettingsProposalCreate).not.toHaveBeenCalled()
    expect(client.agentSettingsProposalApprove).not.toHaveBeenCalled()

    fireEvent.click(screen.getByRole("tab", { name: "执行轨迹" }))
    expect(screen.getByLabelText("生成设置提案：失败")).toBeTruthy()
    expect(screen.getByLabelText("未进入批准：待处理")).toBeTruthy()
    expect(screen.queryByLabelText(/需要处理/)).toBeNull()
    expect(screen.queryByText("等待用户批准")).toBeNull()
  })

  it("Revision Conflict 在 Dialog 内显示业务消息，且不产生回执", async () => {
    const client = fakeClient({
      agentSettingsProposalApprove: vi.fn(async () => {
        throw new HavenError({
          code: "REVISION_CONFLICT",
          userMessage: "设置版本已变化，请重新生成提案。",
          retryable: false,
        })
      }),
    })
    renderDialog({ client })
    await openProposal()

    fireEvent.click(within(proposalRegion()).getByRole("button", { name: "批准并应用" }))

    await waitFor(() => expect(within(proposalRegion()).getByText("版本冲突")).toBeTruthy())
    const alert = screen.getByRole("alert", { name: "版本冲突" })
    expect(alert.textContent).toContain("设置版本已变化，请重新生成提案。")
    expect(alert.textContent).toContain("本次没有写入任何设置")
    expect(within(proposalRegion()).getByRole("alert").textContent).toContain("本次没有写入任何设置")
    expect(within(proposalRegion()).queryByText("Setting Change Receipt")).toBeNull()
    expect(screen.getByText("设置版本在批准前发生变化，本次没有写入任何设置。")).toBeTruthy()

    fireEvent.click(screen.getByRole("tab", { name: "执行轨迹" }))
    expect(screen.getByLabelText("版本冲突：需要处理")).toBeTruthy()
    expect(screen.getByLabelText("生成设置提案：已完成")).toBeTruthy()
    expect(screen.getByText("设置版本已变化，本次没有写入任何设置。")).toBeTruthy()
    expect(screen.queryByLabelText(/：失败/)).toBeNull()
  })

  it("裸 ErrorDto 的 REVISION_CONFLICT 也按确定零写入冲突处理", async () => {
    const client = fakeClient({
      agentSettingsProposalApprove: vi.fn(async () => {
        // 模拟 Tauri/网关边界直接抛出的 ErrorDto，而不是 HavenError 实例。
        throw {
          code: "REVISION_CONFLICT",
          userMessage: "裸 DTO：设置版本已经变化。",
          retryable: false,
        }
      }),
    })
    renderDialog({ client })
    await openProposal()

    fireEvent.click(within(proposalRegion()).getByRole("button", { name: "批准并应用" }))

    await waitFor(() => expect(screen.getByRole("alert", { name: "版本冲突" })).toBeTruthy())
    expect(screen.getByRole("alert", { name: "版本冲突" }).textContent).toContain("裸 DTO：设置版本已经变化")
    expect(screen.getByRole("alert", { name: "版本冲突" }).textContent).toContain("本次没有写入任何设置")
    expect(within(proposalRegion()).queryByText("Setting Change Receipt")).toBeNull()
  })

  it("提案已过期时显示独立的过期零写入结论", async () => {
    const client = fakeClient({
      agentSettingsProposalApprove: vi.fn(async () => {
        throw new HavenError({
          code: "SETTING_PROPOSAL_EXPIRED",
          userMessage: "提案已过期，未执行任何修改",
          retryable: false,
        })
      }),
    })
    renderDialog({ client })
    await openProposal()

    fireEvent.click(within(proposalRegion()).getByRole("button", { name: "批准并应用" }))

    await waitFor(() => expect(within(proposalRegion()).getByText("已过期")).toBeTruthy())
    expect(within(proposalRegion()).getByText("提案已过期，本次没有写入任何设置。请重新生成提案。")).toBeTruthy()
    expect(within(proposalRegion()).queryByText("Setting Change Receipt")).toBeNull()
    fireEvent.click(screen.getByRole("tab", { name: "执行轨迹" }))
    expect(screen.getByLabelText("提案已过期：已过期")).toBeTruthy()
  })

  it("批准失败显示应用失败：未确认写入结果，不谎报成功", async () => {
    const client = fakeClient({
      agentSettingsProposalApprove: vi.fn(async () => {
        throw new Error("gateway unavailable")
      }),
    })
    renderDialog({ client })
    await openProposal()

    fireEvent.click(within(proposalRegion()).getByRole("button", { name: "批准并应用" }))

    await waitFor(() => expect(within(proposalRegion()).getByText("应用失败")).toBeTruthy())
    const alert = screen.getByRole("alert", { name: "应用失败" })
    expect(alert.textContent).toContain("应用失败，未确认写入结果。")
    expect(alert.textContent).toContain("未确认写入结果")
    expect(within(proposalRegion()).queryByText("Setting Change Receipt")).toBeNull()
    expect(screen.getByText("应用失败，请重新读取设置确认当前状态。")).toBeTruthy()

    fireEvent.click(screen.getByRole("tab", { name: "执行轨迹" }))
    expect(screen.getByLabelText("应用失败：失败")).toBeTruthy()
    expect(screen.getByText("应用层回调失败，未确认写入结果。")).toBeTruthy()
    expect(screen.queryByLabelText(/需要处理/)).toBeNull()
    expect(screen.queryByText("等待用户批准")).toBeNull()
  })

  it("Mock 客户端全流程带 Mock/预览标识，不冒充真实模型输出", async () => {
    // 动态引入，避免用例顶层依赖 mock-client 的 fixture 载入顺序。
    const { MockHavenClient } = await import("@/lib/ipc/mock-client")
    const client = new MockHavenClient()
    renderDialog({ client })
    await openProposal()

    const region = proposalRegion()
    expect(within(region).getByText("Mock/预览 · 非真实模型输出")).toBeTruthy()
    // digest 形状与领域摘要一致（64 位小写十六进制），但标注为 Mock。
    expect(within(region).getByText(new RegExp(`digest [0-9a-f]{64}（Mock/预览）`))).toBeTruthy()

    fireEvent.click(within(region).getByRole("button", { name: "批准并应用" }))

    await waitFor(() => expect(within(proposalRegion()).getByText("Setting Change Receipt")).toBeTruthy())
    expect(within(proposalRegion()).getByText(/Mock\/预览回执/)).toBeTruthy()
    // 回执带 receipt id 与同一份 digest（Mock 的确定性实现）。
    expect(within(proposalRegion()).getByText(/^回执 mock-receipt-/)).toBeTruthy()
  })
})

describe("设置页 AI 工作台导航与设置入口", () => {
  function renderSection(section: string) {
    return render(
      <MemoryRouter initialEntries={[`/settings/${section}`]}>
        <Routes>
          <Route path="/settings/:section" element={<SettingsPage />} />
        </Routes>
      </MemoryRouter>,
    )
  }

  it("导航提供 AI 工作台，但不展示 Registry 未登记的同步入口", () => {
    renderSection("overview")

    const nav = screen.getByRole("navigation", { name: "设置分类" })
    expect(nav.textContent).toContain("AI 工作台")
    expect(nav.textContent).toContain("智能")
    expect(within(nav).queryByRole("button", { name: /同步与备份/ })).toBeNull()
    expect(nav.textContent).not.toContain("规划中")
  })

  it("AI 工作台是主视图，齿轮入口打开 Provider 与 Agent 设置", async () => {
    renderSection("ai")

    expect(await screen.findByRole("button", { name: "新会话" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "提案历史" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "执行记录" })).toBeTruthy()
    expect(screen.queryByRole("dialog")).toBeNull()

    fireEvent.click(screen.getByRole("button", { name: "AI 设置" }))
    expect(await screen.findByRole("dialog", { name: "AI 设置" })).toBeTruthy()
    expect(screen.getByText("API 连接")).toBeTruthy()
    expect(screen.getByText("本机内置技能")).toBeTruthy()
  })

  it("停用的分区路由回落到总览，而不是渲染占位页", () => {
    renderSection("sync")

    // 总览既出现在左侧导航，也是正文标题；两条都说明这一页真的是总览。
    expect(screen.getAllByText("总览").length).toBeGreaterThan(1)
    expect(document.body.textContent).not.toContain("当前版本不可用")
    expect(document.body.textContent).not.toContain("尚未接入")
  })

  it("阅读分区保留全局工作台导航，但不显示工作台工具栏或浮层", () => {
    renderSection("reading")

    const nav = screen.getByRole("navigation", { name: "设置分类" })
    expect(nav.textContent).toContain("AI 工作台")
    expect(screen.queryByRole("button", { name: "新会话" })).toBeNull()
    expect(screen.queryByRole("button", { name: "AI 设置" })).toBeNull()
    expect(screen.queryByRole("dialog")).toBeNull()
  })

  it("总览分区同样没有栖伴入口或浮层", () => {
    renderSection("overview")
    expect(screen.queryByRole("button", { name: "新会话" })).toBeNull()
    expect(screen.queryByRole("dialog")).toBeNull()
  })
})

describe("设置页智能功能分区的模型空态", () => {
  // 这些用例保持**只读**：它们断言的是「没有任何 Provider 配置」时的诚实空态。
  // 写入路径（新建配置、单向提交 API Key）由 `pages/SettingsPage.ai-provider.test.tsx`
  // 覆盖 —— 那里有独立的模块注册表，不会与这里的 Mock 单例状态互相污染。
  function renderAiSettings() {
    const view = render(
      <MemoryRouter initialEntries={["/settings/ai"]}>
        <Routes>
          <Route path="/settings/:section" element={<SettingsPage />} />
        </Routes>
      </MemoryRouter>,
    )
    fireEvent.click(screen.getByRole("button", { name: "AI 设置" }))
    return view
  }

  it("默认模型只显示「无可用模型」，对应选择控件禁用", async () => {
    renderAiSettings()

    const trigger = (await screen.findByRole("button", { name: "默认模型" })) as HTMLButtonElement
    await waitFor(() => expect(trigger.textContent).toContain("无可用模型"))
    expect(trigger.disabled).toBe(true)
    // 默认模型与识图模型两处空态都必须显示，而不是只留一个空白控件。
    expect(screen.getAllByText("无可用模型").length).toBeGreaterThanOrEqual(2)
  })

  it("禁用的模型选择打不开菜单，示例模型名不会变成可选项", async () => {
    renderAiSettings()

    const trigger = (await screen.findByRole("button", { name: "默认模型" })) as HTMLButtonElement
    await waitFor(() => expect(trigger.disabled).toBe(true))

    fireEvent.click(trigger)
    expect(screen.queryByRole("listbox")).toBeNull()
  })

  it("不再预置示例 API 地址，没有配置时不提供可编辑的地址输入", async () => {
    renderAiSettings()

    // 旧的占位默认值 `https://api.example.com/v1` 不得再出现在界面上。
    expect(await screen.findByRole("button", { name: "新建配置" })).toBeTruthy()
    expect(document.body.textContent).not.toContain("api.example.com")
    expect(screen.queryByLabelText("API 地址")).toBeNull()
    // 没有 profile 就没有可写的凭据目标：API Key 行整体不渲染。
    expect(screen.queryByLabelText("API Key")).toBeNull()
    expect(screen.queryByText("已配置")).toBeNull()
  })
})

/**
 * 真实 Provider 路径（原生 Skill 切片）。
 *
 * 这一组用例的存在理由只有一个：证明「栖伴会把你输入的目标交给**你配置的**
 * Provider」，而不是用页面侧的确定性模板冒充模型输出。
 *
 * 三条必须同时成立，缺一条这个切片就没有落地：
 * 1. 用户原话、选中的 Provider 配置、设置上下文锚点逐字进入请求；
 * 2. 没有选中配置时**如实失败**，不退回模板；
 * 3. Mock / 纯预览路径永远不碰真实 Provider。
 */
describe("栖伴真实 Provider 路径", () => {
  const PROFILE_ID = "gw-primary"
  const MODEL_ID = "reader-chat-1"

  /** 后端 `AiSettingsRecommendationDto` 的最小自洽投影。 */
  function recommendationDto() {
    return {
      schemaVersion: 1 as const,
      profileId: PROFILE_ID,
      modelId: MODEL_ID,
      explanation: null,
      recommendedPatch: { fontSize: "large" as const },
      proposal: proposalDto(),
    }
  }

  /**
   * 记录每一次真实 Provider 请求。
   *
   * 参数类型直接用生成物里的 `AiSettingsRecommendationGenerateRequest`（已含 `userIntent`）：
   * mock 只把它原样收下来，断言时再按实际载荷取值。
   */
  function providerClient() {
    const seen: Array<Record<string, unknown>> = []
    const generate = vi.fn(async (request: AiSettingsRecommendationGenerateRequest) => {
      seen.push(request as unknown as Record<string, unknown>)
      return recommendationDto()
    })
    const client = fakeClient({
      aiSettingsRecommendationGenerate:
        generate as unknown as AiAssistantAgentClient["aiSettingsRecommendationGenerate"],
    })
    return { client, generate, seen }
  }

  it("用户原话、选中的 Provider 与上下文锚点原样进入请求，且不使用模板", async () => {
    const { client, generate, seen } = providerClient()
    renderDialog({ client, profileId: PROFILE_ID })

    const userIntent = "把正文字号调大一级，不要改动其他项目"
    fireEvent.change(screen.getByLabelText("描述你想要的设置改动"), { target: { value: userIntent } })
    fireEvent.click(screen.getByRole("button", { name: "发送" }))
    await screen.findByRole("button", { name: "批准并应用" })

    expect(generate).toHaveBeenCalledTimes(1)
    expect(seen).toHaveLength(1)
    // 用户实际提交的那句话，而不是任何模板常量。
    expect(seen[0].userIntent).toBe(userIntent)
    expect(seen[0].profileId).toBe(PROFILE_ID)
    // 锚点逐字来自最近一次上下文读取：改一个字符服务端就会 fail-closed。
    const context = agentContext()
    expect(seen[0].contextId).toBe(context.contextId)
    expect(seen[0].contextHash).toBe(context.contextHash)
    expect(seen[0].baseRevision).toBe(context.revision)
    // 确定性模板的唯一入口是 agentSettingsProposalCreate；这里必须一次都没走。
    expect(client.agentSettingsProposalCreate).not.toHaveBeenCalled()
  })

  it("真实路径的卡片标明实际调用的模型，digest 仍由服务端给出", async () => {
    const { client } = providerClient()
    renderDialog({ client, profileId: PROFILE_ID })

    await openProposal()

    const region = proposalRegion()
    // 模型名是「这份建议确实来自模型」的可核对证据，不是装饰。
    expect(within(region).getByText(`模型 ${MODEL_ID}`)).toBeTruthy()
    expect(within(region).getByText(new RegExp(PROPOSAL_DIGEST))).toBeTruthy()
    // 真实路径不得出现任何 Mock/预览 标识。
    expect(within(region).queryByText(/Mock\/预览/)).toBeNull()
    expect(within(region).queryByText(/本地预览/)).toBeNull()

    // 批准只提交提案 ID 与界面显示的那份 digest。
    fireEvent.click(within(region).getByRole("button", { name: "批准并应用" }))
    await waitFor(() => expect(client.agentSettingsProposalApprove).toHaveBeenCalledTimes(1))
    expect(client.agentSettingsProposalApprove).toHaveBeenCalledWith({
      proposalId: PROPOSAL_ID,
      expectedDigest: PROPOSAL_DIGEST,
    })
  })

  it("没有选中 Provider 配置时如实失败，既不调用模型也不退回模板", async () => {
    const { client, generate } = providerClient()
    // 不传 profileId：设置页里还没有选中任何配置。
    renderDialog({ client })

    fireEvent.click(screen.getByRole("button", { name: "优化阅读" }))

    const alert = await screen.findByRole("alert", { name: "提案生成失败" })
    expect(alert.textContent).toContain("尚未选择 AI 服务配置")
    expect(generate).not.toHaveBeenCalled()
    expect(client.agentSettingsProposalCreate).not.toHaveBeenCalled()
    expect(screen.queryByRole("region", { name: "设置提案" })).toBeNull()
  })

  it("畸形建议载荷 fail closed：既不生成提案，也不把它渲染成模型输出", async () => {
    // 对话框接受的是**结构化**客户端类型（`AiAssistantAgentClient`），注入的实现、
    // 测试替身或将来另一个 HavenClient 都可能返回畸形载荷。回归点：跨 Provider 边界的
    // 返回值必须在**消费点**再守一次，而不是只信 TypeScript 的返回类型断言。
    for (const [label, override] of [
      // 少了 modelId：无法证明这份建议来自哪次模型调用。
      ["missing-model", { modelId: undefined }],
      // 声明是另一个配置返回的：把它当成"本次调用结果"会张冠李戴。
      ["profile-mismatch", { profileId: "another-profile" }],
      // 提案不是 pending：渲染出去会让人以为还有一条待批准提案。
      ["not-pending", { proposal: { ...proposalDto(), status: "applied" as const } }],
    ] as const) {
      const generate = vi.fn(async () => ({ ...recommendationDto(), ...override }))
      const client = fakeClient({
        aiSettingsRecommendationGenerate:
          generate as unknown as AiAssistantAgentClient["aiSettingsRecommendationGenerate"],
      })
      renderDialog({ client, profileId: PROFILE_ID })

      fireEvent.click(screen.getByRole("button", { name: "优化阅读" }))

      const alert = await screen.findByRole("alert", { name: "提案生成失败" })
      expect(alert.textContent, label).toContain("不符合约定格式")
      // 也不得退回确定性模板：那会让畸形载荷看起来像一次成功的生成。
      expect(client.agentSettingsProposalCreate, label).not.toHaveBeenCalled()
      expect(screen.queryByRole("region", { name: "设置提案" }), label).toBeNull()
      cleanup()
    }
  })

  it("同一轮里切换选中的 Provider 不会重复发请求，结果仍归这一轮", async () => {
    // 回归点：请求在途时设置页把选中配置换掉（用户点了另一个 Provider），effect 的依赖
    // （`profileId`）会变化并重跑。没有 `issuedTurnRef` 那道闸门的话，同一次用户输入会
    // 生成两份请求——后端可能因此落下两条 pending 提案，而用户只按了一次发送。
    const pending = deferred<ReturnType<typeof recommendationDto>>()
    const seen: Array<Record<string, unknown>> = []
    const generate = vi.fn(async (request: AiSettingsRecommendationGenerateRequest) => {
      seen.push(request as unknown as Record<string, unknown>)
      return await pending.promise
    })
    const client = fakeClient({
      aiSettingsRecommendationGenerate:
        generate as unknown as AiAssistantAgentClient["aiSettingsRecommendationGenerate"],
    })
    const { rerender, onOpenChange } = renderDialog({ client, profileId: PROFILE_ID })

    fireEvent.click(screen.getByRole("button", { name: "优化阅读" }))
    await waitFor(() => expect(generate).toHaveBeenCalledTimes(1))

    rerender(
      <AiAssistantDialog
        open
        traceStepMs={0}
        client={client}
        profileId="gw-other"
        onOpenChange={onOpenChange}
      />,
    )
    // 切配置本身不得触发第二次请求。
    expect(generate).toHaveBeenCalledTimes(1)

    await act(async () => {
      pending.resolve(recommendationDto())
    })

    expect(generate).toHaveBeenCalledTimes(1)
    expect(seen).toHaveLength(1)
    // 请求发的是**当时**选中的那个配置；结果也照旧归这一轮，而不是被丢弃。
    expect(seen[0].profileId).toBe(PROFILE_ID)
    expect(await screen.findByRole("button", { name: "批准并应用" })).toBeTruthy()
    expect(client.agentSettingsProposalCreate).not.toHaveBeenCalled()
  })

  it("请求在途时关闭浮层不会重发请求，结果照旧被记录", async () => {
    // 回归点：关闭对话框**不是**取消。浮层只是不再渲染（组件仍然挂载，钩子照常运行），
    // 因此：
    // - 在途命令不得因为"界面关掉了"而被再发一次（那会落下第二条 pending 提案，而用户
    //   只按了一次发送）；
    // - 它的结果也不能被丢弃——重新打开时这一轮必须还在，界面从不声称一条已经发出的
    //   命令"被取消了"。
    const pending = deferred<ReturnType<typeof recommendationDto>>()
    const generate = vi.fn(async () => await pending.promise)
    const client = fakeClient({
      aiSettingsRecommendationGenerate:
        generate as unknown as AiAssistantAgentClient["aiSettingsRecommendationGenerate"],
    })
    const { rerender, onOpenChange } = renderDialog({ client, profileId: PROFILE_ID })

    fireEvent.click(screen.getByRole("button", { name: "优化阅读" }))
    await waitFor(() => expect(generate).toHaveBeenCalledTimes(1))

    rerender(
      <AiAssistantDialog
        open={false}
        traceStepMs={0}
        client={client}
        profileId={PROFILE_ID}
        onOpenChange={onOpenChange}
      />,
    )
    // 关闭本身不得触发第二次请求，也不得凭空产生一条提案。
    expect(generate).toHaveBeenCalledTimes(1)
    expect(client.agentSettingsProposalCreate).not.toHaveBeenCalled()

    await act(async () => {
      pending.resolve(recommendationDto())
    })
    expect(generate).toHaveBeenCalledTimes(1)
    expect(client.agentSettingsProposalCreate).not.toHaveBeenCalled()

    // 重新打开：这一轮的结果还在（既没有被丢弃，也没有变成第二次生成）。
    rerender(
      <AiAssistantDialog
        open
        traceStepMs={0}
        client={client}
        profileId={PROFILE_ID}
        onOpenChange={onOpenChange}
      />,
    )
    expect(await screen.findByRole("button", { name: "批准并应用" })).toBeTruthy()
    expect(proposalRegion()).toBeTruthy()
  })

  it("不构成阅读排版意图的输入不会触达 Provider", async () => {
    const { client, generate } = providerClient()
    renderDialog({ client, profileId: PROFILE_ID })

    fireEvent.change(screen.getByLabelText("描述你想要的设置改动"), {
      target: { value: "今天天气怎么样" },
    })
    fireEvent.click(screen.getByRole("button", { name: "发送" }))

    await screen.findByText(/我只接入了阅读排版提案/)
    expect(generate).not.toHaveBeenCalled()
  })

  it("Mock 客户端不调用真实 Provider 路径，模板结果必须带 Mock/预览 标识", async () => {
    const { MockHavenClient } = await import("@/lib/ipc/mock-client")
    const mock = new MockHavenClient()
    // Mock 实现了该方法但刻意抛错（拒绝伪造 Provider）；栖伴必须绕开它。
    const generate = vi.spyOn(mock, "aiSettingsRecommendationGenerate")
    renderDialog({ client: mock, profileId: PROFILE_ID })

    await openProposal()

    expect(generate).not.toHaveBeenCalled()
    expect(within(proposalRegion()).getByText(/Mock\/预览 · 非真实模型输出/)).toBeTruthy()
    // 模板没有模型参与，因此不显示任何模型行。
    expect(within(proposalRegion()).queryByText(/^模型 /)).toBeNull()
  })
})

describe("正式嵌入式 AI 工作台", () => {
  it("设置入口调用真实回调，历史和执行记录均可返回对话，不提供隐藏全局导航", () => {
    const openSettings = vi.fn()
    renderDialog({ embedded: true, previewOnly: true, onOpenSettings: openSettings })
    expect(screen.getByRole("heading", { name: "AI 工作台" })).toBeTruthy()
    expect(screen.getByText("本地预览", { exact: true })).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: "AI 设置" }))
    expect(openSettings).toHaveBeenCalledTimes(1)
    fireEvent.click(screen.getByRole("button", { name: "提案历史" }))
    expect(screen.getByRole("button", { name: "提案历史" }).getAttribute("aria-pressed")).toBe("true")
    fireEvent.click(screen.getByRole("button", { name: "返回对话" }))
    expect(screen.getByLabelText("描述你想要的设置改动")).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: "执行记录" }))
    expect(screen.getByRole("button", { name: "返回对话" })).toBeTruthy()
    expect(screen.queryByRole("button", { name: /收起.*导航|隐藏.*侧栏/ })).toBeNull()
  })

  it("无设置回调时不展示可点击的死入口", () => {
    renderDialog({ embedded: true, previewOnly: true })
    expect((screen.getByRole("button", { name: "AI 设置" }) as HTMLButtonElement).disabled).toBe(true)
  })

  it("Shift+Enter 与中文输入法确认不提交，Enter 发送后清空草稿", async () => {
    renderDialog({ embedded: true, previewOnly: true })
    const input = screen.getByLabelText("描述你想要的设置改动") as HTMLTextAreaElement
    fireEvent.change(input, { target: { value: "优化阅读" } })
    fireEvent.keyDown(input, { key: "Enter", shiftKey: true })
    expect(input.value).toBe("优化阅读")
    expect(screen.queryByRole("region", { name: "设置提案" })).toBeNull()
    fireEvent.keyDown(input, { key: "Enter", isComposing: true })
    expect(input.value).toBe("优化阅读")
    expect(screen.queryByRole("region", { name: "设置提案" })).toBeNull()
    fireEvent.keyDown(input, { key: "Enter" })
    expect(input.value).toBe("")
    expect(await screen.findByRole("region", { name: "设置提案" })).toBeTruthy()
  })

  it("超长输入有可见提示，禁用发送且不丢失草稿", () => {
    renderDialog({ embedded: true, previewOnly: true })
    const input = screen.getByLabelText("描述你想要的设置改动") as HTMLTextAreaElement
    const long = "阅".repeat(2001)
    fireEvent.change(input, { target: { value: long } })
    expect(screen.getByText("最多 2000 个字符").className).not.toContain("sr-only")
    expect((screen.getByRole("button", { name: "发送" }) as HTMLButtonElement).disabled).toBe(true)
    fireEvent.keyDown(input, { key: "Enter" })
    expect(input.value).toBe(long)
    expect(screen.queryByRole("region", { name: "设置提案" })).toBeNull()
  })
})
