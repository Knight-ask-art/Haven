import { useCallback, useEffect, useId, useMemo, useReducer, useRef, useState } from "react"
import type { FormEvent, KeyboardEvent as ReactKeyboardEvent, ReactNode } from "react"
import { createPortal } from "react-dom"
import {
  ArrowRight,
  Check,
  ChevronRight,
  CircleAlert,
  CircleX,
  FileClock,
  ListTree,
  Loader2,
  MessageSquare,
  Receipt,
  Send,
  ShieldCheck,
  Sparkles,
  TriangleAlert,
  X,
} from "lucide-react"
import { cn } from "@/lib/utils"
import { HavenError, isErrorDto } from "@/lib/ipc/errors"
import { getHavenClient } from "@/lib/ipc/runtime"
import { MockHavenClient } from "@/lib/ipc/mock-client"
import type { PreferenceReadingPatchDto } from "@/lib/ipc/generated/wire"

/**
 * 栖伴（Haven 智能体，前端第一阶段）。
 *
 * 用户可见名称统一为「栖伴」；组件名、文件名与目录沿用 ai-assistant / AiAssistantDialog，
 * 不改动既有导入路径与内部标识。
 *
 * 边界说明：本阶段**没有**接入 Provider / MCP / 模型服务，因此组件从不发起网络请求，
 * 也不推断或显示任何模型名；没有可用模型时它明确显示这一点。
 *
 * 写入路径只有一条：`agent_settings_*` Typed IPC。提案的 canonical digest 一律来自
 * Rust（Mock 场景来自 MockHavenClient 的确定性摘要），UI 只负责展示并把它原样回传；
 * 组件内部**不使用**本地 FNV 摘要作为领域事实（那只服务于纯预览回执的展示）。
 *
 * 三种运行状态必须区分：
 * - `client` 可用（真实 Tauri 或显式 Mock）→ Typed 闭环：Context → Proposal → 批准 → Receipt；
 * - `client` 不可用 → 本地预览：批准只生成带「本地预览」标注的回执，零写入；
 * - Mock → 全流程都带「Mock/预览」标识，不冒充真实 AI 结果。
 */

/**
 * 提案作用域：与资源内设的优先级（本资源 → 版本 → 全局）以及后端 SettingProposal
 * 的 Target 闭合集合保持一致。UI 不自行推断作用域，只展示 Provider / 应用层给出的值。
 */
export type AiAssistantProposalTarget = "global" | "edition" | "media_item"

export interface AiAssistantProposalChange {
  /** 设置项稳定 id（例如 reading.fontSize）；不含 SQL、绝对路径或凭据。 */
  key: string
  label: string
  from: string
  to: string
}

export interface AiAssistantProposalDraft {
  id: string
  title: string
  summary: string
  target: AiAssistantProposalTarget
  targetLabel: string
  /** 提案基线版本；本地预览没有真实版本，显式使用 null 而不是编造一个。 */
  expectedRevision: string | null
  /**
   * 提案摘要。接入应用层后必须是 SettingProposal 的 canonical JSON SHA-256
   * （64 位小写十六进制）；本地预览值是仅用于本机核对的短摘要，不得当作领域 digest 使用。
   */
  digest: string
  changes: AiAssistantProposalChange[]
}

/**
 * 组件需要的 Typed IPC 子集（`HavenClient` 的 Agent 部分）。
 *
 * 只声明用到的命令，避免组件依赖整个客户端；真实实现是 `TauriHavenClient`，
 * 显式 Mock/开发契约场景是 `MockHavenClient`。
 */
export type AiAssistantAgentClient = {
  agentSettingsContextGet: HavenClientShape["agentSettingsContextGet"]
  agentSettingsProposalCreate: HavenClientShape["agentSettingsProposalCreate"]
  agentSettingsProposalApprove: HavenClientShape["agentSettingsProposalApprove"]
  agentSettingsProposalReject: HavenClientShape["agentSettingsProposalReject"]
}
type HavenClientShape = import("@/lib/ipc/client").HavenClient

export interface AiAssistantDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  /**
   * Typed IPC 客户端。缺省时按运行时自动选择（Tauri → 真实客户端；浏览器 dev →
   * Mock）。显式传入 Mock 实例时，整条链路都会显示「Mock/预览」标识。
   */
  client?: AiAssistantAgentClient
  /**
   * 纯预览模式：不获取任何客户端，批准只产生「本地预览」回执。
   * 保留它是为了让"尚未接线"这条路径可被测试钉住。
   */
  previewOnly?: boolean
  /** 轨迹节点推进间隔（毫秒）。默认值用于展示过程；测试可传 0。 */
  traceStepMs?: number
}

type AiAssistantView = "conversation" | "trace" | "proposals"

/** 失败发生在哪一步：提案生成失败 / 应用失败。两者在轨迹与文案上必须区分。 */
type AiAssistantFailureStage = "proposal" | "apply"

/**
 * 批准/拒绝被拦下的原因（闭合集合）。它同时是**提案卡片状态**与**本轮结果状态**：
 *
 * - `conflict` 版本冲突、`expired` 提案已过期、`closed` 提案不可再处理、
 *   `invalid` 提案校验失败，这四类都是**确定零写入**；
 * - 只有 `failed` 才是「应用失败，未确认写入结果」。
 *
 * 把四类零写入结果压成一句「应用失败」会把"没写"谎报成"不知道写没写"，
 * 所以它们各自占用独立状态与文案。
 */
type AiAssistantBlockedKind = "conflict" | "failed" | "expired" | "closed" | "invalid"

type AiAssistantTurnStatus =
  | "idle"
  | "drafting"
  | "proposal"
  | "rejected"
  | "applied"
  | AiAssistantBlockedKind
type AiAssistantProposalStatus = "pending" | "rejected" | "applied" | AiAssistantBlockedKind
/**
 * 轨迹节点状态。零写入的四类阻断（conflict / expired / closed / invalid）各自占一个节点状态：
 * 把它们压成「需要处理」会让过期与校验失败在轨迹里失去区别。
 */
type AiAssistantTraceNodeStatus =
  | "complete"
  | "active"
  | "pending"
  | AiAssistantBlockedKind
  | "skipped"

interface AiAssistantReceipt {
  revision: string
  appliedAt: string
  changedCount: number
  /** true 表示这是一条**没有写入任何设置**的预览回执（本地预览或 Mock 预览）。 */
  preview: boolean
  /** 回执记录 id（来自应用层）；本地预览没有真实回执，使用 null。 */
  receiptId: string | null
  /** Rust 生成的 canonical digest；本地预览为 null（不伪造领域摘要）。 */
  digest: string | null
}

interface AiAssistantProposalRecord extends AiAssistantProposalDraft {
  status: AiAssistantProposalStatus
  receipt: AiAssistantReceipt | null
  /** digest 是否来自本地预览草稿；走 Typed IPC 后为 false。 */
  digestPreview: boolean
  /**
   * 提案是否来自 Mock/开发契约客户端。true 时整条记录必须显示 Mock/预览标识，
   * 不得让用户误以为它来自真实模型。
   */
  mock: boolean
  /**
   * 只作用于**这一张卡片**的拦截说明（当前只有"拒绝失败"）。
   *
   * 它与 `status` 正交：拒绝失败时提案仍然是 pending（服务端也是 pending），
   * 因此卡片必须保持可重试，而不是被改写成终态。
   */
  notice: AiAssistantIssue | null
}

interface AiAssistantMessage {
  id: string
  role: "user" | "assistant"
  text: string
  /** 提案作为对话消息附件，而不是独立事实源。 */
  proposalId?: string
  tone?: "default" | "warning"
}

interface AiAssistantState {
  turn: number
  status: AiAssistantTurnStatus
  /** 已完成（或正在执行）的轨迹节点数量；最后一个节点由用户批准驱动，不在这里推进。 */
  traceRevealed: number
  messages: AiAssistantMessage[]
  proposals: AiAssistantProposalRecord[]
  /**
   * 最近一次生成的提案，**只用于执行轨迹的展示**（改动项数/作用域）。
   * 批准与拒绝一律按被点击卡片的 proposalId 定位，绝不读取这个字段。
   */
  latestProposalId: string | null
  /** 应用层业务消息；阻断时必须展示在 Dialog 内，不允许静默丢弃。 */
  errorMessage: string | null
  /** 仅 failed 有值：区分提案生成失败与应用失败。 */
  failedStage: AiAssistantFailureStage | null
}

interface AiAssistantTraceStep {
  id: string
  title: string
  description: string
  meta: Array<{ label: string; value: string }>
}

type AiAssistantAction =
  | { type: "submit"; prompt: string }
  | { type: "answer"; prompt: string; text: string }
  | { type: "advance-trace" }
  | { type: "proposal-ready"; draft: AiAssistantProposalDraft; digestPreview: boolean; mock: boolean }
  | { type: "proposal-failed"; message: string }
  // 提案动作一律带上被点击卡片的 proposalId：连续生成多份 pending 提案时，
  // 批准/拒绝只作用于用户点中的那一条。
  | { type: "reject"; proposalId: string }
  | { type: "reject-failed"; proposalId: string; message: string }
  | { type: "apply-applied"; proposalId: string; receipt: AiAssistantReceipt }
  | { type: "apply-blocked"; proposalId: string; kind: AiAssistantBlockedKind; message: string }

/**
 * 本阶段唯一接线的确定性意图：阅读排版优化。
 *
 * 它是 UI 侧的**意图模板**（固定目标档位），不是模型输出：真正生效的 proposal、
 * canonical digest、Diff 与回执都由 Rust Application/Domain 生成并校验。
 */
export const READING_OPTIMIZE_PATCH: PreferenceReadingPatchDto = {
  fontSize: "large",
  lineHeight: "airy",
  contentWidth: "narrow",
}

/** 设置项 key → 中文标签；未知 key 回落到 key 原文（不隐藏上游改动）。 */
const SETTING_KEY_LABELS: Record<string, string> = {
  "reading.fontFamily": "字体",
  "reading.customFontFamily": "自定义字体",
  "reading.fontSize": "正文字号",
  "reading.lineHeight": "行高",
  "reading.contentWidth": "正文宽度",
  "reading.theme": "阅读主题",
  "reading.customBackground": "自定义背景",
  "reading.customText": "自定义文字色",
  "reading.fontWeight": "字重",
  "reading.letterSpacing": "字间距",
  "reading.systemAuto": "跟随系统外观",
  "reading.pagination": "翻页方式",
}

function settingKeyLabel(key: string): string {
  return SETTING_KEY_LABELS[key] ?? key
}

/**
 * 从 IPC 错误里取出稳定 code。
 *
 * 同时接受 `HavenError` 与**裸 ErrorDto**（`{ code, userMessage, retryable }`）：
 * 后者会从网关/事件边界直接冒出来，只认 instanceof 会把版本冲突误判成未知失败。
 */
function ipcErrorCode(error: unknown): string | null {
  if (error instanceof HavenError) return error.code
  if (isErrorDto(error)) return error.code
  return null
}

/** 从 IPC 错误里取出可展示的业务事实（HavenError 的 message 就是 ErrorDto.userMessage）。 */
function describeIpcError(error: unknown, fallback: string): string {
  if (error instanceof HavenError) return error.message
  if (isErrorDto(error)) return error.userMessage
  return fallback
}

/**
 * 把批准/拒绝的失败归类成阻断原因。
 *
 * 关键区分：`REVISION_CONFLICT` 与三类提案自身不可用（过期/不可再处理/校验失败）
 * 都是**零写入**的确定结论，只有归不到任何已知 code 时才落到 `failed`
 * （"应用失败，未确认写入结果"）。
 */
function classifyProposalFailure(error: unknown): AiAssistantBlockedKind {
  switch (ipcErrorCode(error)) {
    case "REVISION_CONFLICT":
      return "conflict"
    case "SETTING_PROPOSAL_EXPIRED":
      return "expired"
    case "SETTING_PROPOSAL_NOT_PENDING":
    case "SETTING_PROPOSAL_REJECTED":
      return "closed"
    case "SETTING_PROPOSAL_DIGEST_MISMATCH":
    case "SETTING_PROPOSAL_NOT_FOUND":
      return "invalid"
    default:
      return "failed"
  }
}

/** 设置项档位值 → 中文文案（未知值原样展示，不隐藏上游改动）。 */
const SETTING_VALUE_LABELS: Record<string, string> = {
  sans: "无衬线", serif: "衬线", kai: "楷体", heiti: "黑体", fangsong: "仿宋", mianfei: "免费字体", custom: "自定义",
  small: "小", medium: "中", large: "大",
  compact: "紧凑", comfortable: "舒适", airy: "宽松",
  narrow: "较窄", wide: "较宽",
  light: "细", regular: "常规", semibold: "半粗", bold: "粗体",
  tight: "较紧", normal: "正常", relaxed: "较松", loose: "很松",
  system: "跟随系统", paper: "纸张", warm: "暖色", slate: "石板", dark: "深色", sepia: "护眼棕",
  eyeCare: "护眼", scroll: "连续滚动", paginated: "分页", double: "双栏",
  true: "是", false: "否",
}

function settingValueLabel(value: string): string {
  if (value.length === 0) return "未设置"
  return SETTING_VALUE_LABELS[value] ?? value
}

/**
 * 走 Typed IPC 生成提案：读取上下文 → 创建提案（携带 context id/hash + base revision）。
 *
 * canonical digest 完全来自 Rust（或 Mock 的确定性摘要），UI 不计算领域摘要。
 * 没有可用模型/设置读取能力时抛错，让调用方显示"无可用模型"，而不是退回草稿。
 */
async function buildAgentProposal(
  client: AiAssistantAgentClient,
): Promise<AiAssistantProposalDraft> {
  const context = await client.agentSettingsContextGet()
  if (!context.capabilities.capabilities.settingsRead) {
    throw new HavenError({
      code: "AGENT_CAPABILITY_NOT_GRANTED",
      userMessage: "当前没有可用的模型服务，栖伴无法读取设置并生成提案。",
      retryable: false,
    })
  }
  // 本阶段没有可用模型：提案内容是**确定性意图模板**，不是模型输出。
  // 页面必须始终把这一点显示给用户（文案与 Mock/预览标识分别承担）。
  const proposal = await client.agentSettingsProposalCreate({
    sessionId: cryptoRandomUuid(),
    requestId: cryptoRandomUuid(),
    contextId: context.contextId,
    contextHash: context.contextHash,
    baseRevision: context.revision,
    patch: READING_OPTIMIZE_PATCH,
  })
  return {
    id: proposal.proposalId,
    title: "阅读排版优化",
    summary: `已基于本机设置快照整理 ${proposal.changes.length} 项阅读排版改动。请在下方提案中逐项审查后决定是否批准。`,
    target: "global",
    targetLabel: proposal.targetLabel,
    expectedRevision: proposal.baseRevision,
    digest: proposal.digest,
    changes: proposal.changes.map((change) => ({
      key: change.key,
      label: settingKeyLabel(change.key),
      from: settingValueLabel(change.before),
      to: settingValueLabel(change.after),
    })),
  }
}

/** 会话/请求身份：只用于绑定本次交互，不承载任何权限。 */
function cryptoRandomUuid(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID()
  }
  // 仅在缺少 WebCrypto 的测试环境兜底；格式仍是 UUID v4 形状。
  const hex = "0123456789abcdef"
  let out = ""
  for (let index = 0; index < 32; index += 1) {
    out += hex[Math.floor(Math.random() * 16)]
  }
  return `${out.slice(0, 8)}-${out.slice(8, 12)}-4${out.slice(13, 16)}-8${out.slice(17, 20)}-${out.slice(20, 32)}`
}

const VIEW_TABS: Array<{ id: AiAssistantView; label: string; icon: ReactNode }> = [
  { id: "conversation", label: "对话", icon: <MessageSquare className="h-[15px] w-[15px]" strokeWidth={1.9} /> },
  { id: "trace", label: "执行轨迹", icon: <ListTree className="h-[15px] w-[15px]" strokeWidth={1.9} /> },
  { id: "proposals", label: "提案记录", icon: <FileClock className="h-[15px] w-[15px]" strokeWidth={1.9} /> },
]

const TRACE_STEPS: AiAssistantTraceStep[] = [
  {
    id: "device-capability",
    title: "读取设备能力",
    description: "确认本机可用的阅读与渲染能力。",
    meta: [{ label: "范围", value: "仅本机" }, { label: "访问", value: "只读" }],
  },
  {
    id: "settings-snapshot",
    title: "读取设置快照",
    description: "读取当前生效的设置版本，作为提案基线。",
    meta: [{ label: "来源", value: "本机设置" }, { label: "访问", value: "只读" }],
  },
  {
    id: "reading-preference",
    title: "读取阅读偏好",
    description: "读取全局默认与当前资源覆盖之间的传播关系。",
    meta: [{ label: "优先级", value: "本资源 → 版本 → 全局" }, { label: "访问", value: "只读" }],
  },
  {
    id: "proposal",
    title: "生成设置提案",
    description: "只整理可回滚的排版改动，不写入任何设置。",
    meta: [],
  },
  {
    id: "approval",
    title: "等待用户批准",
    description: "批准前不会写入任何设置。",
    meta: [{ label: "审批", value: "需要用户确认" }],
  },
]

/** 最后一个节点由批准动作驱动，轨迹最多推进到这里。 */
const TRACE_READY_REVEALED = TRACE_STEPS.length - 1

/** 提案生成节点；生成失败时失败标记落在这里，而不是批准节点。 */
const TRACE_PROPOSAL_INDEX = TRACE_STEPS.length - 2

const PROPOSAL_STATUS_LABEL: Record<AiAssistantProposalStatus, string> = {
  pending: "待批准",
  rejected: "已拒绝",
  applied: "已应用",
  conflict: "版本冲突",
  failed: "应用失败",
  expired: "已过期",
  closed: "不可再处理",
  invalid: "提案校验失败",
}

/**
 * 被拦下状态在提案卡片内的结论句。
 *
 * 只有 `failed` 是「未确认写入」；其余四类都已在服务端确定没有写入，
 * 必须逐字写成零写入，不能让用户以为设置可能已经被改动。
 */
const PROPOSAL_STATUS_NOTE: Partial<Record<AiAssistantProposalStatus, string>> = {
  conflict: "设置版本已变化，本次没有写入任何设置。请重新生成提案后再批准。",
  failed: "应用失败，未确认写入结果。请重新读取设置确认当前状态。",
  expired: "提案已过期，本次没有写入任何设置。请重新生成提案。",
  closed: "提案已不可再处理，本次没有写入任何设置。请重新生成提案。",
  invalid: "提案校验失败，本次没有写入任何设置。请重新生成提案后再批准。",
}

/** 被拦下时助手在对话里补的结论句；零写入的四类不得写成「未确认写入」。 */
const BLOCKED_TURN_NOTE: Record<AiAssistantBlockedKind, string> = {
  conflict: "设置版本在批准前发生变化，本次没有写入任何设置。",
  failed: "应用失败，请重新读取设置确认当前状态。",
  expired: "提案已过期，本次没有写入任何设置。",
  closed: "提案已不可再处理，本次没有写入任何设置。",
  invalid: "提案校验失败，本次没有写入任何设置。",
}

/** 失败与冲突必须使用不同标签：两者都不是「等待处理」；过期/关闭/校验失败同样各自区分。 */
const TRACE_NODE_STATUS_LABEL: Record<AiAssistantTraceNodeStatus, string> = {
  complete: "已完成",
  active: "进行中",
  pending: "待处理",
  conflict: "需要处理",
  expired: "已过期",
  closed: "不可再处理",
  invalid: "校验失败",
  failed: "失败",
  skipped: "已跳过",
}

const QUICK_ACTIONS: Array<{ id: string; label: string; prompt: string }> = [
  { id: "optimize-reading", label: "优化阅读", prompt: "优化阅读" },
  { id: "capability", label: "当前能改什么", prompt: "当前能改什么设置" },
]

/** 本阶段只接入「阅读排版」一个确定性意图；其余输入只给出能力说明，不生成提案。 */
const READING_INTENT_KEYWORDS = ["阅读", "排版", "字体", "字号", "行高", "正文宽度", "优化"]

export function wantsReadingProposal(prompt: string): boolean {
  const normalized = prompt.trim().toLocaleLowerCase()
  return READING_INTENT_KEYWORDS.some((keyword) => normalized.includes(keyword))
}

/**
 * 本地预览用的短摘要（FNV-1a，8 位十六进制），只让用户核对两次预览是否同一份。
 * 它不是领域 digest：SettingProposal 的 digest 是 canonical JSON 的 SHA-256
 * （恰好 64 位小写十六进制），只能由应用层计算并回传。
 */
export function proposalDigest(input: { target: string; changes: AiAssistantProposalChange[] }): string {
  const serialized = `${input.target}|${input.changes.map((change) => `${change.key}:${change.from}->${change.to}`).join("|")}`
  let hash = 0x811c9dc5
  for (let index = 0; index < serialized.length; index += 1) {
    hash ^= serialized.charCodeAt(index)
    hash = Math.imul(hash, 0x01000193) >>> 0
  }
  return hash.toString(16).padStart(8, "0")
}

/** 本地预览草稿：内容固定，不依赖 Provider，也不读取真实设置。 */
export function createPreviewProposal(turn: number): AiAssistantProposalDraft {
  const changes: AiAssistantProposalChange[] = [
    { key: "reading.fontSize", label: "正文字号", from: "中", to: "大" },
    { key: "reading.lineHeight", label: "行高", from: "舒适", to: "宽松" },
    { key: "reading.contentWidth", label: "正文宽度", from: "适中", to: "较窄" },
  ]
  return {
    id: `preview-proposal-${turn}`,
    title: "阅读排版优化",
    summary: `已基于本机设置快照整理 ${changes.length} 项阅读排版改动。请在下方提案中逐项审查后决定是否批准。`,
    target: "global",
    targetLabel: "全局默认",
    expectedRevision: null,
    digest: proposalDigest({ target: "global", changes }),
    changes,
  }
}

function createInitialState(): AiAssistantState {
  return {
    turn: 0,
    status: "idle",
    traceRevealed: 0,
    latestProposalId: null,
    errorMessage: null,
    failedStage: null,
    proposals: [],
    messages: [
      {
        id: "seed-greeting",
        role: "assistant",
        text: "我是栖伴，栖阅的 Haven 智能体。我会先读取本机设备能力、设置快照和阅读偏好，再给出可以逐项审查的设置提案。",
      },
      { id: "seed-request", role: "user", text: "阅读时正文偏小，排版也有点挤。" },
      {
        id: "seed-offer",
        role: "assistant",
        text: "可以。我会生成一份只改排版三项的提案，批准之前不会写入任何设置。需要现在生成吗？",
      },
    ],
  }
}

function patchProposal(
  proposals: AiAssistantProposalRecord[],
  proposalId: string,
  patch: Partial<AiAssistantProposalRecord>,
): AiAssistantProposalRecord[] {
  return proposals.map((proposal) => (proposal.id === proposalId ? { ...proposal, ...patch } : proposal))
}

/** 按 id 取出仍待批准的记录：只有 pending 的卡片才允许被批准/拒绝。 */
function pendingProposalOf(state: AiAssistantState, proposalId: string): AiAssistantProposalRecord | null {
  const record = state.proposals.find((proposal) => proposal.id === proposalId) ?? null
  return record && record.status === "pending" ? record : null
}

function reducer(state: AiAssistantState, action: AiAssistantAction): AiAssistantState {
  switch (action.type) {
    case "submit": {
      const turn = state.turn + 1
      return {
        ...state,
        turn,
        status: "drafting",
        traceRevealed: 0,
        latestProposalId: null,
        errorMessage: null,
        failedStage: null,
        messages: [...state.messages, { id: `m-${turn}-user`, role: "user", text: action.prompt }],
      }
    }
    case "answer": {
      const turn = state.turn + 1
      return {
        ...state,
        turn,
        messages: [
          ...state.messages,
          { id: `m-${turn}-user`, role: "user", text: action.prompt },
          { id: `m-${turn}-answer`, role: "assistant", text: action.text },
        ],
      }
    }
    case "advance-trace": {
      if (state.status !== "drafting") return state
      return { ...state, traceRevealed: Math.min(state.traceRevealed + 1, TRACE_READY_REVEALED) }
    }
    case "proposal-ready": {
      const record: AiAssistantProposalRecord = {
        ...action.draft,
        status: "pending",
        receipt: null,
        digestPreview: action.digestPreview,
        mock: action.mock,
        notice: null,
      }
      return {
        ...state,
        status: "proposal",
        traceRevealed: TRACE_READY_REVEALED,
        latestProposalId: record.id,
        errorMessage: null,
        failedStage: null,
        proposals: [record, ...state.proposals],
        messages: [
          ...state.messages,
          { id: `m-${state.turn}-proposal`, role: "assistant", text: record.summary, proposalId: record.id },
        ],
      }
    }
    case "proposal-failed": {
      return {
        ...state,
        status: "failed",
        failedStage: "proposal",
        traceRevealed: TRACE_PROPOSAL_INDEX,
        errorMessage: action.message,
        messages: [
          ...state.messages,
          {
            id: `m-${state.turn}-failed`,
            role: "assistant",
            text: `${action.message}本次没有写入任何设置。`,
            tone: "warning",
          },
        ],
      }
    }
    case "reject": {
      if (!pendingProposalOf(state, action.proposalId)) return state
      // 只有作用于**最新**提案时才推进本轮状态与轨迹；处理旧卡片不得串改最新轨迹。
      const isLatest = state.latestProposalId === action.proposalId
      return {
        ...state,
        status: isLatest ? "rejected" : state.status,
        proposals: patchProposal(state.proposals, action.proposalId, { status: "rejected", notice: null }),
        messages: [
          ...state.messages,
          {
            id: `m-${state.turn}-reject-${action.proposalId}`,
            role: "assistant",
            text: "已拒绝该提案，本次没有写入任何设置。",
          },
        ],
      }
    }
    case "reject-failed": {
      // 拒绝失败**不改变提案状态**：服务端提案仍然是 pending，本轮也没有进入终态，
      // 因此卡片保持可重试（批准/拒绝按钮继续可用），文案只承诺零写入。
      if (!pendingProposalOf(state, action.proposalId)) return state
      return {
        ...state,
        proposals: patchProposal(state.proposals, action.proposalId, {
          notice: {
            tone: "warning",
            title: "拒绝失败",
            message: action.message,
            detail: "本次没有写入任何设置，提案仍待批准，可以重试。",
          },
        }),
      }
    }
    case "apply-applied": {
      if (!pendingProposalOf(state, action.proposalId)) return state
      const isLatest = state.latestProposalId === action.proposalId
      const note = action.receipt.preview
        ? "已生成设置变更回执（本地预览）。本次没有写入任何设置。"
        : "设置变更已应用，回执如下。"
      return {
        ...state,
        status: isLatest ? "applied" : state.status,
        proposals: patchProposal(state.proposals, action.proposalId, {
          status: "applied",
          receipt: action.receipt,
          notice: null,
        }),
        messages: [
          ...state.messages,
          { id: `m-${state.turn}-applied-${action.proposalId}`, role: "assistant", text: note },
        ],
      }
    }
    case "apply-blocked": {
      if (!pendingProposalOf(state, action.proposalId)) return state
      const isLatest = state.latestProposalId === action.proposalId
      return {
        ...state,
        status: isLatest ? action.kind : state.status,
        // 只有真正的应用失败才是「未确认写入」；其余四类都是确定零写入。
        failedStage: isLatest && action.kind === "failed" ? "apply" : state.failedStage,
        errorMessage: isLatest ? action.message : state.errorMessage,
        proposals: patchProposal(state.proposals, action.proposalId, { status: action.kind, notice: null }),
        messages: [
          ...state.messages,
          {
            id: `m-${state.turn}-blocked-${action.proposalId}`,
            role: "assistant",
            text: BLOCKED_TURN_NOTE[action.kind],
            tone: "warning",
          },
        ],
      }
    }
    default:
      return state
  }
}

/** 节点状态：绿色完成 / 蓝色当前 / 灰色待处理 / 琥珀色冲突 / 红色失败。 */
export function traceNodeStatuses(state: {
  status: AiAssistantTurnStatus
  traceRevealed: number
  failedStage?: AiAssistantFailureStage | null
}): AiAssistantTraceNodeStatus[] {
  const statuses: AiAssistantTraceNodeStatus[] = TRACE_STEPS.map(() => "pending")
  if (state.status === "idle") return statuses

  for (let index = 0; index < state.traceRevealed && index < statuses.length; index += 1) {
    statuses[index] = "complete"
  }

  if (state.status === "drafting") {
    statuses[Math.min(state.traceRevealed, TRACE_STEPS.length - 1)] = "active"
    return statuses
  }
  if (state.status === "proposal") {
    statuses[TRACE_STEPS.length - 1] = "active"
    return statuses
  }
  if (state.status === "applied") {
    statuses[TRACE_STEPS.length - 1] = "complete"
    return statuses
  }
  if (state.status === "rejected") {
    statuses[TRACE_STEPS.length - 1] = "skipped"
    return statuses
  }
  if (state.status === "failed") {
    // 提案生成失败落在提案节点；应用失败落在批准节点。失败不是冲突，必须分开展示。
    statuses[state.failedStage === "proposal" ? TRACE_PROPOSAL_INDEX : TRACE_STEPS.length - 1] = "failed"
    return statuses
  }
  // 剩下的都是确定零写入的阻断，各自沿用独立节点状态。
  statuses[Math.min(state.traceRevealed, TRACE_STEPS.length - 1)] = state.status
  return statuses
}

function approvalNodeCopy(
  state: Pick<AiAssistantState, "status" | "failedStage">,
): Pick<AiAssistantTraceStep, "title" | "description" | "meta"> {
  switch (state.status) {
    case "applied":
      return {
        title: "用户批准并应用",
        description: "设置变更已应用，回执已生成。",
        meta: [{ label: "写入", value: "已应用" }],
      }
    case "rejected":
      return {
        title: "用户已拒绝",
        description: "本次没有写入任何设置。",
        meta: [{ label: "写入", value: "零写入" }],
      }
    case "conflict":
      return {
        title: "版本冲突",
        description: "设置版本已变化，本次没有写入任何设置。",
        meta: [{ label: "写入", value: "零写入" }],
      }
    case "expired":
      return {
        title: "提案已过期",
        description: "提案在批准前已过期，本次没有写入任何设置。",
        meta: [{ label: "写入", value: "零写入" }],
      }
    case "closed":
      return {
        title: "提案不可再处理",
        description: "提案已进入终态，本次没有写入任何设置。",
        meta: [{ label: "写入", value: "零写入" }],
      }
    case "invalid":
      return {
        title: "提案校验失败",
        description: "提案身份或摘要校验未通过，本次没有写入任何设置。",
        meta: [{ label: "写入", value: "零写入" }],
      }
    case "failed":
      // failed 必须有独立文案，不能退回「等待用户批准」。
      return state.failedStage === "apply"
        ? {
          title: "应用失败",
          description: "应用层回调失败，未确认写入结果。",
          meta: [{ label: "写入", value: "未确认" }],
        }
        : {
          title: "未进入批准",
          description: "提案未生成，本次没有写入任何设置。",
          meta: [{ label: "写入", value: "零写入" }],
        }
    default:
      return {
        title: "等待用户批准",
        description: "批准前不会写入任何设置。",
        meta: [{ label: "审批", value: "需要用户确认" }],
      }
  }
}

function proposalNodeCopy(state: AiAssistantState): Pick<AiAssistantTraceStep, "title" | "description" | "meta"> {
  if (state.status === "failed" && state.failedStage === "proposal") {
    return {
      title: TRACE_STEPS[TRACE_PROPOSAL_INDEX].title,
      description: "读取本机设置失败，未生成提案。",
      meta: [{ label: "改动", value: "未生成" }, { label: "写入", value: "零写入" }],
    }
  }
  // latestProposalId 只服务轨迹展示：它永远指向最近生成的那份提案，
  // 无论用户随后批准/拒绝的是哪一张卡片。
  const latest = state.proposals.find((proposal) => proposal.id === state.latestProposalId) ?? null
  return {
    title: TRACE_STEPS[TRACE_PROPOSAL_INDEX].title,
    description: TRACE_STEPS[TRACE_PROPOSAL_INDEX].description,
    meta: [
      { label: "改动", value: latest ? `${latest.changes.length} 项` : "尚未生成" },
      { label: "作用域", value: latest ? latest.targetLabel : "未确定" },
    ],
  }
}

function buildTraceSteps(state: AiAssistantState): AiAssistantTraceStep[] {
  return [
    TRACE_STEPS[0],
    TRACE_STEPS[1],
    TRACE_STEPS[2],
    { ...TRACE_STEPS[TRACE_PROPOSAL_INDEX], ...proposalNodeCopy(state) },
    { ...TRACE_STEPS[TRACE_STEPS.length - 1], ...approvalNodeCopy(state) },
  ]
}

function formatNow(): string {
  // 只用于回执展示；本地预览不会把它写入任何存储。
  return new Date().toLocaleString("zh-CN", { hour12: false })
}

/** digest 必须在同一行标注来源，避免本地预览/Mock 摘要被误读为真实领域 digest。 */
function digestDisplay(record: { digest: string; digestPreview: boolean; mock?: boolean }): string {
  if (record.digestPreview) return `${record.digest}（本地预览）`
  if (record.mock) return `${record.digest}（Mock/预览）`
  return record.digest
}

interface AiAssistantIssue {
  tone: "warning" | "danger"
  title: string
  /** 应用层业务消息原文，不做改写。 */
  message: string
  /** 写入结论：冲突永远是零写入，应用失败必须是「未确认」。 */
  detail: string
}

/**
 * 冲突与失败/零写入拦阻的提示条内容。errorMessage 是应用层给的业务消息，必须原样出现在
 * Dialog 内；结论句由 UI 补全，避免上游漏写「是否写入」。
 *
 * 只有未知应用异常（failed）才是「未确认写入」；REVISION_CONFLICT 与提案自身的
 * 过期/不可再处理/校验失败都是服务端已确定的零写入结论。
 */
function buildIssue(state: AiAssistantState): AiAssistantIssue | null {
  if (state.status === "failed") {
    if (state.failedStage === "proposal") {
      return {
        tone: "danger",
        title: "提案生成失败",
        message: state.errorMessage ?? "读取本机设置失败，未生成提案。",
        detail: "本次没有写入任何设置。",
      }
    }
    return {
      tone: "danger",
      title: "应用失败",
      message: state.errorMessage ?? "应用层回调不可用。",
      detail: "未确认写入结果，请重新读取设置确认当前状态。",
    }
  }
  const blocked = blockedIssueCopy(state.status)
  if (!blocked) return null
  return {
    tone: "warning",
    title: blocked.title,
    message: state.errorMessage ?? blocked.fallback,
    detail: "本次没有写入任何设置。",
  }
}

/** 确定零写入的四类阻断在 Dialog 顶部的标题与兜底业务消息（业务消息缺失时才有用）。 */
function blockedIssueCopy(
  status: AiAssistantTurnStatus,
): { title: string; fallback: string } | null {
  switch (status) {
    case "conflict":
      return { title: "版本冲突", fallback: "设置版本在批准前发生了变化。" }
    case "expired":
      return { title: "提案已过期", fallback: "提案在批准前已过期。" }
    case "closed":
      return { title: "提案不可再处理", fallback: "提案已进入终态，不能再处理。" }
    case "invalid":
      return { title: "提案校验失败", fallback: "提案身份或摘要校验未通过。" }
    default:
      return null
  }
}

/** 节点配色：琥珀=需要用户处理，灰色=终态（已拒绝/不可再处理）。 */
const WARN_NODE_TONE = {
  node: "border-[#f0b429]/50 bg-[#fff8e5] text-[#b7791f] dark:border-[#f0b429]/40 dark:bg-[#3a2f12] dark:text-[#f0b429]",
  connector: "bg-[#f0b429]/40",
  title: "text-[#b7791f] dark:text-[#f0b429]",
}

const MUTED_NODE_TONE = {
  node: "border-black/[0.08] bg-black/[0.03] text-[#86868b] dark:border-white/[0.1] dark:bg-white/[0.04] dark:text-[#98989d]",
  connector: "bg-black/[0.08] dark:bg-white/[0.1]",
  title: "text-[#6e6e73] dark:text-[#98989d]",
}

const NODE_TONE: Record<AiAssistantTraceNodeStatus, { node: string; connector: string; title: string }> = {
  complete: {
    node: "border-[#34c759]/40 bg-[#34c759]/[0.14] text-[#248a3d] dark:text-[#30d158]",
    connector: "bg-[#34c759]/30",
    title: "text-[#1d1d1f] dark:text-[#f5f5f5]",
  },
  active: {
    node: "border-[#007aff]/45 bg-[#007aff]/[0.14] text-[#007aff]",
    connector: "bg-[#007aff]/25",
    title: "text-[#007aff]",
  },
  pending: {
    node: "border-black/[0.08] bg-black/[0.03] text-[#c7c7cc] dark:border-white/[0.1] dark:bg-white/[0.04] dark:text-[#636366]",
    connector: "bg-black/[0.08] dark:bg-white/[0.1]",
    title: "text-[#86868b] dark:text-[#98989d]",
  },
  // 需要用户处理的零写入阻断（版本冲突 / 过期 / 校验失败）共用琥珀色。
  conflict: WARN_NODE_TONE,
  expired: WARN_NODE_TONE,
  invalid: WARN_NODE_TONE,
  // 已关闭与已拒绝一样是灰色终态，不是需要用户处理的问题。
  closed: MUTED_NODE_TONE,
  failed: {
    node: "border-[#ff3b30]/45 bg-[#ff3b30]/[0.12] text-[#d70015] dark:border-[#ff453a]/45 dark:bg-[#3a1a18] dark:text-[#ff453a]",
    connector: "bg-[#ff3b30]/35",
    title: "text-[#d70015] dark:text-[#ff453a]",
  },
  skipped: MUTED_NODE_TONE,
}

export function AiAssistantDialog({
  open,
  onOpenChange,
  client,
  previewOnly = false,
  traceStepMs = 420,
}: AiAssistantDialogProps) {
  const [state, dispatch] = useReducer(reducer, undefined, createInitialState)
  const [view, setView] = useState<AiAssistantView>("conversation")
  const [input, setInput] = useState("")
  const [busy, setBusy] = useState(false)
  const titleId = useId()
  const inputId = useId()
  const panelRef = useRef<HTMLDivElement>(null)
  const lastPromptRef = useRef("")
  const tabButtonRefs = useRef<Partial<Record<AiAssistantView, HTMLButtonElement | null>>>({})

  // 客户端只解析一次：Tauri → 真实 IPC；浏览器 dev / 显式注入 → Mock。
  // 解析失败（unavailable）等同"没有可用模型服务"：退回本地预览，而不是伪造结果。
  const resolvedClient = useMemo<AiAssistantAgentClient | null>(() => {
    if (previewOnly) return null
    if (client) return client
    try {
      return getHavenClient()
    } catch {
      return null
    }
  }, [client, previewOnly])

  const mockMode = resolvedClient instanceof MockHavenClient

  useEffect(() => {
    if (open) setView("conversation")
  }, [open])

  useEffect(() => {
    panelRef.current?.focus()
  }, [open])

  useEffect(() => {
    if (!open) return
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return
      event.stopPropagation()
      onOpenChange(false)
    }
    document.addEventListener("keydown", handleKeyDown)
    return () => document.removeEventListener("keydown", handleKeyDown)
  }, [open, onOpenChange])

  // 轨迹推进：推进到提案节点后等待用户批准。有客户端时走 Typed IPC，
  // 否则退回本地预览草稿（digest 会标注「本地预览」，不会进入任何写入路径）。
  useEffect(() => {
    if (state.status !== "drafting") return

    if (state.traceRevealed < TRACE_READY_REVEALED) {
      const timer = window.setTimeout(() => dispatch({ type: "advance-trace" }), traceStepMs)
      return () => window.clearTimeout(timer)
    }

    let cancelled = false
    const fallback = () => {
      if (!cancelled) {
        dispatch({
          type: "proposal-ready",
          draft: createPreviewProposal(state.turn),
          digestPreview: true,
          mock: false,
        })
      }
    }
    if (!resolvedClient) {
      fallback()
      return () => {
        cancelled = true
      }
    }

    const request = buildAgentProposal(resolvedClient).then(
      (draft) => {
        if (!cancelled) {
          dispatch({ type: "proposal-ready", draft, digestPreview: false, mock: mockMode })
        }
      },
      (error: unknown) => {
        if (cancelled) return
        // 没有可用模型/设置读取失败：明确说明，不生成伪造 AI 结果，也不退回预览草稿。
        const message = describeIpcError(error, "读取本机设置失败，未生成提案。")
        dispatch({ type: "proposal-failed", message })
      },
    )
    void request
    return () => {
      cancelled = true
    }
  }, [state.status, state.traceRevealed, state.turn, traceStepMs, resolvedClient, mockMode])

  const isDrafting = state.status === "drafting"
  const issue = buildIssue(state)
  // 没有可用客户端时进入本地预览：批准只推进 UI 状态，不伪造持久化。
  const previewMode = resolvedClient === null

  const submitPrompt = useCallback((raw: string) => {
    const prompt = raw.trim()
    if (!prompt || state.status === "drafting") return
    setInput("")
    if (!wantsReadingProposal(prompt)) {
      dispatch({
        type: "answer",
        prompt,
        text: previewMode
          ? "当前阶段我只接入了阅读排版提案（本地预览）。可以试试输入「优化阅读」，或使用上方的快捷操作。"
          : "当前阶段我只接入了阅读排版提案。可以试试输入「优化阅读」，或使用上方的快捷操作。",
      })
      return
    }
    lastPromptRef.current = prompt
    dispatch({ type: "submit", prompt })
  }, [state.status, previewMode])

  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    submitPrompt(input)
  }

  /**
   * 拒绝被点击的那张卡片。
   *
   * `proposalId` 一律来自被点击的卡片：连续生成多份 pending 提案时，
   * 旧卡片的拒绝绝不能落到最新提案上（那会拒错提案，也会串改最新轨迹）。
   */
  const handleReject = async (proposalId: string) => {
    if (busy) return
    const record = pendingProposalOf(state, proposalId)
    if (!record) return
    // 预览草稿没有服务端提案可拒绝，也不该走写入路径：直接推进本地状态。
    if (record.digestPreview || !resolvedClient) {
      dispatch({ type: "reject", proposalId })
      return
    }
    setBusy(true)
    try {
      // 拒绝同样携带 UI 显示的那份 digest：服务端据此核对，且不写任何设置。
      await resolvedClient.agentSettingsProposalReject({
        proposalId: record.id,
        expectedDigest: record.digest,
      })
      dispatch({ type: "reject", proposalId })
    } catch (error) {
      // 拒绝失败不是"应用失败"：服务端提案仍是 pending，本轮没有写入任何设置，
      // 因此卡片保持待批准并可重试（见 reject-failed 的 reducer 分支）。
      const message = describeIpcError(error, "拒绝失败，请重试。")
      const kind = classifyProposalFailure(error)
      if (kind === "failed") {
        dispatch({ type: "reject-failed", proposalId, message })
      } else {
        dispatch({ type: "apply-blocked", proposalId, kind, message })
      }
    } finally {
      setBusy(false)
    }
  }

  /** 批准被点击的那张卡片；同样只认卡片自己的 proposalId 与 digest。 */
  const handleApprove = async (proposalId: string) => {
    if (busy) return
    const record = pendingProposalOf(state, proposalId)
    if (!record) return
    setBusy(true)
    try {
      // 双重防护：即使客户端在提案生成后不可用，也以记录来源标记为准。
      // 本地预览草稿永远只产生预览回执，绝不进入任何写入路径。
      const fromPreview = record.digestPreview
      if (fromPreview || !resolvedClient) {
        if (fromPreview) {
          dispatch({
            type: "apply-applied",
            proposalId,
            receipt: {
              revision: "local-preview",
              appliedAt: formatNow(),
              changedCount: record.changes.length,
              preview: true,
              receiptId: null,
              digest: null,
            },
          })
        } else {
          // 应用层提案已经生成，但客户端已不可用：既不能写入，也不能伪造成预览回执。
          dispatch({
            type: "apply-blocked",
            proposalId,
            kind: "failed",
            message: "应用层不可用，未确认写入结果。",
          })
        }
        return
      }

      // 唯一写入口：只提交提案 ID 与 UI 显示的那份 canonical digest。
      // 一次性审批令牌由 Rust 内部生成并消费，前端既不持有也不传递它。
      const result = await resolvedClient.agentSettingsProposalApprove({
        proposalId: record.id,
        expectedDigest: record.digest,
      })
      dispatch({
        type: "apply-applied",
        proposalId,
        receipt: {
          revision: result.receipt.appliedRevision ?? "unchanged",
          appliedAt: result.receipt.appliedAt,
          changedCount: result.receipt.changes.length,
          // Mock 客户端不写真实设置：回执必须带「Mock/预览」标注。
          preview: mockMode,
          receiptId: result.receipt.receiptId,
          digest: result.receipt.proposalDigest,
        },
      })
    } catch (error) {
      // 已知 code 一律是服务端已确定的零写入结论；只有归不到任何 code 才是未确认写入。
      const message = describeIpcError(error, "应用失败，未确认写入结果。")
      dispatch({
        type: "apply-blocked",
        proposalId,
        kind: classifyProposalFailure(error),
        message,
      })
    } finally {
      setBusy(false)
    }
  }

  // rail 是竖向分区导航：上下为主方向，左右保留为兼容方向。
  const handleTabKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const forward = event.key === "ArrowDown" || event.key === "ArrowRight"
    const backward = event.key === "ArrowUp" || event.key === "ArrowLeft"
    if (!forward && !backward) return
    event.preventDefault()
    const currentIndex = VIEW_TABS.findIndex((tab) => tab.id === view)
    const offset = forward ? 1 : -1
    const next = VIEW_TABS[(currentIndex + offset + VIEW_TABS.length) % VIEW_TABS.length]
    setView(next.id)
    tabButtonRefs.current[next.id]?.focus()
  }

  if (!open) return null

  const renderProposalAttachment = (proposalId: string) => {
    const record = state.proposals.find((proposal) => proposal.id === proposalId)
    if (!record) return null
    return (
      <ProposalAttachment
        record={record}
        busy={busy}
        onViewTrace={() => setView("trace")}
        onReject={handleReject}
        onApprove={handleApprove}
      />
    )
  }

  const messagesPane = (
    <div className="settings-scrollbar-hidden min-h-0 flex-1 space-y-3 overflow-y-auto px-5 py-5 sm:px-6">
      {state.messages.map((message) => (
        <div key={message.id} className={cn("flex", message.role === "user" ? "justify-end" : "justify-start")}>
          <div className={cn("max-w-[86%] space-y-2", message.role === "user" && "flex flex-col items-end")}>
            <p
              className={cn(
                "rounded-2xl px-4 py-2.5 text-[13px] leading-6",
                message.role === "user"
                  ? "bg-[#007aff] text-white"
                  : message.tone === "warning"
                    ? "border border-[#f0b429]/35 bg-[#fff8e5] text-[#7a5a1a] dark:bg-[#3a2f12] dark:text-[#f0b429]"
                    : "border border-black/[0.06] bg-white text-[#1d1d1f] dark:border-white/[0.08] dark:bg-[#2c2c2e] dark:text-[#f5f5f5]",
              )}
            >
              {message.text}
            </p>
            {message.proposalId ? renderProposalAttachment(message.proposalId) : null}
          </div>
        </div>
      ))}
      {isDrafting && (
        <p className="flex items-center gap-2 text-[12px] text-[#86868b] dark:text-[#98989d]" role="status">
          <Loader2 className="h-[14px] w-[14px] animate-spin" strokeWidth={2.2} />
          正在读取本机设置并生成提案…
        </p>
      )}
    </div>
  )

  const composer = (
    <form
      onSubmit={handleSubmit}
      className="shrink-0 border-t border-black/[0.06] bg-white/70 px-5 py-4 dark:border-white/[0.06] dark:bg-[#1c1c1e]/80 sm:px-6"
    >
      <div className="flex flex-wrap gap-2 pb-3">
        {QUICK_ACTIONS.map((action) => (
          <button
            key={action.id}
            type="button"
            disabled={isDrafting}
            onClick={() => submitPrompt(action.prompt)}
            className="rounded-full border border-black/[0.08] bg-white px-3 py-1.5 text-[12px] font-medium text-[#1d1d1f] transition-colors hover:border-[#007aff]/40 hover:text-[#007aff] disabled:cursor-not-allowed disabled:opacity-50 dark:border-white/[0.1] dark:bg-[#2c2c2e] dark:text-[#f5f5f5]"
          >
            {action.label}
          </button>
        ))}
      </div>
      <div className="flex items-center gap-2 rounded-2xl border border-black/[0.08] bg-white px-3 py-2 focus-within:border-[#007aff]/45 focus-within:ring-4 focus-within:ring-[#007aff]/10 dark:border-white/[0.1] dark:bg-[#2c2c2e]">
        <label htmlFor={inputId} className="sr-only">
          描述你想要的设置改动
        </label>
        <input
          id={inputId}
          value={input}
          onChange={(event) => setInput(event.target.value)}
          placeholder="描述你想要的设置改动，例如：优化阅读"
          disabled={isDrafting}
          autoComplete="off"
          className="min-w-0 flex-1 bg-transparent text-[13px] text-[#1d1d1f] outline-none placeholder:text-[#a1a1a6] disabled:cursor-not-allowed dark:text-[#f5f5f5] dark:placeholder:text-[#8e8e93]"
        />
        <button
          type="submit"
          aria-label="发送"
          disabled={isDrafting || input.trim().length === 0}
          className="flex h-[32px] w-[32px] shrink-0 items-center justify-center rounded-full bg-[#007aff] text-white transition-colors hover:bg-[#006fe6] disabled:cursor-not-allowed disabled:opacity-40"
        >
          <Send className="h-[15px] w-[15px]" strokeWidth={2} />
        </button>
      </div>
      <p className="mt-2 text-[11px] leading-5 text-[#86868b] dark:text-[#8e8e93]">
        {previewMode
          ? "本地预览 · 提交只在本机推进界面状态，不发起网络请求，也不写入设置。"
          : "批准后会通过设置应用层写入，写入前仍需要你逐项确认。"}
      </p>
    </form>
  )

  const traceInspector = (
    <TraceInspector steps={buildTraceSteps(state)} statuses={traceNodeStatuses(state)} running={isDrafting} />
  )

  return createPortal(
    <div
      className="fixed inset-0 z-[130] flex items-stretch justify-center bg-black/35 backdrop-blur-[2px] sm:items-center sm:p-6"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onOpenChange(false)
      }}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        className="flex h-full w-full max-w-[1040px] flex-col overflow-hidden border border-black/[0.06] bg-[#f5f5f7] text-[#1d1d1f] shadow-[0_30px_80px_rgba(0,0,0,0.28)] outline-none dark:border-white/[0.08] dark:bg-[#1c1c1e] dark:text-[#f5f5f5] sm:h-[min(780px,calc(100dvh-48px))] sm:rounded-[24px]"
      >
        <header className="flex shrink-0 items-start gap-3 border-b border-black/[0.06] px-5 py-4 dark:border-white/[0.06] sm:px-6">
          <span className="flex h-[34px] w-[34px] shrink-0 items-center justify-center rounded-xl bg-[#007aff]/[0.12] text-[#007aff]">
            <Sparkles className="h-[18px] w-[18px]" strokeWidth={1.9} />
          </span>
          <div className="min-w-0 flex-1">
            <h2 id={titleId} className="text-[16px] font-semibold tracking-[-0.02em]">
              栖伴
            </h2>
            <p className="mt-0.5 text-[12px] leading-5 text-[#6e6e73] dark:text-[#98989d]">
              {previewMode
                ? "Haven 智能体 · 本地预览：尚未接入完整应用层，批准只会生成预览回执，不会写入任何设置。"
                : "Haven 智能体 · 读取本机设置快照后给出提案，写入前需要你逐项批准。"}
            </p>
          </div>
          <button
            type="button"
            aria-label="关闭栖伴"
            onClick={() => onOpenChange(false)}
            className="flex h-[32px] w-[32px] shrink-0 items-center justify-center rounded-full text-[#6e6e73] transition-colors hover:bg-black/[0.05] hover:text-[#1d1d1f] dark:text-[#98989d] dark:hover:bg-white/[0.08] dark:hover:text-[#f5f5f5]"
          >
            <X className="h-[17px] w-[17px]" strokeWidth={2} />
          </button>
        </header>

        {issue && (
          <div
            role="alert"
            aria-label={issue.title}
            className={cn(
              "flex shrink-0 items-start gap-2.5 border-b px-5 py-3 text-[12px] leading-5 sm:px-6",
              issue.tone === "danger"
                ? "border-[#ff3b30]/25 bg-[#fdecea] text-[#a3231a] dark:border-[#ff453a]/30 dark:bg-[#3a1a18] dark:text-[#ff8a80]"
                : "border-[#f0b429]/30 bg-[#fff8e5] text-[#7a5a1a] dark:border-[#f0b429]/30 dark:bg-[#3a2f12] dark:text-[#f0b429]",
            )}
          >
            {issue.tone === "danger"
              ? <CircleX className="mt-0.5 h-[15px] w-[15px] shrink-0" strokeWidth={2.2} />
              : <TriangleAlert className="mt-0.5 h-[15px] w-[15px] shrink-0" strokeWidth={2.2} />}
            <p className="min-w-0">
              <span className="font-semibold">{issue.title}</span>
              <span className="mt-0.5 block">{issue.message}</span>
              <span className="mt-0.5 block">{issue.detail}</span>
            </p>
          </div>
        )}

        <div className="flex min-h-0 flex-1 flex-col sm:flex-row">
          {/* 竖向分区 rail：桌面固定约 172px 并与右侧内容区分隔，窄屏降级为可横向滚动的分区条。 */}
          <div
            role="tablist"
            aria-label="栖伴分区"
            aria-orientation="vertical"
            onKeyDown={handleTabKeyDown}
            className="settings-scrollbar-hidden flex shrink-0 items-center gap-1 overflow-x-auto border-b border-black/[0.06] px-4 py-2 dark:border-white/[0.06] sm:w-[172px] sm:flex-col sm:items-stretch sm:overflow-visible sm:border-b-0 sm:border-r sm:px-3 sm:py-4"
          >
            {VIEW_TABS.map((tab) => {
              const isActive = tab.id === view
              return (
                <button
                  key={tab.id}
                  ref={(node) => {
                    tabButtonRefs.current[tab.id] = node
                  }}
                  type="button"
                  role="tab"
                  id={`${titleId}-tab-${tab.id}`}
                  aria-selected={isActive}
                  aria-controls={`${titleId}-panel-${tab.id}`}
                  tabIndex={isActive ? 0 : -1}
                  onClick={() => setView(tab.id)}
                  className={cn(
                    "flex h-[34px] shrink-0 items-center gap-2.5 rounded-xl px-3 text-[13px] font-medium transition-colors sm:w-full",
                    isActive
                      ? "bg-[#007aff]/[0.12] text-[#007aff]"
                      : "text-[#6e6e73] hover:bg-black/[0.04] hover:text-[#1d1d1f] dark:text-[#98989d] dark:hover:bg-white/[0.06] dark:hover:text-[#f5f5f5]",
                  )}
                >
                  {tab.icon}
                  {tab.label}
                </button>
              )
            })}
          </div>

          <div className="flex min-h-0 min-w-0 flex-1 flex-col">
            {view === "conversation" && (
              <div
                role="tabpanel"
                id={`${titleId}-panel-conversation`}
                aria-labelledby={`${titleId}-tab-conversation`}
                className="flex min-h-0 flex-1 flex-col"
              >
                {messagesPane}
                {composer}
              </div>
            )}

            {view === "trace" && (
              <div
                role="tabpanel"
                id={`${titleId}-panel-trace`}
                aria-labelledby={`${titleId}-tab-trace`}
                className="flex min-h-0 flex-1 flex-col lg:flex-row"
              >
                <div className="flex min-h-0 flex-1 flex-col">
                  {messagesPane}
                  {composer}
                </div>
                <aside className="settings-scrollbar-hidden max-h-[46%] shrink-0 overflow-y-auto border-t border-black/[0.06] px-5 py-4 dark:border-white/[0.06] lg:max-h-none lg:w-[330px] lg:border-l lg:border-t-0 lg:px-5 lg:py-5">
                  {traceInspector}
                </aside>
              </div>
            )}

            {view === "proposals" && (
              <div
                role="tabpanel"
                id={`${titleId}-panel-proposals`}
                aria-labelledby={`${titleId}-tab-proposals`}
                className="settings-scrollbar-hidden min-h-0 flex-1 overflow-y-auto px-5 py-5 sm:px-6"
              >
                <ProposalHistory
                  proposals={state.proposals}
                  onSelect={() => setView("conversation")}
                />
              </div>
            )}
          </div>
        </div>
      </div>
    </div>,
    document.body,
  )
}

function ProposalAttachment({
  record,
  busy,
  onViewTrace,
  onReject,
  onApprove,
}: {
  record: AiAssistantProposalRecord
  busy: boolean
  onViewTrace: () => void
  /** 参数是被点击卡片的 proposalId：旧卡片只能操作它自己。 */
  onReject: (proposalId: string) => Promise<void>
  onApprove: (proposalId: string) => Promise<void>
}) {
  const isPending = record.status === "pending"
  // 卡片结论：pending / rejected 之外的每一类都在服务端确定了"本次没有写入"，
  // 只有 failed 是未确认写入——文案逐字来自 PROPOSAL_STATUS_NOTE。
  const statusNote = PROPOSAL_STATUS_NOTE[record.status]
  return (
    <section
      aria-label="设置提案"
      className="w-full rounded-2xl border border-black/[0.06] bg-white p-4 text-left dark:border-white/[0.08] dark:bg-[#2c2c2e]"
    >
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="text-[13px] font-semibold">{record.title}</p>
          {record.mock && (
            <p
              role="note"
              className="mt-1 inline-flex items-center gap-1.5 rounded-full bg-[#f0b429]/[0.18] px-2.5 py-0.5 text-[11px] font-semibold text-[#b7791f] dark:text-[#f0b429]"
            >
              Mock/预览 · 非真实模型输出
            </p>
          )}
          <p className="mt-1 text-[11px] text-[#86868b] dark:text-[#98989d]">
            {`作用域 ${record.targetLabel} · digest ${digestDisplay(record)}`}
          </p>
          <p className="mt-0.5 text-[11px] text-[#86868b] dark:text-[#98989d]">
            基线版本 {record.expectedRevision ?? "未提供（本地预览）"}
          </p>
        </div>
        <span
          className={cn(
            "shrink-0 rounded-full px-2.5 py-1 text-[11px] font-semibold",
            record.status === "pending"
              ? "bg-[#007aff]/[0.12] text-[#007aff]"
              : record.status === "applied"
                ? "bg-[#34c759]/[0.16] text-[#248a3d] dark:text-[#30d158]"
                : record.status === "rejected"
                  ? "bg-black/[0.05] text-[#6e6e73] dark:bg-white/[0.08] dark:text-[#98989d]"
                  : "bg-[#f0b429]/[0.18] text-[#b7791f] dark:text-[#f0b429]",
          )}
        >
          {PROPOSAL_STATUS_LABEL[record.status]}
        </span>
      </div>

      <ul className="mt-3 space-y-1.5">
        {record.changes.map((change) => (
          <li
            key={change.key}
            className="flex flex-wrap items-center justify-between gap-2 rounded-xl bg-black/[0.025] px-3 py-2 dark:bg-white/[0.04]"
          >
            <span className="text-[13px] font-medium">{change.label}</span>
            <span className="flex items-center gap-2 text-[12px]">
              <span className="rounded-md bg-black/[0.05] px-2 py-0.5 text-[#6e6e73] dark:bg-white/[0.08] dark:text-[#98989d]">
                {change.from}
              </span>
              <ArrowRight className="h-3.5 w-3.5 text-[#86868b]" strokeWidth={2} />
              <span className="rounded-md bg-[#007aff]/[0.12] px-2 py-0.5 font-semibold text-[#007aff]">{change.to}</span>
            </span>
          </li>
        ))}
      </ul>

      {record.status === "rejected" && (
        <p className="mt-3 flex items-center gap-2 rounded-xl bg-black/[0.03] px-3 py-2 text-[12px] text-[#6e6e73] dark:bg-white/[0.05] dark:text-[#98989d]">
          <ShieldCheck className="h-[14px] w-[14px] shrink-0" strokeWidth={2} />
          本次没有写入任何设置。
        </p>
      )}

      {statusNote && (
        <p
          role="alert"
          className="mt-3 flex items-start gap-2 rounded-xl border border-[#f0b429]/40 bg-[#fff8e5] px-3 py-2 text-[12px] leading-5 text-[#7a5a1a] dark:bg-[#3a2f12] dark:text-[#f0b429]"
        >
          <TriangleAlert className="mt-0.5 h-[14px] w-[14px] shrink-0" strokeWidth={2} />
          <span>{statusNote}</span>
        </p>
      )}

      {/*
        卡片级 notice 与 status 正交：拒绝失败时提案仍然是 pending（服务端也是 pending），
        所以按钮保留、可重试，只是额外说明"这次没有写入"。
      */}
      {record.notice && (
        <div
          role="alert"
          className={cn(
            "mt-3 flex items-start gap-2 rounded-xl border px-3 py-2 text-[12px] leading-5",
            record.notice.tone === "danger"
              ? "border-[#ff3b30]/30 bg-[#fdecea] text-[#a3231a] dark:border-[#ff453a]/30 dark:bg-[#3a1a18] dark:text-[#ff8a80]"
              : "border-[#f0b429]/40 bg-[#fff8e5] text-[#7a5a1a] dark:bg-[#3a2f12] dark:text-[#f0b429]",
          )}
        >
          {record.notice.tone === "danger"
            ? <CircleX className="mt-0.5 h-[14px] w-[14px] shrink-0" strokeWidth={2} />
            : <TriangleAlert className="mt-0.5 h-[14px] w-[14px] shrink-0" strokeWidth={2} />}
          <span>
            <span className="font-semibold">{record.notice.title}</span>
            <span className="mt-0.5 block">{record.notice.message}</span>
            <span className="mt-0.5 block">{record.notice.detail}</span>
          </span>
        </div>
      )}

      {record.receipt && <ProposalReceipt receipt={record.receipt} record={record} />}

      <div className="mt-3 flex flex-wrap items-center gap-2">
        <button
          type="button"
          onClick={onViewTrace}
          className="rounded-full border border-black/[0.08] px-3 py-1.5 text-[12px] font-medium text-[#6e6e73] transition-colors hover:text-[#007aff] dark:border-white/[0.1] dark:text-[#98989d]"
        >
          查看执行轨迹
        </button>
        {isPending && (
          <>
            <button
              type="button"
              disabled={busy}
              onClick={() => { void onReject(record.id) }}
              className="rounded-full border border-black/[0.08] px-3 py-1.5 text-[12px] font-semibold text-[#d70015] transition-colors hover:bg-[#d70015]/[0.06] disabled:cursor-not-allowed disabled:opacity-50 dark:border-white/[0.1]"
            >
              拒绝
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() => { void onApprove(record.id) }}
              className="rounded-full bg-[#007aff] px-4 py-1.5 text-[12px] font-semibold text-white transition-colors hover:bg-[#006fe6] disabled:cursor-not-allowed disabled:opacity-50"
            >
              批准并应用
            </button>
          </>
        )}
      </div>
    </section>
  )
}

function ProposalReceipt({ receipt, record }: { receipt: AiAssistantReceipt; record: AiAssistantProposalRecord }) {
  return (
    <div className="mt-3 rounded-xl border border-[#34c759]/25 bg-[#edf8f0] px-3 py-3 dark:border-[#34c759]/25 dark:bg-[#16281c]">
      <p className="flex items-center gap-2 text-[12px] font-semibold text-[#216e32] dark:text-[#30d158]">
        <Receipt className="h-[14px] w-[14px]" strokeWidth={2} />
        Setting Change Receipt
      </p>
      <dl className="mt-2 grid grid-cols-2 gap-x-3 gap-y-1 text-[11px]">
        <div className="flex items-center gap-1">
          <dt className="text-[#4f7659] dark:text-[#7ec79a]">状态</dt>
          <dd className="font-semibold text-[#216e32] dark:text-[#30d158]">
            {receipt.preview ? "预览完成（未写入）" : "已应用"}
          </dd>
        </div>
        <div className="flex items-center gap-1">
          <dt className="text-[#4f7659] dark:text-[#7ec79a]">改动</dt>
          <dd className="font-semibold text-[#216e32] dark:text-[#30d158]">{receipt.changedCount} 项</dd>
        </div>
        <div className="flex items-center gap-1">
          <dt className="text-[#4f7659] dark:text-[#7ec79a]">作用域</dt>
          <dd className="font-semibold text-[#216e32] dark:text-[#30d158]">{record.targetLabel}</dd>
        </div>
        <div className="flex items-center gap-1">
          <dt className="text-[#4f7659] dark:text-[#7ec79a]">版本</dt>
          <dd className="font-semibold text-[#216e32] dark:text-[#30d158]">{receipt.revision}</dd>
        </div>
      </dl>
      {receipt.receiptId && (
        <p className="mt-2 break-all text-[11px] leading-5 text-[#4f7659] dark:text-[#7ec79a]">
          {`回执 ${receipt.receiptId}`}
          {receipt.digest ? ` · digest ${receipt.digest}` : ""}
        </p>
      )}
      <p className="mt-2 text-[11px] leading-5 text-[#4f7659] dark:text-[#7ec79a]">
        {receipt.preview
          ? `Mock/预览回执 · 本阶段没有可用的模型供应商，${receipt.appliedAt} 的这次批准没有写入本机设置。`
          : `写入时间 ${receipt.appliedAt}`}
      </p>
    </div>
  )
}

function ProposalHistory({
  proposals,
  onSelect,
}: {
  proposals: AiAssistantProposalRecord[]
  onSelect: () => void
}) {
  if (proposals.length === 0) {
    return (
      <p className="rounded-2xl border border-dashed border-black/[0.1] px-4 py-6 text-center text-[13px] text-[#86868b] dark:border-white/[0.12] dark:text-[#98989d]">
        还没有设置提案。回到对话，让栖伴先生成一份提案。
      </p>
    )
  }
  return (
    <ul aria-label="提案记录" className="space-y-2">
      {proposals.map((proposal) => (
        <li key={proposal.id}>
          <button
            type="button"
            onClick={onSelect}
            className="flex w-full items-center gap-3 rounded-xl border border-black/[0.06] bg-white px-4 py-3 text-left transition-colors hover:border-[#007aff]/35 dark:border-white/[0.08] dark:bg-[#2c2c2e]"
          >
            <span
              className={cn(
                "shrink-0 rounded-full px-2.5 py-1 text-[11px] font-semibold",
                proposal.status === "pending"
                  ? "bg-[#007aff]/[0.12] text-[#007aff]"
                  : proposal.status === "applied"
                    ? "bg-[#34c759]/[0.16] text-[#248a3d] dark:text-[#30d158]"
                    : proposal.status === "rejected"
                      ? "bg-black/[0.05] text-[#6e6e73] dark:bg-white/[0.08] dark:text-[#98989d]"
                      : "bg-[#f0b429]/[0.18] text-[#b7791f] dark:text-[#f0b429]",
              )}
            >
              {PROPOSAL_STATUS_LABEL[proposal.status]}
            </span>
            <span className="min-w-0 flex-1">
              <span className="block truncate text-[13px] font-medium">{proposal.title}</span>
              <span className="mt-0.5 block truncate text-[11px] text-[#86868b] dark:text-[#98989d]">
                {`${proposal.targetLabel} · ${proposal.changes.length} 项改动 · digest ${digestDisplay(proposal)}`}
              </span>
            </span>
            <ChevronRight className="h-[15px] w-[15px] shrink-0 text-[#c7c7cc] dark:text-[#636366]" strokeWidth={2} />
          </button>
        </li>
      ))}
    </ul>
  )
}

function TraceInspector({
  steps,
  statuses,
  running,
}: {
  steps: AiAssistantTraceStep[]
  statuses: AiAssistantTraceNodeStatus[]
  running: boolean
}) {
  return (
    <div className="min-w-0">
      <p className="text-[11px] font-semibold uppercase tracking-[0.12em] text-[#86868b] dark:text-[#98989d]">执行轨迹</p>
      <p className="mt-1 text-[11px] leading-5 text-[#86868b] dark:text-[#8e8e93]">
        只读步骤记录，不含查询语句、本机路径或凭据。
      </p>
      <ol aria-label="执行轨迹节点" className="mt-4">
        {steps.map((step, index) => (
          <TraceNode
            key={step.id}
            step={step}
            status={statuses[index] ?? "pending"}
            isLast={index === steps.length - 1}
            spinning={running && statuses[index] === "active"}
          />
        ))}
      </ol>
      {statuses.every((status) => status === "pending") && (
        <p className="mt-3 flex items-center gap-2 text-[11px] text-[#86868b] dark:text-[#8e8e93]">
          <CircleAlert className="h-[13px] w-[13px] shrink-0" strokeWidth={2} />
          还没有执行记录。在对话中提交需求后，这里会逐步显示读取与提案节点。
        </p>
      )}
    </div>
  )
}

function TraceNode({
  step,
  status,
  isLast,
  spinning,
}: {
  step: AiAssistantTraceStep
  status: AiAssistantTraceNodeStatus
  isLast: boolean
  spinning: boolean
}) {
  const tone = NODE_TONE[status]
  return (
    <li aria-label={`${step.title}：${TRACE_NODE_STATUS_LABEL[status]}`} className="relative flex gap-3 pb-4 last:pb-0">
      {!isLast && (
        <span aria-hidden="true" className={cn("absolute left-[13px] top-[28px] h-[calc(100%-28px)] w-[2px] rounded-full", tone.connector)} />
      )}
      <span className={cn("relative z-[1] flex h-[28px] w-[28px] shrink-0 items-center justify-center rounded-full border", tone.node)}>
        {status === "complete" && <Check className="h-[14px] w-[14px]" strokeWidth={2.6} />}
        {status === "active" && (spinning
          ? <Loader2 className="h-[14px] w-[14px] animate-spin" strokeWidth={2.2} />
          : <Sparkles className="h-[14px] w-[14px]" strokeWidth={2.2} />)}
        {status === "pending" && <span aria-hidden="true" className="h-1.5 w-1.5 rounded-full bg-current" />}
        {status === "conflict" && <TriangleAlert className="h-[14px] w-[14px]" strokeWidth={2.4} />}
        {status === "expired" && <TriangleAlert className="h-[14px] w-[14px]" strokeWidth={2.4} />}
        {status === "invalid" && <TriangleAlert className="h-[14px] w-[14px]" strokeWidth={2.4} />}
        {status === "closed" && <X className="h-[13px] w-[13px]" strokeWidth={2.6} />}
        {status === "failed" && <CircleX className="h-[14px] w-[14px]" strokeWidth={2.4} />}
        {status === "skipped" && <X className="h-[13px] w-[13px]" strokeWidth={2.6} />}
      </span>
      <div className="min-w-0 flex-1 pt-0.5">
        <p className={cn("text-[13px] font-semibold", tone.title)}>
          {step.title}
          <span className="sr-only">（{TRACE_NODE_STATUS_LABEL[status]}）</span>
        </p>
        <p className="mt-0.5 text-[11px] leading-5 text-[#6e6e73] dark:text-[#98989d]">{step.description}</p>
        {step.meta.length > 0 && (
          <dl className="mt-1.5 flex flex-wrap gap-x-3 gap-y-1">
            {step.meta.map((entry) => (
              <div key={entry.label} className="flex items-center gap-1 text-[10px]">
                <dt className="text-[#86868b] dark:text-[#8e8e93]">{entry.label}</dt>
                <dd className="font-medium text-[#6e6e73] dark:text-[#98989d]">{entry.value}</dd>
              </div>
            ))}
          </dl>
        )}
      </div>
    </li>
  )
}
