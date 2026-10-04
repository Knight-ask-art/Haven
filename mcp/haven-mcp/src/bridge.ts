// Typed Haven Agent Bridge —— MCP 与栖阅之间的**唯一**通道。
//
// 规范：docs/architecture/AI_SYSTEM.md §5、§10（阶段划分）。
//
// 边界（违反其中任何一条都属缺陷）：
// - 本端口**没有** Apply / Approve / Reject / Delete / 任意写入方法。审批与执行只在
//   Haven 自己的 UI 上发生；MCP 侧能做的写操作只有"创建一条 pending 提案"。
// - 本端口**不读** SQLite、文件系统或 CredentialStore。存储与凭据只存在于 Haven 进程内；
//   桥接传输只承载已经投影、已经脱敏的载荷。
// - 本端口**不**invoke Tauri。它是传输无关的接口；具体传输由显式不可用适配器或
//   本地 Broker 适配器提供。
//
// ## 当前接通状态（本切片）
//
// 生产传输由 `LiveHavenAgentBridge` 通过本地 Broker 接入；默认仍是显式不可用的
// [`UnavailableHavenAgentBridge`]。桥接只承载已经由 Haven Application service 投影的
// 读结果或 pending Proposal，不拥有 Apply / Approve / 文件 / SQL / secret 能力。

import { BRIDGE_TIMEOUT_MS } from "./constants.js";
import { bridgeCancelled, bridgeTimeout, bridgeUnavailable } from "./errors.js";

/** 桥接类型。生产默认是 `unavailable`；用户显式配置本地 Broker 后可为 `live`。 */
export type BridgeKind = "unavailable" | "fixture" | "live";

/**
 * 一次桥接调用的传输上下文。
 *
 * 目前只有一个字段：MCP 客户端取消（`notifications/cancelled`）时触发的 `AbortSignal`。
 * 传输适配器必须**响应**它——收到取消就立刻释放本地连接，而不是继续等到超时；否则
 * 取消只是一个"客户端不再看结果"的假象，Broker 侧的工作照旧占着连接与配额。
 */
export interface BridgeCallContext {
  signal?: AbortSignal;
}

/** 全局设置分区（当前只有 reading，与 `AgentSettingsSection` 一致）。 */
export type SettingsSection = "reading";

/** 资源偏好作用域。 */
export type PreferenceScope = "edition" | "media_item";

export interface ReadingPatch {
  font_family?: string | null;
  custom_font_family?: string | null;
  font_size?: string | null;
  line_height?: string | null;
  content_width?: string | null;
  theme?: string | null;
  custom_background?: string | null;
  custom_text?: string | null;
  font_weight?: string | null;
  letter_spacing?: string | null;
  system_auto?: boolean | null;
  pagination?: string | null;
}

export interface ComicPatch {
  view_mode?: string | null;
  direction?: string | null;
  page_gap?: string | null;
  preload_pages?: string | null;
}

export interface ResourcePreferencePatch {
  reading?: ReadingPatch | null;
  comic?: ComicPatch | null;
}

/** 提案创建结果。**刻意只有这些字段**：没有任何 token/secret/路径/SQL 的位置。 */
export interface ProposalOutcome {
  proposal_id: string;
  status: "pending";
  /** canonical JSON 的 SHA-256（64 位小写十六进制）；UI 与客户端据此核对同一份提案。 */
  digest: string;
  target_label: string;
  subject_scope: "global" | "edition" | "media_item";
  section: SettingsSection | null;
  edition_id: string | null;
  media_item_id: string | null;
  base_revision: string | null;
  created_at: string;
  expires_at: string;
  changes: { key: string; before: string; after: string }[];
}

/** Haven 侧能力清单（与 `AgentCapabilityManifest` 同形，snake_case 投影）。 */
export interface HavenCapabilityReport {
  agent_api_version: number;
  capabilities: {
    settings_read: boolean;
    settings_proposal: boolean;
    library_summary_read: boolean;
    setting_sources_read: boolean;
    resource_preference_read: boolean;
    resource_preference_proposal: boolean;
    media_capabilities_read: boolean;
    onboarding_read: boolean;
    metadata_proposal: boolean;
    rename_proposal: boolean;
    secret_read: boolean;
    filesystem_write: boolean;
  };
}

export interface SettingsSnapshotResult {
  subject_scope: "global";
  section: SettingsSection;
  context_id: string;
  context_hash: string;
  revision: string | null;
  settings: Record<string, unknown>;
  redacted_fields: string[];
}

export interface SettingSourceLayer {
  layer: "default" | "global" | "edition" | "media_item";
  present: boolean;
  revision: string | null;
}

export interface SettingSourcesResult {
  section: SettingsSection;
  revision: string | null;
  layers: SettingSourceLayer[];
}

export interface ResourcePreferenceSnapshotResult {
  context_id: string;
  context_hash: string;
  target_scope: PreferenceScope;
  edition_id: string;
  media_item_id: string | null;
  revision: string | null;
  reading: Record<string, unknown> | null;
  comic: Record<string, unknown> | null;
}

export interface LibrarySummaryResult {
  counts: {
    works: number;
    editions: number;
    media_items: number;
    favorites: number;
    in_progress: number;
  };
  categories: { category: string; work_count: number }[];
  recent: {
    work_id: string;
    title: string;
    category: string;
    progress_ratio: number | null;
  }[];
  truncated: boolean;
}

export interface MediaCapabilitiesResult {
  items: {
    media_item_id: string;
    media_type: string;
    availability: string;
    can_open_session: boolean;
    can_extract_text: boolean;
    can_render_pages: boolean;
    declared_capabilities: string[];
  }[];
  truncated: boolean;
}

export interface OnboardingStateResult {
  completed_steps: string[];
  next_step: string | null;
  has_storage_location: boolean;
  has_library_content: boolean;
  has_ai_provider: boolean;
}

export interface SettingsProposalRequest {
  section: SettingsSection;
  context_id: string;
  context_hash: string;
  base_revision: string | null;
  patch: ReadingPatch;
}

export interface ResourcePreferenceProposalRequest {
  target_scope: PreferenceScope;
  edition_id: string;
  media_item_id: string | null;
  context_id: string;
  context_hash: string;
  base_revision: string | null;
  patch: ResourcePreferencePatch;
}

/**
 * Typed Haven Agent Bridge。
 *
 * 实现方必须先通过 `assertBridgeResult` 之类的方式自证返回形状；本文件提供的
 * [`withBridgeTimeout`] 是唯一允许的调用包装，用来把"无限等待"变成稳定超时错误，
 * 并把客户端取消传下去。
 *
 * 每个方法的最后一个可选参数都是 [`BridgeCallContext`]（当前只承载取消信号）。
 * 允许实现方忽略它——但真实传输实现必须响应：收到取消就断开本地连接。
 */
export interface HavenAgentBridge {
  readonly kind: BridgeKind;
  /** 是否已经接通真实运行时。`unavailable` 时恒为 false。 */
  readonly available: boolean;
  /** 人类可读的当前状态说明（会进 `get_system_capabilities`）。 */
  readonly statusDetail: string;

  getCapabilityManifest(context?: BridgeCallContext): Promise<HavenCapabilityReport>;
  getSettingsSnapshot(context?: BridgeCallContext): Promise<SettingsSnapshotResult>;
  getSettingSources(context?: BridgeCallContext): Promise<SettingSourcesResult>;
  getResourcePreferenceSnapshot(
    request: {
      target_scope: PreferenceScope;
      edition_id: string;
      media_item_id: string | null;
    },
    context?: BridgeCallContext,
  ): Promise<ResourcePreferenceSnapshotResult>;
  getLibrarySummary(
    request: { limit: number },
    context?: BridgeCallContext,
  ): Promise<LibrarySummaryResult>;
  getMediaCapabilities(
    request: { media_item_id: string | null; limit: number },
    context?: BridgeCallContext,
  ): Promise<MediaCapabilitiesResult>;
  getOnboardingState(context?: BridgeCallContext): Promise<OnboardingStateResult>;
  proposeSettingsPatch(
    request: SettingsProposalRequest,
    context?: BridgeCallContext,
  ): Promise<ProposalOutcome>;
  proposeResourcePreferencePatch(
    request: ResourcePreferenceProposalRequest,
    context?: BridgeCallContext,
  ): Promise<ProposalOutcome>;
}

/**
 * 生产默认桥接：**显式不可用**。
 *
 * 它不返回空列表、不返回零计数、不返回任何"看起来像数据"的东西——那些都会被客户端
 * 当成真实答案。每个操作都抛出稳定的 `HAVEN_BRIDGE_UNAVAILABLE`。
 */
export class UnavailableHavenAgentBridge implements HavenAgentBridge {
  readonly kind: BridgeKind = "unavailable";
  readonly available = false;

  constructor(
    readonly statusDetail: string = "本构建没有接通 Haven 运行时桥接（尚无本地传输适配器）。",
  ) {}

  private refuse(operation: string): never {
    throw bridgeUnavailable(
      `${operation} 不可用：${this.statusDetail} 请改用 Haven 应用内的界面完成需要写操作或本机数据的任务。`,
    );
  }

  async getCapabilityManifest(): Promise<HavenCapabilityReport> {
    this.refuse("读取 Haven 能力清单");
  }

  async getSettingsSnapshot(): Promise<SettingsSnapshotResult> {
    this.refuse("读取设置快照");
  }

  async getSettingSources(): Promise<SettingSourcesResult> {
    this.refuse("读取设置来源");
  }

  async getResourcePreferenceSnapshot(): Promise<ResourcePreferenceSnapshotResult> {
    this.refuse("读取资源偏好快照");
  }

  async getLibrarySummary(): Promise<LibrarySummaryResult> {
    this.refuse("读取媒体库摘要");
  }

  async getMediaCapabilities(): Promise<MediaCapabilitiesResult> {
    this.refuse("读取条目能力");
  }

  async getOnboardingState(): Promise<OnboardingStateResult> {
    this.refuse("读取引导状态");
  }

  async proposeSettingsPatch(): Promise<ProposalOutcome> {
    this.refuse("创建设置提案");
  }

  async proposeResourcePreferencePatch(): Promise<ProposalOutcome> {
    this.refuse("创建资源偏好提案");
  }
}

/**
 * 给桥接调用套上超时**与取消**。
 *
 * 没有超时的桥接会让 MCP 客户端无限挂起；没有取消的桥接会让"用户按了停止"变成一个
 * 只有客户端一侧生效的姿态。两者都被归一为稳定错误码，且**重试语义由调用方按副作用
 * 决定**（见 [`BridgeTimeoutOptions.retryableOnCancel`]）：只读操作可以重试，提案创建
 * 不可以——它可能已经落库。
 *
 * `operation` 只用于错误文案，必须是不含用户数据的固定短语。
 */
export interface BridgeTimeoutOptions {
  timeoutMs?: number;
  /**
   * 调用方取消信号。触发时本函数立刻以 `HAVEN_BRIDGE_CANCELLED` 收尾，并把这个信号
   * 转发给 `call`，让传输层能销毁底层连接、释放工作。
   */
  signal?: AbortSignal;
  /**
   * 取消 / 超时之后是否可以安全重试。
   *
   * 默认 `false`（保守）：不知道副作用的调用方不该被鼓励重试。只读工具显式传 `true`。
   */
  retryableOnCancel?: boolean;
}

export async function withBridgeTimeout<T>(
  operation: string,
  call: (signal: AbortSignal) => Promise<T>,
  options: BridgeTimeoutOptions = {},
): Promise<T> {
  const timeoutMs = options.timeoutMs ?? BRIDGE_TIMEOUT_MS;
  const retryable = options.retryableOnCancel === true;

  // 一个内部控制器的两个用途：把"调用方取消"和"本地超时"合流给 `call`，让传输层
  // 只需要监听一个信号；同时保证超时也会断开底层连接，而不是留一个孤儿请求。
  const controller = new AbortController();
  const forwardAbort = (): void => controller.abort();
  if (options.signal?.aborted) {
    controller.abort();
  } else {
    options.signal?.addEventListener("abort", forwardAbort, { once: true });
  }

  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      call(controller.signal),
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(() => {
          // 先让超时错误成为 race 的第一个落定值，再中止底层连接。顺序反过来的话，
          // abort 监听器会同步抢先 reject，用户看到的就会是"被取消"而不是"超时"。
          reject(bridgeTimeout(operation, retryable));
          controller.abort();
        }, timeoutMs);
      }),
      new Promise<never>((_resolve, reject) => {
        const onAbort = (): void => {
          reject(bridgeCancelled(operation, !retryable));
        };
        if (controller.signal.aborted) {
          onAbort();
          return;
        }
        controller.signal.addEventListener("abort", onAbort, { once: true });
      }),
    ]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
    options.signal?.removeEventListener("abort", forwardAbort);
  }
}
