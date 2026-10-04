// 测试替身桥接。
//
// **只存在于 test/ 目录**，生产入口（`src/index.ts`）不引用它，`selectBridge` 在
// 没有注入工厂时也会拒绝 fixture 模式。这是刻意的：一个能被环境变量打开、
// 返回很像真数据的假桥接，迟早会有人拿它当"生产降级路径"。
//
// 它的职责是让"零写入""只创建 pending 提案""脱敏"这些性质**可被观察到**：
// - `mutationCount` 只在 propose* 里自增，因此任何一次只读调用都不可能偷偷改状态；
// - `calls` 记录每个被调用的端口方法，便于断言调用路径。

import type {
  HavenAgentBridge,
  HavenCapabilityReport,
  LibrarySummaryResult,
  MediaCapabilitiesResult,
  OnboardingStateResult,
  ProposalOutcome,
  ResourcePreferencePatch,
  ResourcePreferenceProposalRequest,
  ResourcePreferenceSnapshotResult,
  SettingsProposalRequest,
  SettingsSnapshotResult,
  SettingSourcesResult,
} from "../../src/bridge.js";
import { HavenMcpError } from "../../src/errors.js";

const DIGEST_A = "a".repeat(64);
const UUID_EDITION = "0196f0d2-0000-7000-8000-0000000000e1";
const UUID_MEDIA_ITEM = "0196f0d2-0000-7000-8000-0000000000d1";
const UUID_CONTEXT = "0196f0d2-0000-7000-8000-0000000000c1";

/** 生成一个形状合法的提案 id（v7 风格 UUID）。 */
function proposalId(prefix: string, index: number): string {
  return `0196f0d2-0000-7000-8000-${prefix}${String(index).padStart(11, "0")}`;
}

export class FixtureHavenAgentBridge implements HavenAgentBridge {
  readonly kind = "fixture" as const;
  readonly available = true;
  readonly statusDetail = "测试替身桥接（仅测试）。";

  /** 只有 propose* 会自增；任何只读调用让它保持不变。 */
  mutationCount = 0;
  readonly calls: string[] = [];
  readonly proposals: ProposalOutcome[] = [];

  /** 设成非 null 时，下一个端口调用抛出它（用于验证稳定错误映射）。 */
  failure: HavenMcpError | null = null;

  /** 让列表型工具返回超长数据，用于验证响应上限。 */
  bulkyLibrarySummary = false;

  private revision = "rev-1";
  private readonly reading: Record<string, unknown> = {
    font_family: "sans",
    custom_font_family: null,
    font_size: "medium",
    line_height: "comfortable",
    content_width: "medium",
    theme: "system",
    custom_background: null,
    custom_text: null,
    font_weight: "regular",
    letter_spacing: "normal",
    system_auto: true,
    pagination: "scroll",
  };

  private guard(operation: string): void {
    this.calls.push(operation);
    if (this.failure !== null) {
      const error = this.failure;
      this.failure = null;
      throw error;
    }
  }

  async getCapabilityManifest(): Promise<HavenCapabilityReport> {
    this.guard("getCapabilityManifest");
    return {
      agent_api_version: 1,
      capabilities: {
        settings_read: true,
        settings_proposal: true,
        library_summary_read: true,
        setting_sources_read: true,
        resource_preference_read: true,
        resource_preference_proposal: true,
        media_capabilities_read: true,
        onboarding_read: true,
        metadata_proposal: false,
        rename_proposal: false,
        secret_read: false,
        filesystem_write: false,
      },
    };
  }

  async getSettingsSnapshot(): Promise<SettingsSnapshotResult> {
    this.guard("getSettingsSnapshot");
    return {
      subject_scope: "global",
      section: "reading",
      context_id: UUID_CONTEXT,
      context_hash: DIGEST_A,
      revision: this.revision,
      settings: { ...this.reading },
      redacted_fields: [],
    };
  }

  async getSettingSources(): Promise<SettingSourcesResult> {
    this.guard("getSettingSources");
    return {
      section: "reading",
      revision: this.revision,
      layers: [
        { layer: "default", present: true, revision: null },
        { layer: "global", present: true, revision: this.revision },
      ],
    };
  }

  async getResourcePreferenceSnapshot(): Promise<ResourcePreferenceSnapshotResult> {
    this.guard("getResourcePreferenceSnapshot");
    return {
      context_id: UUID_CONTEXT,
      context_hash: DIGEST_A,
      target_scope: "media_item",
      edition_id: UUID_EDITION,
      media_item_id: UUID_MEDIA_ITEM,
      revision: this.revision,
      reading: { font_size: "medium" },
      comic: { view_mode: "single" },
    };
  }

  async getLibrarySummary({ limit }: { limit: number }): Promise<LibrarySummaryResult> {
    this.guard("getLibrarySummary");
    const total = this.bulkyLibrarySummary ? 5_000 : 3;
    const recent = Array.from({ length: Math.min(total, limit) }, (_value, index) => ({
      work_id: `0196f0d2-0000-7000-8000-0000000000${String(index).padStart(2, "0")}`,
      title: this.bulkyLibrarySummary ? "很长的标题".repeat(40) : `作品 ${index}`,
      category: "book",
      progress_ratio: index === 0 ? 0.42 : null,
    }));
    return {
      counts: {
        works: total,
        editions: total,
        media_items: total,
        favorites: 1,
        in_progress: 1,
      },
      categories: [{ category: "book", work_count: total }],
      recent,
      truncated: false,
    };
  }

  async getMediaCapabilities({
    limit,
  }: {
    media_item_id: string | null;
    limit: number;
  }): Promise<MediaCapabilitiesResult> {
    this.guard("getMediaCapabilities");
    return {
      items: Array.from({ length: Math.min(limit, 2) }, (_value, index) => ({
        media_item_id: `0196f0d2-0000-7000-8000-0000000001${String(index).padStart(2, "0")}`,
        media_type: "book",
        availability: "available",
        can_open_session: true,
        can_extract_text: true,
        can_render_pages: false,
        declared_capabilities: ["text"],
      })),
      truncated: false,
    };
  }

  async getOnboardingState(): Promise<OnboardingStateResult> {
    this.guard("getOnboardingState");
    return {
      completed_steps: ["storage_location"],
      next_step: "scan_library",
      has_storage_location: true,
      has_library_content: false,
      has_ai_provider: false,
    };
  }

  async proposeSettingsPatch(request: SettingsProposalRequest): Promise<ProposalOutcome> {
    this.guard("proposeSettingsPatch");
    this.mutationCount += 1;
    const changes = Object.entries(request.patch)
      .filter(([, value]) => value !== undefined && value !== null)
      .map(([key, value]) => ({
        key: `reading.${key}`,
        before: String(this.reading[key] ?? "null"),
        after: String(value),
      }));
    const outcome: ProposalOutcome = {
      proposal_id: proposalId("1", this.mutationCount),
      status: "pending",
      digest: DIGEST_A,
      target_label: "全局 · 阅读",
      subject_scope: "global",
      section: "reading",
      edition_id: null,
      media_item_id: null,
      base_revision: request.base_revision,
      created_at: "2026-09-18T00:00:00Z",
      expires_at: "2026-09-18T00:15:00Z",
      changes,
    };
    this.proposals.push(outcome);
    return outcome;
  }

  async proposeResourcePreferencePatch(
    request: ResourcePreferenceProposalRequest,
  ): Promise<ProposalOutcome> {
    this.guard("proposeResourcePreferencePatch");
    this.mutationCount += 1;
    const patch: ResourcePreferencePatch = request.patch;
    const changes: { key: string; before: string; after: string }[] = [];
    for (const scope of ["reading", "comic"] as const) {
      const section = patch[scope];
      if (section === undefined || section === null) continue;
      for (const [key, value] of Object.entries(section)) {
        if (value === undefined || value === null) continue;
        changes.push({ key: `${scope}.${key}`, before: "unknown", after: String(value) });
      }
    }
    const outcome: ProposalOutcome = {
      proposal_id: proposalId("2", this.mutationCount),
      status: "pending",
      digest: DIGEST_A,
      target_label: "条目 · 阅读偏好",
      subject_scope: request.target_scope,
      section: null,
      edition_id: request.edition_id,
      media_item_id: request.media_item_id,
      base_revision: request.base_revision,
      created_at: "2026-09-18T00:00:00Z",
      expires_at: "2026-09-18T00:15:00Z",
      changes,
    };
    this.proposals.push(outcome);
    return outcome;
  }
}

export const FIXTURE_IDS = {
  edition: UUID_EDITION,
  mediaItem: UUID_MEDIA_ITEM,
  context: UUID_CONTEXT,
  digest: DIGEST_A,
} as const;

/**
 * 故意"泄漏"的桥接：每个返回值都塞进凭据 target、绝对路径、API key 形状与
 * denylist 键名。用来证明响应层确实把这些东西剥掉了——而不是"我们相信桥接层会守规矩"。
 */
export class LeakyHavenAgentBridge extends FixtureHavenAgentBridge {
  override async getSettingsSnapshot(): Promise<SettingsSnapshotResult> {
    const base = await super.getSettingsSnapshot();
    // 故意构造一个**违反契约**的返回值：多出 denylist 键名（含数据库原始列名与
    // Provider 原始响应），值里藏着路径与密钥。响应层必须把前者丢掉、把后者打码。
    const leaky = {
      ...base,
      secret: "sk-leaked-value-1234567890",
      credential_ref: "haven:ai:gw-main",
      sql: "SELECT data_json FROM settings WHERE section = 'reading'",
      sql_statement: "SELECT * FROM setting_proposals",
      db_row: { id: "p-1", data_json: "{}" },
      data_json: '{"fontSize":"large"}',
      payload_json: "{}",
      raw_provider_response: '{"data":[{"id":"gpt-4o"}]}',
      stack_trace: "at Object.<anonymous> (C:\\haven\\src\\db.rs:42:7)",
      settings: {
        ...base.settings,
        custom_background: "C:\\Users\\someone\\AppData\\Local\\haven\\bg.png",
        custom_text: "Bearer abcdefghijklmnop",
      },
    };
    return leaky as SettingsSnapshotResult;
  }
}
