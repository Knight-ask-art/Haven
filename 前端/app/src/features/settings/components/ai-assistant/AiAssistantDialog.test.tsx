// @vitest-environment jsdom
//
// 栖伴（Haven 智能体，FE-AI-ASSISTANT-001）测试：
// - 只覆盖本次白名单内的组件，不修改既有测试。
// - 用户可见名称是「栖伴」；组件名/文件名 ai-assistant / AiAssistantDialog 保持不变。
// - 安全边界：组件不发起网络请求、不写 Settings、不写 localStorage、不调用 Tauri；
//   只有读写回调同时提供时才走真实应用层，否则一律本地预览，
//   本地预览草稿永远不会进入写入回调。

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { MemoryRouter, Route, Routes } from "react-router"
import { afterEach, describe, expect, it, vi } from "vitest"

import { SettingsPage } from "../../pages/SettingsPage"
import { HavenError } from "@/lib/ipc/errors"
import type {
  AgentSettingsProposalApproveRequest,
  AgentSettingsProposalCreateRequest,
  AgentSettingsProposalRejectRequest,
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

// 「智能功能」分区的模型空态回归：当前没有真实的 Provider 模型发现/能力列表，
// 默认模型与识图模型都只能显示「无可用模型」并保持禁用，不得出现示例模型名。
vi.mock("@/lib/ipc/runtime", () => ({
  getHavenClientMode: () => "mock",
  getHavenClient: () => {
    throw new Error("AI 分区不读取 IPC；本用例只覆盖模型选择控件的空态")
  },
  isTauriRuntime: () => false,
}))
// 分区表单与模型空态无关：只保留一个空控制器，避免用例依赖各分区快照形状。
vi.mock("@/features/settings/lib/useSettingsForm", () => ({
  useSettingsForm: () => ({
    section: "general",
    state: { status: "loading" },
    displayValue: undefined,
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
  it("使用 portal + role=dialog，默认展示对话与预置短对话", () => {
    renderDialog()

    const dialog = screen.getByRole("dialog")
    expect(dialog.getAttribute("aria-modal")).toBe("true")
    expect(dialog.parentElement?.parentElement).toBe(document.body)
    expect(screen.getByRole("heading", { name: "栖伴" })).toBeTruthy()

    expect(screen.getByText(/我是栖伴，栖阅的 Haven 智能体/)).toBeTruthy()
    expect(screen.getByText("阅读时正文偏小，排版也有点挤。")).toBeTruthy()

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
    renderDialog()
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
    renderDialog()
    fireEvent.change(screen.getByLabelText("描述你想要的设置改动"), {
      target: { value: "把阅读排版调舒服一点" },
    })
    fireEvent.click(screen.getByRole("button", { name: "发送" }))

    expect(screen.getByText("把阅读排版调舒服一点")).toBeTruthy()
    await waitFor(() => expect(proposalRegion()).toBeTruthy())
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
    renderDialog({ traceStepMs: 100_000 })
    fireEvent.click(screen.getByRole("button", { name: "优化阅读" }))
    fireEvent.click(screen.getByRole("tab", { name: "执行轨迹" }))

    expect(screen.getByLabelText("读取设备能力：进行中")).toBeTruthy()
    expect(screen.getByLabelText("生成设置提案：待处理")).toBeTruthy()
    expect(screen.queryByRole("button", { name: "批准并应用" })).toBeNull()
    cleanup()

    renderDialog()
    await openProposal()
    fireEvent.click(screen.getByRole("tab", { name: "执行轨迹" }))
    expect(screen.getByLabelText("读取设置快照：已完成")).toBeTruthy()
    expect(screen.getByLabelText("等待用户批准：进行中")).toBeTruthy()

    fireEvent.click(screen.getByRole("button", { name: "批准并应用" }))
    await waitFor(() => expect(screen.getByLabelText("用户批准并应用：已完成")).toBeTruthy())
  })

  it("拒绝是零写入：没有回执，也没有任何存储写入", async () => {
    const setItem = vi.spyOn(Storage.prototype, "setItem")
    renderDialog()
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
    expect(within(region).queryByText(/Mock\/预览 · 非真实模型输出/)).toBeNull()

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

describe("设置页智能功能分区的模型空态", () => {
  // 这些用例保持**只读**：它们断言的是「没有任何 Provider 配置」时的诚实空态。
  // 写入路径（新建配置、单向提交 API Key）由 `pages/SettingsPage.ai-provider.test.tsx`
  // 覆盖 —— 那里有独立的模块注册表，不会与这里的 Mock 单例状态互相污染。
  function renderAiSettings() {
    return render(
      <MemoryRouter initialEntries={["/settings/ai"]}>
        <Routes>
          <Route path="/settings/:section" element={<SettingsPage />} />
        </Routes>
      </MemoryRouter>,
    )
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

    for (const fakeModel of ["gpt-4o", "claude-compatible", "vision-compatible"]) {
      expect(document.body.textContent).not.toContain(fakeModel)
    }
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
