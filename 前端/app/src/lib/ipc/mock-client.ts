// MockHavenClient（冻结前置消费证据：Rust 反序列化、TS 运行时守卫、Fixture 基线与 Mock 行为）。
// 无 Tauri 依赖：直接以 contracts/ipc/v1/fixtures/ 为数据源，供 UI 开发与契约测试共用。
// fixture 路径统一相对本文件：src/lib/ipc → 项目根 contracts/...
//
// favoriteSet 状态机（R-FAV-001/R-FAV-002）：
// - 首次收藏（true）→ 使用 set.success.json 的 revision（共享 fixture 数据源）。
// - 重复设置相同状态 → 返回当前 revision（不制造新版本）。
// - 取消收藏（false）→ 生成新 revision（mock-rev-N，≠ 收藏版本）。
// - 重复取消 → 返回同一 revision。
// - 从未收藏过的 Work 首次 set(false) → revision=null（无版本历史）。
//
// settingsGet / settingsUpdate 状态机（BE-SETTINGS-001 + R-MAIN-01）：
// - 默认值种子来自 settings/* default/saved fixture（共享 fixture）。
// - expected_revision 校验**先于**一切（含幂等短路）：过期 revision 即使提交相同值也 REVISION_CONFLICT；
//   已有行 + expected=None → 冲突；从未保存 + 非空 expected → 冲突。
// - 校验通过 + 相同值/空 patch → 幂等（changed=false，不写状态、不发事件）。
// - 校验通过 + 实际变化 → 新 revision（set-mock-N），changed=true，记入 settingsChangedEvents
//   （revision 与 Result 同源，镜像 P1-8 settings.changed 语义）。

import type { HavenClient } from "./client";
import type { UpdaterCheckResult, UpdaterInstallResult } from "./client";
import type {
  FavoriteSetRequest,
  FavoriteSetResult,
  LibraryListRequest,
  LibraryScanEvent,
  LibraryScanStartRequest,
  PageDto,
  ScanStartResult,
  WorkCardDto,
  WorkDetailHeaderDto,
  WorkGetRequest,
  ResourceListByMediaItemRequest,
  ResourceListDto,
  SessionOpenRequest,
  SessionOpenResultDto,
  SessionCloseRequest,
  SessionCloseResultDto,
  ProgressSaveRequest,
  ProgressMarkCompletedRequest,
  ProgressSaveResult,
  ProgressRecentRequest,
  ProgressResetRequest,
  ProgressSummaryDto,
  HistoryListRequest,
  HistoryEntryDto,
  MarkerCreateRequest,
  MarkerListRequest,
  MarkerListAllRequest,
  MarkerDeleteRequest,
  MarkerDto,
  CompletionWire,
  HomeDto,
  StorageLocationDto,
  DownloadCreateRequest,
  DownloadEvent,
  DownloadListRequest,
  DownloadMutationResultDto,
  DownloadRevealResultDto,
  DownloadTaskActionRequest,
  DownloadTaskDto,
  DownloadStateDto,
  ComicChapterCatalogGetRequest,
  ComicChapterCatalogDto,
  ComicRegisteredChapterCatalogDto,
  ComicWorkChapterCatalogRequestDto,
  ComicWorkChapterCatalogDto,
  ComicChapterSourceCandidatesGetRequestDto,
  ComicChapterSourceCandidatesDto,
  ComicProgressMigrationRequestDto,
  ComicProgressMigrationReceiptDto,
  ComicProgressMigrationResultDto,
  ComicPageProgressRemapRequestDto,
  ComicProgressMigrationRevertRequestDto,
  ComicProgressMigrationRevertResultDto,
  ComicPageManifestGetRequest,
  ComicPageManifestDto,
  ReaderTocGetRequest,
  ReaderTocResultDto,
  ReaderSearchRequest,
  ReaderSearchResultDto,
  ReaderSearchEvent,
  ReaderSearchCancelRequest,
  ReaderSearchCancelResultDto,
  TocItemDto,
  SourceRegistryDto,
  SourceDescriptorDto,
  SourceRegistrySetRequest,
  SourceRegistrySetResult,
  SourceEndpointSetRequest,
  SourceEndpointSetResult,
  SearchSourceStartRequest,
  SearchStartResultDto,
  SearchSourceEvent,
  SearchSourceEventKind,
  SearchSourceCancelRequest,
  SearchSourceCancelResultDto,
  SourceWorkImportRequest,
  SourceWorkImportResult,
  CredentialStatusRequest,
  CredentialStatusDto,
  CredentialSetRequest,
  CredentialDeleteRequest,
  MediaStateGetRequest,
  MediaStateDto,
  EnrichmentStatusRequest,
  EnrichmentStateDto,
  AppInfoDto,
  ErrorReportActionRequest,
  ErrorReportActionResultDto,
  ErrorReportConfirmRequest,
  ErrorReportConfirmResultDto,
  ErrorReportPreviewDto,
  ErrorReportPreviewRequest,
  SearchHistoryEntryDto,
  SearchHistoryListRequest,
  SearchHistoryRecordRequest,
  SearchHistoryRemoveRequest,
  CacheClearResultDto,
  CacheScopeDto,
  VideoScreenshotBeginResultDto,
  VideoScreenshotChunkRequest,
  VideoScreenshotResultDto,
  PreferenceComicSettingsDto,
  PreferenceReadingPatchDto,
  PreferenceReadingSettingsDto,
  PeriodicalTreeGetRequest,
  PeriodicalTreeDto,
  AgentBrokerStatusResultDto,
  AgentCapabilityManifestDto,
  AgentSettingChangeDto,
  AgentSettingChangeReceiptDto,
  AgentSettingChangeReceiptGetRequest,
  AgentResourcePreferenceProposalApproveRequest,
  AgentResourcePreferenceProposalApproveResultDto,
  AgentResourcePreferenceProposalCreateRequest,
  AgentResourcePreferenceProposalDto,
  AgentResourcePreferenceProposalGetRequest,
  AgentResourcePreferenceProposalGetResultDto,
  AgentSettingsContextDto,
  AgentSettingsProposalApproveRequest,
  AgentSettingsProposalApproveResultDto,
  AgentSettingsProposalCreateRequest,
  AgentSettingsProposalDto,
  AgentSettingsProposalGetRequest,
  AgentSettingsProposalGetResultDto,
  AgentSettingsProposalRejectRequest,
  AgentSettingsProposalRejectResultDto,
  AgentTraceGetRequest,
  AgentTraceGetResultDto,
  AiModelCapabilityDto,
  AiProviderModelsCatalogDto,
  AiProviderModelsListRequest,
  AiSettingsRecommendationDto,
  AiSettingsRecommendationGenerateRequest,
  AiProviderProfileDeleteRequest,
  AiProviderProfileDeleteResultDto,
  AiProviderProfileDto,
  AiProviderProfileGetRequest,
  AiProviderProfileListResultDto,
  AiProviderProfileUpsertRequest,
  TvboxConfigPreviewDto,
  TvboxConfigPreviewRequest,
  TvboxConfigSaveRequest,
  TvboxConfigSaveResult,
} from "./generated/wire";
import type {
  AgentSkillActivationWire,
  AgentSkillListResultWire,
  AgentSkillSetEnabledRequestWire,
  AgentSkillStateWire,
} from "./agent-skill-wire";
import type {
  McpClientConfigStatusWire,
  McpClientConfigureRequestWire,
  McpClientTargetStatusWire,
  McpClientTargetWire,
} from "./mcp-client-wire";
import type {
  SettingsChangedDto,
  SettingsSectionWire,
  SettingsSnapshot,
  SettingsUpdateRequest,
  SettingsUpdateResult,
  SettingsValue,
  PreferenceGetRequest,
  PreferenceGetResult,
  PreferenceReadingPatchWire,
  PreferenceComicPatchWire,
  PreferenceTargetWire,
  PreferenceUpdateRequest,
  PreferenceUpdateResult,
  AppearanceAssetImportRequestWire,
  AppearanceAssetDeleteRequestWire,
  AppearanceAssetDeleteResultWire,
  AppearanceAssetWire,
  AppearanceAssetsListRequestWire,
  AppearanceAssetsWire,
  HomeLayoutWire,
  HomeLayoutMutationResultWire,
  HomeLayoutResetRequestWire,
  HomeLayoutSaveRequestWire,
  HomeLayoutSnapshotWire,
  OverviewLayoutWire,
  OverviewLayoutMutationResultWire,
  OverviewLayoutResetRequestWire,
  OverviewLayoutSaveRequestWire,
  OverviewLayoutSnapshotWire,
  ReadingOverviewGetRequestWire,
  ReadingOverviewWire,
} from "./settings-wire";
import type { EditionListByWorkRequest, EditionListByWorkResultDto } from "../../features/media/ipc/edition-wire";
import type { EditionDetailDto, EditionGetRequest } from "./generated/wire";
import type { ErrorDto } from "./generated/wire";
import type {
  InterfaceFontAsset,
  InterfaceFontFamily,
  InterfaceFontImportResult,
} from "./interface-font-wire";
import {
  guardInterfaceFontAsset,
  isCanonicalInterfaceFontAssetId,
} from "./interface-font-wire.js";
import {
  applySettingsPatch,
  defaultHomeLayout,
  defaultOverviewLayout,
  defaultSettingsValue,
  emptyReadingOverview,
  guardHomeLayout,
  guardOverviewLayout,
  guardReadingOverviewGetRequest,
  guardSettingsSnapshot,
  guardSettingsUpdateResult,
  parseSettingsSection,
  settingsValuesEqual,
} from "./settings-wire.js";

import listNormal from "../../../../../contracts/ipc/v1/fixtures/library/list.normal.json" with { type: "json" };
import listEmpty from "../../../../../contracts/ipc/v1/fixtures/library/list.empty.json" with { type: "json" };
import favoriteSuccess from "../../../../../contracts/ipc/v1/fixtures/favorite/set.success.json" with { type: "json" };
import favoriteError from "../../../../../contracts/ipc/v1/fixtures/favorite/set.error-work-not-found.json" with { type: "json" };
import scanAlreadyRunning from "../../../../../contracts/ipc/v1/fixtures/scan/already-running.json" with { type: "json" };
import workGetNormal from "../../../../../contracts/ipc/v1/fixtures/work/get.normal.json" with { type: "json" };
import workGetNotFound from "../../../../../contracts/ipc/v1/fixtures/work/get.error-work-not-found.json" with { type: "json" };
import resourceMixedAvailability from "../../../../../contracts/ipc/v1/fixtures/resource/list.mixed-availability.json" with { type: "json" };
import comicCatalogNormal from "../../../../../contracts/ipc/v1/fixtures/comic/chapter-catalog.normal.json" with { type: "json" };
import comicWorkCatalogNormal from "../../../../../contracts/ipc/v1/fixtures/comic/work-chapter-catalog.normal.json" with { type: "json" };
import settingsGeneralDefault from "../../../../../contracts/ipc/v1/fixtures/settings/general.default.json" with { type: "json" };
import settingsAppearanceDefault from "../../../../../contracts/ipc/v1/fixtures/settings/appearance.default.json" with { type: "json" };
import settingsGeneralSaved from "../../../../../contracts/ipc/v1/fixtures/settings/general.saved.json" with { type: "json" };
import settingsAppearanceSaved from "../../../../../contracts/ipc/v1/fixtures/settings/appearance.saved.json" with { type: "json" };
import settingsPlaybackDefault from "../../../../../contracts/ipc/v1/fixtures/settings/playback.default.json" with { type: "json" };
import settingsPlaybackSaved from "../../../../../contracts/ipc/v1/fixtures/settings/playback.saved.json" with { type: "json" };
import settingsReadingDefault from "../../../../../contracts/ipc/v1/fixtures/settings/reading.default.json" with { type: "json" };
import settingsReadingSaved from "../../../../../contracts/ipc/v1/fixtures/settings/reading.saved.json" with { type: "json" };
import settingsComicDefault from "../../../../../contracts/ipc/v1/fixtures/settings/comic.default.json" with { type: "json" };
import settingsComicSaved from "../../../../../contracts/ipc/v1/fixtures/settings/comic.saved.json" with { type: "json" };
import settingsDownloadsDefault from "../../../../../contracts/ipc/v1/fixtures/settings/downloads.default.json" with { type: "json" };
import settingsDownloadsSaved from "../../../../../contracts/ipc/v1/fixtures/settings/downloads.saved.json" with { type: "json" };
import settingsPrivacyDefault from "../../../../../contracts/ipc/v1/fixtures/settings/privacy.default.json" with { type: "json" };
import type { ReadingPatchWire } from "./settings-wire";
import settingsConflictError from "../../../../../contracts/ipc/v1/fixtures/settings/update.error-revision-conflict.json" with { type: "json" };
import settingsInvalidArgument from "../../../../../contracts/ipc/v1/fixtures/settings/update.error-invalid-argument.json" with { type: "json" };
import sourceRegistryNormal from "../../../../../contracts/ipc/v1/fixtures/source/registry.normal.json" with { type: "json" };
import sourceSetErrorUnknown from "../../../../../contracts/ipc/v1/fixtures/source/set.error-unknown-source.json" with { type: "json" };
import mediaStateNormal from "../../../../../contracts/ipc/v1/fixtures/media-state/state.normal.json" with { type: "json" };
import appInfoMock from "../../../../../contracts/ipc/v1/fixtures/app-info/mock.json" with { type: "json" };
// A2 AI Provider：Mock 的默认状态就是「没有任何 profile」，也就是 UI 必须显示的
// 「无可用模型」。它不会凭空造出一个模型目录来让界面好看。
import aiProviderProfilesEmpty from "../../../../../contracts/ipc/v1/fixtures/ai-provider/profile.list.empty.json" with { type: "json" };
import aiProviderModelsReady from "../../../../../contracts/ipc/v1/fixtures/ai-provider/models.catalog.normal.json" with { type: "json" };
import { HavenError } from "./errors.js";
import type {
  CloudAccountDisconnectRequestWire,
  CloudAccountWire,
  CloudBrowsePageWire,
  CloudConnectAttemptWire,
  CloudConnectBeginRequestWire,
  CloudConnectPollWire,
  CloudConnectStatusWire,
  CloudFolderWire,
  CloudImportPdfRequestWire,
  CloudObjectWire,
  CloudRegisterFolderRequestWire,
  CloudStorageListWire,
} from "./cloud-storage-client";

const RESOURCE_FIXTURE_MEDIA_ITEM_ID = "0196f0d2-0000-7000-8000-000000000000";
const MEDIA_ITEM_ID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const CANONICAL_LOCAL_ID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

/** Demo 只接受规范小写 UUID；缺省与非法值分别返回 null 与显式错误。 */
function requireLocalUuid(value: string | null, label: string): string | null {
  if (value === null) return null;
  if (!CANONICAL_LOCAL_ID_PATTERN.test(value)) {
    throw new HavenError({
      code: "INVALID_ID",
      userMessage: `${label}标识格式非法`,
      retryable: false,
    });
  }
  return value;
}

/** 非法布局在 Rust 侧是 `APPEARANCE_INVALID_HOME_LAYOUT`（不是 REVISION_CONFLICT）。 */
function mockInvalidHomeLayout(): HavenError {
  return new HavenError({
    code: "APPEARANCE_INVALID_HOME_LAYOUT",
    userMessage: "首页布局非法（坐标越界/模块重复/占格重叠）",
    retryable: false,
  });
}

/** 规范形态：模块按 `order` 升序（镜像 Rust `HomeLayout::canonicalized`，幂等比较的前提）。 */
function canonicalHomeLayout(layout: HomeLayoutWire): HomeLayoutWire {
  return {
    schemaVersion: layout.schemaVersion,
    modules: [...layout.modules]
      .sort((left, right) => left.order - right.order)
      .map((placement) => ({ ...placement })),
  };
}

/** 深拷贝：Mock 内部的布局对象不得被调用方原地改写。 */
function cloneHomeLayout(layout: HomeLayoutWire): HomeLayoutWire {
  return {
    schemaVersion: layout.schemaVersion,
    modules: layout.modules.map((placement) => ({ ...placement })),
  };
}

/** 布局比较（顺序无关：先按 order 归一，再看模块与坐标是否逐字段相等）。 */
function homeLayoutsEqual(left: HomeLayoutWire, right: HomeLayoutWire): boolean {
  if (left.schemaVersion !== right.schemaVersion) return false;
  if (left.modules.length !== right.modules.length) return false;
  const normalizedLeft = canonicalHomeLayout(left).modules;
  const normalizedRight = canonicalHomeLayout(right).modules;
  return normalizedLeft.every((placement, index) => {
    const other = normalizedRight[index];
    return placement.module === other.module
      && placement.size === other.size
      && placement.row === other.row
      && placement.column === other.column
      && placement.order === other.order;
  });
}

// ---- 设置页总览布局（048）----
//
// 与首页布局同形但**完全独立**的一套状态与辅助函数：三列网格、另一组闭合模块 ID、
// 自己的 revision 计数器。Mock 里共用一套（例如把两者当成同一个 key）会让「两份布局
// 互不影响」这条不变量在演示环境与真实后端之间出现分歧。

/** 非法总览布局在 Rust 侧是 `APPEARANCE_INVALID_OVERVIEW_LAYOUT`（不是 REVISION_CONFLICT）。 */
function mockInvalidOverviewLayout(): HavenError {
  return new HavenError({
    code: "APPEARANCE_INVALID_OVERVIEW_LAYOUT",
    userMessage: "总览布局非法（坐标越界/模块重复/占格重叠）",
    retryable: false,
  });
}

/** 规范形态：模块按 `order` 升序（镜像 Rust `OverviewLayout::canonicalized`）。 */
function canonicalOverviewLayout(layout: OverviewLayoutWire): OverviewLayoutWire {
  return {
    schemaVersion: layout.schemaVersion,
    modules: [...layout.modules]
      .sort((left, right) => left.order - right.order)
      .map((placement) => ({ ...placement })),
  };
}

/** 深拷贝：Mock 内部的布局对象不得被调用方原地改写。 */
function cloneOverviewLayout(layout: OverviewLayoutWire): OverviewLayoutWire {
  return {
    schemaVersion: layout.schemaVersion,
    modules: layout.modules.map((placement) => ({ ...placement })),
  };
}

/** 布局比较（顺序无关；总览与首页是两套类型，因此不能共用同一个比较函数）。 */
function overviewLayoutsEqual(left: OverviewLayoutWire, right: OverviewLayoutWire): boolean {
  if (left.schemaVersion !== right.schemaVersion) return false;
  if (left.modules.length !== right.modules.length) return false;
  const normalizedLeft = canonicalOverviewLayout(left).modules;
  const normalizedRight = canonicalOverviewLayout(right).modules;
  return normalizedLeft.every((placement, index) => {
    const other = normalizedRight[index];
    return placement.module === other.module
      && placement.size === other.size
      && placement.row === other.row
      && placement.column === other.column
      && placement.order === other.order;
  });
}

const MOCK_COMIC_NO_SOURCE_PROGRESS_RECEIPT: ComicProgressMigrationReceiptDto = {
  migrationId: "0196f0d2-0000-7000-8000-00000000c001",
  sourceMediaItemId: RESOURCE_FIXTURE_MEDIA_ITEM_ID,
  targetMediaItemId: RESOURCE_FIXTURE_MEDIA_ITEM_ID,
  strategy: "no_target",
  confidence: "low",
  evidence: [],
  sourceProgressSnapshot: null,
  targetProgressBefore: null,
  targetProgressAfter: null,
  pageMapping: {
    targetPageIndex: null,
    confidence: "low",
    strategy: "no_target",
    reversible: true,
  },
  algorithmVersion: "comic-progress/v2",
  createdAt: "2026-01-01T00:00:00.000Z",
  undoable: false,
  appliedRevision: null,
};

/** Mock 只为这个演示 Work 提供期刊层级；其它 Work 一律「无期刊」。 */
const DEMO_PERIODICAL_WORK_ID = "0196f0d2-0000-7000-8000-00000000e001";

/**
 * 演示期刊层级：一个期刊 → 两个卷 → 三个期 → 四篇文章，确定性内联 fixture。
 *
 * 只包含本地身份与展示事实（标题、ISSN、刊期原文、发行日期、DOI、页码区间），
 * 以及用于幂等核对的来源 key / opaque 远端文章 ID；**不伪造正文 URL、signed URL、
 * 本地路径或 Provider 原始响应**——正文资源仍走既有 Session/grant 路径。
 * 每次调用返回新建对象，避免调用方改动污染后续 Demo 会话。
 */
function demoPeriodicalTree(): PeriodicalTreeDto {
  return {
    schemaVersion: 1,
    workId: DEMO_PERIODICAL_WORK_ID,
    periodical: {
      id: "0196f0d2-0000-7000-8000-00000000e100",
      workId: DEMO_PERIODICAL_WORK_ID,
      title: "示范期刊 · 信息科学前沿",
      issnPrint: null,
      issnElectronic: "1234-5679",
      publisher: "栖阅演示数据",
    },
    volumes: [
      {
        id: "0196f0d2-0000-7000-8000-00000000e201",
        periodicalId: "0196f0d2-0000-7000-8000-00000000e100",
        label: null,
        number: 12,
        year: 2024,
        ordinal: 0,
        issues: [
          {
            id: "0196f0d2-0000-7000-8000-00000000e301",
            volumeId: "0196f0d2-0000-7000-8000-00000000e201",
            label: "3-4",
            number: null,
            publicationDate: "2024-03",
            ordinal: 0,
            articles: [
              {
                id: "0196f0d2-0000-7000-8000-00000000e401",
                issueId: "0196f0d2-0000-7000-8000-00000000e301",
                mediaItemId: "0196f0d2-0000-7000-8000-00000000e501",
                ordinal: 1,
                title: "面向本地优先阅读器的期刊层级建模",
                doi: "10.0000/zhiyue.2024.0001",
                pageRange: { start: "e12345", end: null },
                sourceKey: "europepmc",
                remoteArticleId: "demo-0001",
                availability: "full_text",
              },
              {
                id: "0196f0d2-0000-7000-8000-00000000e402",
                issueId: "0196f0d2-0000-7000-8000-00000000e301",
                mediaItemId: "0196f0d2-0000-7000-8000-00000000e502",
                ordinal: null,
                title: "来源未给出序号的第二篇",
                doi: null,
                pageRange: null,
                sourceKey: "europepmc",
                remoteArticleId: "demo-0002",
                availability: "metadata_only",
              },
            ],
          },
          {
            id: "0196f0d2-0000-7000-8000-00000000e302",
            volumeId: "0196f0d2-0000-7000-8000-00000000e201",
            label: null,
            number: 4,
            publicationDate: "2024-04-18",
            ordinal: 1,
            articles: [
              {
                id: "0196f0d2-0000-7000-8000-00000000e403",
                issueId: "0196f0d2-0000-7000-8000-00000000e302",
                mediaItemId: "0196f0d2-0000-7000-8000-00000000e503",
                ordinal: 1,
                title: "不规则页码与缺页区间",
                doi: null,
                pageRange: { start: "S1", end: "S5" },
                sourceKey: "europepmc",
                remoteArticleId: "demo-0003",
                availability: "unknown",
              },
            ],
          },
        ],
      },
      {
        id: "0196f0d2-0000-7000-8000-00000000e202",
        periodicalId: "0196f0d2-0000-7000-8000-00000000e100",
        label: "Suppl 1",
        number: null,
        year: 2025,
        ordinal: 1,
        issues: [
          {
            id: "0196f0d2-0000-7000-8000-00000000e303",
            volumeId: "0196f0d2-0000-7000-8000-00000000e202",
            label: "Spring",
            number: null,
            publicationDate: "2025",
            ordinal: 0,
            articles: [
              {
                id: "0196f0d2-0000-7000-8000-00000000e404",
                issueId: "0196f0d2-0000-7000-8000-00000000e303",
                mediaItemId: "0196f0d2-0000-7000-8000-00000000e504",
                ordinal: 1,
                title: "增刊中的元数据候选文章",
                doi: null,
                pageRange: null,
                sourceKey: "europepmc",
                remoteArticleId: "demo-0004",
                availability: "metadata_only",
              },
            ],
          },
        ],
      },
    ],
  };
}

// Browser-only demo content. Production receives an opaque haven-resource URI from Tauri.
const DEMO_SESSION_CONTENT: Record<string, string> = {
  "2": "https://interactive-examples.mdn.mozilla.net/media/cc0-videos/flower.mp4",
  "4": "https://media.w3.org/2010/05/sintel/trailer.mp4",
};

/** 演示目录：与 demo 图书章节命名一致的确定性列表（浏览器环境只读展示）。 */
const DEMO_READER_TOC: TocItemDto[] = [
  { id: "a1b2c3d4e5f60718", title: "序言 · 重新定义个人内容空间", depth: 0, progression: 0 },
  { id: "b2c3d4e5f6071829", title: "第一章 · 硅谷的王者归来与 NeXT 时代", depth: 0, progression: 0.2 },
  { id: "c3d4e5f60718293a", title: "第二章 · 软件架构的复兴之路", depth: 0, progression: 0.4 },
  { id: "d4e5f60718293a4b", title: "第三章 · 跨媒介协同与信息流净化", depth: 0, progression: 0.6 },
  { id: "e5f60718293a4b5c", title: "第四章 · 留给未来的记忆锚点", depth: 0, progression: 0.8 },
  { id: "f60718293a4b5c6d", title: "尾声 · 寻找值得被做出来的造物", depth: 0, progression: 1 },
];

interface FavoriteState {
  active: boolean;
  revision: string | null;
}

interface SettingsState {
  value: SettingsValue;
  revision: string | null;
}

interface PreferenceState {
  readingPatch: PreferenceReadingPatchWire | null;
  comicPatch: PreferenceComicPatchWire | null;
  revision: string | null;
}

function toPreferenceReadingSettings(
  value: Extract<SettingsValue, { section: "reading" }>,
): PreferenceReadingSettingsDto {
  return {
    section: "reading",
    fontFamily: value.fontFamily,
    customFontFamily: value.customFontFamily ?? null,
    fontSize: value.fontSize,
    lineHeight: value.lineHeight,
    contentWidth: value.contentWidth,
    theme: value.theme,
    customBackground: value.customBackground ?? null,
    customText: value.customText ?? null,
    fontWeight: value.fontWeight,
    letterSpacing: value.letterSpacing,
    systemAuto: value.systemAuto,
    pagination: value.pagination ?? "scroll",
  };
}

function toPreferenceComicSettings(
  value: Extract<SettingsValue, { section: "comic" }>,
): PreferenceComicSettingsDto {
  return {
    section: "comic",
    viewMode: value.viewMode,
    direction: value.direction,
    pageGap: value.pageGap,
    preloadPages: value.preloadPages,
  };
}

interface ProgressState {
  request: ProgressSaveRequest;
  revision: string;
}

export interface MockHavenClientOptions {
  /** true（默认）：用 settings/*.saved.json 播种（镜像 favorites 播种；用于更新/冲突/事件场景）。 */
  seedSettings?: boolean;
  /**
   * 预置外观资产（默认空）。
   *
   * Mock 没有原生文件选择器，`appearanceAssetImport` 因此不会「假装选中文件」；
   * 需要资产的 UI 场景只能通过这里注入合法资产投影（含不透明 assetId，没有路径），
   * 列表读取与删除就作用在这份内存状态上。`byteSize` 就是 IPC 载荷里的 number
   * （DTO 上的 u64 已按 `#[ts(type = "number")]` 对齐真实 JSON 数字）。
   */
  appearanceAssets?: AppearanceAssetWire[];
}

/** 共享 Fixture 驱动的 Mock Client（契约冻结前置的最小消费实现）。 */
// ---- Agent 全局设置 Mock 辅助（A1 智能配置推荐） ----

interface MockAgentProposalRecord {
  proposalId: string
  digest: string
  status: "pending" | "applied" | "rejected" | "expired" | "conflict"
  baseRevision: string | null
  changes: AgentSettingChangeDto[]
  patch: PreferenceReadingPatchDto
  createdAt: string
  expiresAt: string
  receipt?: { appliedRevision: string; appliedAt: string; changed: boolean }
}

/** 从 SettingsValue 里取出阅读分区（形状不符即抛错，不做静默降级）。 */
function readingSettingsOf(value: SettingsValue): PreferenceReadingSettingsDto {
  if (typeof value !== "object" || value === null || !("section" in value)) {
    throw new Error("reading 设置形状非法")
  }
  const candidate = value as { section?: unknown }
  if (candidate.section !== "reading") {
    throw new Error("reading 设置形状非法")
  }
  return value as PreferenceReadingSettingsDto
}

/**
 * 提案 patch → 设置分区 patch。
 *
 * 空串在 Rust 语义里表示"清除该覆盖"，因此这里映射为 null；
 * 其余字段直接透传（Wire 枚举与 DTO 枚举的字符串表示逐字一致）。
 */
function mockReadingPatch(patch: PreferenceReadingPatchDto): ReadingPatchWire {
  const cleared = (value: string | null | undefined): string | null | undefined =>
    value === undefined ? undefined : value === null || value.trim() === "" ? null : value
  return {
    section: "reading",
    fontFamily: patch.fontFamily ?? undefined,
    customFontFamily: cleared(patch.customFontFamily) as string | null | undefined,
    fontSize: patch.fontSize ?? undefined,
    lineHeight: patch.lineHeight ?? undefined,
    contentWidth: patch.contentWidth ?? undefined,
    theme: patch.theme ?? undefined,
    customBackground: cleared(patch.customBackground) as string | null | undefined,
    customText: cleared(patch.customText) as string | null | undefined,
    fontWeight: patch.fontWeight ?? undefined,
    letterSpacing: patch.letterSpacing ?? undefined,
    systemAuto: patch.systemAuto ?? undefined,
    pagination: patch.pagination ?? undefined,
  }
}

/** 逐字段比较"当前值 vs patch 应用后的值"，与 Rust 的改动列表保持同一语义。 */
function mockReadingChanges(
  current: SettingsValue,
  patch: PreferenceReadingPatchDto,
): AgentSettingChangeDto[] {
  const before = readingSettingsOf(current)
  const after = readingSettingsOf(applySettingsPatch(current, mockReadingPatch(patch)))
  const changes: AgentSettingChangeDto[] = []
  const push = (key: string, from: unknown, to: unknown) => {
    const fromText = from === null || from === undefined ? "" : String(from)
    const toText = to === null || to === undefined ? "" : String(to)
    if (fromText !== toText) {
      changes.push({ key, before: fromText, after: toText })
    }
  }
  push("reading.fontFamily", before.fontFamily, after.fontFamily)
  push("reading.customFontFamily", before.customFontFamily, after.customFontFamily)
  push("reading.fontSize", before.fontSize, after.fontSize)
  push("reading.lineHeight", before.lineHeight, after.lineHeight)
  push("reading.contentWidth", before.contentWidth, after.contentWidth)
  push("reading.theme", before.theme, after.theme)
  push("reading.customBackground", before.customBackground, after.customBackground)
  push("reading.customText", before.customText, after.customText)
  push("reading.fontWeight", before.fontWeight, after.fontWeight)
  push("reading.letterSpacing", before.letterSpacing, after.letterSpacing)
  push("reading.systemAuto", before.systemAuto, after.systemAuto)
  push("reading.pagination", before.pagination, after.pagination)
  return changes
}

function mockProposalDto(record: MockAgentProposalRecord): AgentSettingsProposalDto {
  return {
    schemaVersion: 1,
    proposalId: record.proposalId,
    status: record.status === "conflict" ? "pending" : record.status,
    subject: { section: "reading" },
    targetLabel: "全局默认 · 阅读",
    baseRevision: record.baseRevision,
    digest: record.digest,
    createdAt: record.createdAt,
    expiresAt: record.expiresAt,
    changes: record.changes,
  }
}

function mockReceiptDto(record: MockAgentProposalRecord): AgentSettingChangeReceiptDto {
  const receipt = record.receipt
  if (!receipt) throw new Error("回执不存在")
  return {
    schemaVersion: 1,
    receiptId: `mock-receipt-${record.proposalId}`,
    proposalId: record.proposalId,
    proposalDigest: record.digest,
    status: record.status === "conflict" ? "pending" : record.status,
    appliedRevision: receipt.appliedRevision,
    changed: receipt.changed,
    changes: record.changes,
    appliedAt: receipt.appliedAt,
  }
}

/**
 * Mock 确定性摘要（64 位小写十六进制，FNV-1a 展开）。
 *
 * 它**不是** Rust 的 canonical SHA-256，只是让契约形状（长度/字符集）与真实 digest
 * 一致，从而让页面能按同一条路径渲染与回传；Mock/预览标识由调用方负责展示。
 */
function mockDigest(payload: unknown): string {
  const canonical = mockCanonicalJson(payload)
  let out = ""
  for (let block = 0; out.length < 64; block += 1) {
    let hash = 0x811c9dc5 ^ block
    for (let index = 0; index < canonical.length; index += 1) {
      hash ^= canonical.charCodeAt(index)
      hash = Math.imul(hash, 0x01000193) >>> 0
    }
    out += hash.toString(16).padStart(8, "0")
  }
  return out.slice(0, 64)
}

/** 键按字节序、紧凑输出的 canonical JSON（数组保序）。 */
function mockCanonicalJson(value: unknown): string {
  if (value === null || typeof value !== "object") return JSON.stringify(value) ?? "null"
  if (Array.isArray(value)) return `[${value.map(mockCanonicalJson).join(",")}]`
  const entries = Object.entries(value as Record<string, unknown>)
    .filter(([, entry]) => entry !== undefined)
    .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))
  return `{${entries.map(([key, entry]) => `${JSON.stringify(key)}:${mockCanonicalJson(entry)}`).join(",")}}`
}

/** 由 digest 派生确定性 UUID（v5 风格版本/变体位），让 Mock 的 contextId 稳定可复算。 */
function mockUuidFromDigest(digest: string): string {
  const hex = digest.slice(0, 32).split("")
  hex[12] = "5"
  hex[16] = "8"
  const joined = hex.join("")
  return `${joined.slice(0, 8)}-${joined.slice(8, 12)}-${joined.slice(12, 16)}-${joined.slice(16, 20)}-${joined.slice(20, 32)}`
}

/**
 * Mock 样例数据：本机字体族。
 *
 * 真实枚举只在 Tauri surface 发生（Rust 侧读系统字体目录）。这里的数据只服务
 * 浏览器 dev 与测试的可重复性，不声称任何一项是当前机器的真实字体事实。
 */
const MOCK_SYSTEM_FONT_FAMILIES: InterfaceFontFamily[] = [
  { family: "Microsoft YaHei UI", localizedFamily: "微软雅黑" },
  { family: "Source Han Serif SC", localizedFamily: "思源宋体" },
  { family: "Inter", localizedFamily: null },
];

/** Mock 样例数据：已导入字体资产（opaque id + 展示元数据，无路径、无字节）。 */
const MOCK_INTERFACE_FONT_ASSETS: InterfaceFontAsset[] = [
  {
    id: "0f8f7a2c-1b3d-4e5f-8a90-1c2d3e4f5a60",
    familyName: "示例展示体",
    fileName: "DemoDisplay.woff2",
    extension: "woff2",
    mimeType: "font/woff2",
    byteSize: 40960,
    createdAt: 1735689600000,
  },
];

const interfaceFontAssetInUseError: ErrorDto = {
  code: "FONT_ASSET_IN_USE",
  userMessage: "该字体仍被界面字体设置引用，请先改选其他字体再删除",
  retryable: false,
};

const interfaceFontAssetNotFoundError: ErrorDto = {
  code: "FONT_ASSET_NOT_FOUND",
  userMessage: "字体不存在或已被删除",
  retryable: false,
};

/**
 * Browser Demo 的 Broker「端点」。
 *
 * 它**不是** Windows Named Pipe，也不是 Unix domain socket：浏览器里没有 Rust
 * Broker，没有监听，也没有可连接的本地端点。因此这里给的是一个明确标注的演示
 * 标识，而不是 `\\.\pipe\haven-agent-v1-…` / `…/agent-v1.sock` 形状的假路径——
 * 伪造一段真实形态的本地地址，会让页面把假端点当可用配置展示或复制给用户。
 */
const MOCK_AGENT_BROKER_ENDPOINT = "mock://browser-preview-agent-broker"

/**
 * 浏览器预览的示例技能标识。
 *
 * 与上面的 mock 端点同一条理由：真实的内置技能目录是编译进二进制的构建产物，
 * 浏览器里没有它。使用真实 id（`haven-agent-proposal`）会让预览看起来像
 * "这项技能已经在栖阅里生效"，而预览根本没有模型请求路径可以消费它。
 */
const MOCK_PREVIEW_SKILL_ID = "preview-builtin-skill"
const MOCK_PREVIEW_SKILL_DESCRIPTION = "浏览器预览项：桌面应用的内置技能目录由 Rust 侧提供"
const MOCK_PREVIEW_SKILL_CHARS = 0

/**
 * 与后端 `MAX_CONFIG_URL_CHARS` 对齐的预览地址上限（Rust 用字符数，这里用 UTF-16
 * 码元数——对 ASCII 地址两者一致，Mock 只做与后端同形的输入校验）。
 */
const MAX_TVBOX_CONFIG_URL_CHARS = 2048

/** 与后端 `MAX_TVBOX_DISPLAY_NAME_CHARS` 对齐的显示名上限。 */
const MAX_TVBOX_DISPLAY_NAME_CHARS = 100

/**
 * 浏览器预览里没有的能力：返回一句可展示的说明。
 *
 * 用 `HAVEN_CAPABILITY_UNAVAILABLE` 与「演示环境不支持…；请在桌面应用里…」的既有措辞，
 * 让 UI 能把它转成提示而不是崩溃。
 */
function cloudUnsupported(action: string): HavenError {
  return new HavenError({
    code: "HAVEN_CAPABILITY_UNAVAILABLE",
    userMessage: `演示环境不支持${action}；请在桌面应用里使用云盘。`,
    retryable: false,
  })
}

export class MockHavenClient implements HavenClient {
  /** 空库模式：libraryList 返回 list.empty（供空态 UI 场景）。 */
  private readonly emptyLibrary: boolean;
  private readonly favorites: Map<string, FavoriteState>;
  private readonly settings: Map<SettingsSectionWire, SettingsState>;
  private readonly preferences = new Map<string, PreferenceState>();
  private readonly searchHistory = new Map<string, SearchHistoryEntryDto>();
  private readonly screenshotUploads = new Map<string, { nextSequence: number; totalBytes: number }>();
  private screenshotUploadCounter = 1;
  private readonly progress = new Map<string, ProgressState>();
  private readonly activeSessions = new Map<string, SessionOpenResultDto>();
  private readonly comicManifests = new Map<string, ComicPageManifestDto>();
  private revisionCounter = 2;
  private settingsRevisionCounter = 1;
  private preferenceRevisionCounter = 1;
  private runtimeIdentityCounter = 1;
  private progressRevisionCounter = 1;
  private markerCounter = 1;
  private readonly markers: MarkerDto[] = [];
  private readonly agentProposals = new Map<string, MockAgentProposalRecord>();
  private agentProposalCounter = 1;
  /** A5：Broker 与 Rust 侧一样**默认关闭**；只有显式 enable 才是 listening。 */
  private agentBrokerListening = false;
  private searchOperationCounter = 1;
  /** 导入字体资产的 Mock 内存态（与真实后端同一不变量，但不读真实文件）。 */
  private readonly fontAssets: InterfaceFontAsset[] = MOCK_INTERFACE_FONT_ASSETS.map((asset) => ({ ...asset }));
  private fontAssetCounter = 1;
  private readonly sourceEnabled = new Map<string, boolean>();
  private readonly sourceEndpoints = new Map<string, string>();
  private readonly credentialProfiles = new Set<string>();
  /// A2：Mock 的初始 Provider Profile 集合直接来自共享 fixture（当前为空），
  /// 因此默认状态下设置页必须显示「无可用模型」。
  private readonly aiProfiles = new Map<string, AiProviderProfileDto>(
    (aiProviderProfilesEmpty.profiles as AiProviderProfileDto[]).map((profile) => [
      profile.profileId,
      profile,
    ]),
  );
  private aiProfileRevisionCounter = 1;
  private readonly searchOperations = new Map<
    string,
    { queryKey: string; finished: boolean; onEvent?: (event: SearchSourceEvent) => void; nextSequence: number }
  >();
  private readonly downloadTasks: DownloadTaskDto[] = [
    {
      schemaVersion: 1,
      taskId: "0196f0d2-0000-7000-8000-00000000d001",
      workId: "0196f0d2-0000-7000-8000-000000000001",
      editionId: "0196f0d2-0000-7000-8000-000000000002",
      mediaItemId: RESOURCE_FIXTURE_MEDIA_ITEM_ID,
      sourceResourceId: "0196f0d2-0000-7000-8000-000000000103",
      targetStorageId: "0196f0d2-0000-7000-8000-00000000d100",
      offlineResourceId: null,
      title: "怪奇物语：1985故事集 第一季",
      mediaType: "movie",
      category: "video",
      posterUri: "https://images.unsplash.com/photo-1578632767115-351597cf2477?q=80&w=400&auto=format&fit=crop",
      state: "downloading",
      bytesTotal: 2_600_000_000,
      bytesDownloaded: 1_200_000_000,
      progressRatio: 1_200_000_000 / 2_600_000_000,
      speedBps: 8_500_000,
      etaSeconds: 165,
      createdAt: "2026-08-20T01:00:00.000Z",
      updatedAt: "2026-08-20T01:02:00.000Z",
    },
  ];

  /** `settings.changed` 事件日志（仅 changed=true 时追加；镜像 P1-8，供测试断言）。 */
  readonly settingsChangedEvents: SettingsChangedDto[] = [];

  /** 外观资产内存状态：只有类型化投影，没有路径/URL，也没有第二份持久化事实源。 */
  private readonly appearanceAssets: AppearanceAssetWire[];
  /** 首页布局内存状态；null = 从未保存过自定义（读取回落到领域默认布局）。 */
  private homeLayoutState: { layout: HomeLayoutWire; revision: string } | null = null;
  private homeLayoutRevisionCounter = 1;
  /** 总览布局内存状态；与首页布局各自独立（另一张表、另一条 revision）。 */
  private overviewLayoutState: { layout: OverviewLayoutWire; revision: string } | null = null;
  private overviewLayoutRevisionCounter = 1;

  constructor(emptyLibrary = false, options: MockHavenClientOptions = {}) {
    this.emptyLibrary = emptyLibrary;
    this.favorites = new Map();
    this.settings = new Map();
    this.appearanceAssets = [...(options.appearanceAssets ?? [])];
    // 从共享 Fixture 播种：set.success.json 的 workId 处于已收藏状态。
    const success = favoriteSuccess as FavoriteSetResult;
    this.favorites.set(success.workId, { active: success.favorite, revision: success.revision });
    // Settings 播种（默认开启）：与 favorites 播种同理，让 Mock 返回同一份共享 Fixture。
    if (options.seedSettings !== false) {
      const general = settingsGeneralSaved as SettingsSnapshot;
      const appearance = settingsAppearanceSaved as SettingsSnapshot;
      const playback = settingsPlaybackSaved as SettingsSnapshot;
      const reading = settingsReadingSaved as SettingsSnapshot;
      const comic = settingsComicSaved as SettingsSnapshot;
      const downloads = settingsDownloadsSaved as SettingsSnapshot;
      this.settings.set("general", { value: general.value, revision: general.revision });
      this.settings.set("appearance", { value: appearance.value, revision: appearance.revision });
      this.settings.set("playback", { value: playback.value, revision: playback.revision });
      this.settings.set("reading", { value: reading.value, revision: reading.revision });
      this.settings.set("comic", { value: comic.value, revision: comic.revision });
      this.settings.set("downloads", { value: downloads.value, revision: downloads.revision });
    }
  }

  async libraryList(_request: LibraryListRequest): Promise<PageDto<WorkCardDto>> {
    return this.emptyLibrary
      ? (listEmpty as PageDto<WorkCardDto>)
      : (listNormal as PageDto<WorkCardDto>);
  }

  async favoriteSet(request: FavoriteSetRequest): Promise<FavoriteSetResult> {
    // Mock 规则：workId 必须是 36 字符 uuid 形状（否则视为不存在的 Work → WORK_NOT_FOUND）。
    if (request.workId.length !== 36) {
      throw new HavenError(favoriteError as never);
    }
    const state = this.favorites.get(request.workId) ?? { active: false, revision: null };
    if (state.active === request.favorite) {
      // 幂等重复设置：返回当前 revision，不制造新版本（R-FAV-001）
      return { workId: request.workId, favorite: request.favorite, revision: state.revision };
    }
    // 状态实际变化 → 统一生成新 token（含 false→true 重新收藏；success fixture 仅用于初始播种/基线）
    const nextRevision = `mock-rev-${this.revisionCounter++}`;
    this.favorites.set(request.workId, { active: request.favorite, revision: nextRevision });
    return { workId: request.workId, favorite: request.favorite, revision: nextRevision };
  }

  async libraryScanStart(
    _request: LibraryScanStartRequest,
    _onEvent?: (event: LibraryScanEvent) => void,
  ): Promise<ScanStartResult> {
    // Mock 环境无真实扫描：返回 already-running fixture，不触发 Channel 回调。
    return scanAlreadyRunning as ScanStartResult;
  }

  async workGet(request: WorkGetRequest): Promise<WorkDetailHeaderDto> {
    const result = workGetNormal as WorkDetailHeaderDto;
    if (result.workId !== request.workId) {
      throw new HavenError(workGetNotFound as never);
    }
    return result;
  }

  /** Browser demo intentionally keeps its curated page data; this method remains a typed client seam. */
  async editionListByWork(_request: EditionListByWorkRequest): Promise<EditionListByWorkResultDto> {
    return {
      schemaVersion: 1,
      items: [],
      nextCursor: null,
      total: 0,
      revision: null,
    };
  }

  async editionGet(_request: EditionGetRequest): Promise<EditionDetailDto> {
    return {
      schemaVersion: 1,
      editionId: _request.editionId,
      workId: "",
      title: "",
      subtitle: null,
      mediaType: "unknown",
      releaseDate: null,
      language: null,
      region: null,
      publisherOrStudio: null,
      description: null,
      items: [],
    };
  }

  async resourceListByMediaItem(request: ResourceListByMediaItemRequest): Promise<ResourceListDto> {
    if (
      request.mediaItemId.length !== 36 ||
      !MEDIA_ITEM_ID_PATTERN.test(request.mediaItemId)
    ) {
      throw new HavenError({
        code: "INVALID_ID",
        userMessage: "ID 格式非法",
        retryable: false,
      });
    }
    if (request.mediaItemId !== RESOURCE_FIXTURE_MEDIA_ITEM_ID) {
      throw new HavenError({
        code: "MEDIA_ITEM_NOT_FOUND",
        userMessage: "媒体条目不存在",
        retryable: false,
      });
    }
    return resourceMixedAvailability as ResourceListDto;
  }

  async sessionOpen(request: SessionOpenRequest): Promise<SessionOpenResultDto> {
    const demoContentUri = DEMO_SESSION_CONTENT[request.mediaItemId];
    if (!demoContentUri) {
      throw new HavenError({
        code: "MEDIA_ITEM_NOT_FOUND",
        userMessage: "媒体条目不存在",
        retryable: false,
      });
    }
    const sessionId = this.nextRuntimeIdentity();
    const session: SessionOpenResultDto = {
      schemaVersion: 1,
      sessionId,
      contentUri: request.engine === "comic" ? null : demoContentUri,
      workId: `mock-work-${request.mediaItemId}`,
      editionId: `mock-edition-${request.mediaItemId}`,
      mediaItemId: request.mediaItemId,
      engine: request.engine,
      progress: null,
    };
    this.activeSessions.set(sessionId, session);
    return session;
  }

  async comicPageManifestGet(
    request: ComicPageManifestGetRequest,
  ): Promise<ComicPageManifestDto> {
    const session = this.activeSessions.get(request.sessionId);
    if (!session) {
      throw new HavenError({
        code: "RESOURCE_NOT_FOUND",
        userMessage: "漫画会话不存在或已关闭",
        retryable: false,
      });
    }
    if (session.engine !== "comic") {
      throw new HavenError({
        code: "FORMAT_UNSUPPORTED",
        userMessage: "当前会话不是漫画会话",
        retryable: false,
      });
    }
    const existing = this.comicManifests.get(request.sessionId);
    if (existing) return existing;

    const pages = [0, 1, 2].map((pageIndex) => {
      const pageId = this.nextRuntimeIdentity();
      const ready = pageIndex !== 1;
      return {
        pageId,
        pageIndex,
        availability: ready ? "ready" as const : "unavailable" as const,
        contentUri: ready
          ? `haven-resource://comic-page/${this.nextRuntimeIdentity()}`
          : null,
      };
    });
    const manifest: ComicPageManifestDto = {
      schemaVersion: 1,
      sessionId: session.sessionId,
      mediaItemId: session.mediaItemId,
      pageCount: pages.length,
      pages,
    };
    this.comicManifests.set(request.sessionId, manifest);
    return manifest;
  }

  /**
   * 浏览器 Demo 的期刊层级：只对演示 Work 返回确定性层级，其它 Work（含演示书库里
   * 的图书）一律 `PERIODICAL_NOT_FOUND`。这里不按标题猜期刊、不伪造空树，与后端
   * "读不到层级就是资源错误"的语义保持一致。
   */
  async periodicalTreeGet(request: PeriodicalTreeGetRequest): Promise<PeriodicalTreeDto> {
    if (request.workId !== DEMO_PERIODICAL_WORK_ID) {
      throw new HavenError({
        code: "PERIODICAL_NOT_FOUND",
        userMessage: "该作品没有期刊层级",
        retryable: false,
      });
    }
    return demoPeriodicalTree();
  }

  async comicChapterCatalogGet(
    request: ComicChapterCatalogGetRequest,
  ): Promise<ComicChapterCatalogDto> {
    const catalog = comicCatalogNormal as ComicChapterCatalogDto;
    if (request.sourceId !== catalog.sourceId || request.remoteWorkId !== catalog.remoteWorkId) {
      throw new HavenError({
        code: "RESOURCE_NOT_FOUND",
        userMessage: "漫画来源作品不存在",
        retryable: false,
      });
    }
    return catalog;
  }

  async comicChapterCatalogRegisteredGet(
    request: ComicChapterCatalogGetRequest,
  ): Promise<ComicRegisteredChapterCatalogDto> {
    const catalog = comicCatalogNormal as ComicChapterCatalogDto;
    if (request.sourceId !== catalog.sourceId || request.remoteWorkId !== catalog.remoteWorkId) {
      throw new HavenError({
        code: "RESOURCE_NOT_FOUND",
        userMessage: "漫画来源作品不存在",
        retryable: false,
      });
    }
    return {
      schemaVersion: 1,
      sourceId: catalog.sourceId,
      remoteWorkId: catalog.remoteWorkId,
      refreshState: {
        generation: 1,
        fetchedAt: catalog.fetchedAt,
        total: catalog.total,
        truncated: catalog.truncated,
      },
      chapters: catalog.chapters.map((chapter, sourceOrder) => ({
        mediaItemId: `eeeeeeee-eeee-4eee-8eee-${String(sourceOrder + 1).padStart(12, "0")}`,
        sourceId: catalog.sourceId,
        remoteWorkId: catalog.remoteWorkId,
        remoteChapterId: chapter.remoteChapterId,
        chapterNumber: chapter.chapterNumber,
        volumeNumber: chapter.volumeNumber,
        title: chapter.title,
        pageCount: chapter.pageCount,
        sourceOrder,
        availability: chapter.availability,
        publishedAt: chapter.publishedAt,
        sourceUpdatedAt: chapter.updatedAt,
        lastSeenGeneration: 1,
        editionProfile: chapter.editionProfile,
      })),
    };
  }

  async comicChapterCatalogRefresh(
    request: ComicChapterCatalogGetRequest,
  ): Promise<ComicChapterCatalogDto> {
    return this.comicChapterCatalogGet(request);
  }

  /**
   * Browser Demo 的 Work 级漫画目录。
   *
   * Demo 只持有一份确定性 fixture；这里回显请求里的本地 Work/MediaItem 身份，
   * 不构造 Provider URL、页面授权、远端章节 ID 或后端未给出的上一章/下一章。
   */
  async comicWorkChapterCatalogGet(
    request: ComicWorkChapterCatalogRequestDto,
  ): Promise<ComicWorkChapterCatalogDto> {
    const catalog = comicWorkCatalogNormal as ComicWorkChapterCatalogDto;
    const workId = requireLocalUuid(request.workId, "作品");
    const mediaItemId = requireLocalUuid(request.mediaItemId, "章节");
    if ((workId === null) === (mediaItemId === null)) {
      throw new HavenError({
        code: "INVALID_ARGUMENT",
        userMessage: "作品与章节必须二选一",
        retryable: false,
      });
    }
    return {
      ...catalog,
      workId: workId ?? catalog.workId,
      currentMediaItemId: workId === null ? mediaItemId : null,
      refreshReceipts: catalog.refreshReceipts.map((receipt) => ({
        ...receipt,
        workId: workId ?? catalog.workId,
      })),
    };
  }

  async comicWorkChapterCatalogRefresh(
    request: ComicWorkChapterCatalogRequestDto,
  ): Promise<ComicWorkChapterCatalogDto> {
    return this.comicWorkChapterCatalogGet(request);
  }

  /** Browser Demo 没有持久化的来源引用图谱；返回明确的空候选投影。 */
  async comicChapterSourceCandidatesGet(
    request: ComicChapterSourceCandidatesGetRequestDto,
  ): Promise<ComicChapterSourceCandidatesDto> {
    return {
      schemaVersion: 1,
      source: request.source,
      currentMediaItemId: RESOURCE_FIXTURE_MEDIA_ITEM_ID,
      candidates: [],
      truncated: false,
    };
  }

  /** Browser Demo 没有持久化的漫画章节进度；返回真实的“无源进度”终态，
   * 避免伪造已经应用的迁移或快照。 */
  async comicProgressMigrate(
    _request: ComicProgressMigrationRequestDto,
  ): Promise<ComicProgressMigrationResultDto> {
    return {
      status: "no_source_progress",
      matchResult: null,
      pageMigration: {
        targetPageIndex: null,
        confidence: "low",
        strategy: "no_target",
        reversible: true,
      },
      snapshotId: null,
      appliedRevision: null,
      receipt: MOCK_COMIC_NO_SOURCE_PROGRESS_RECEIPT,
    };
  }

  async comicProgressRemap(
    _request: ComicPageProgressRemapRequestDto,
  ): Promise<ComicProgressMigrationResultDto> {
    return {
      status: "no_source_progress",
      matchResult: null,
      pageMigration: {
        targetPageIndex: null,
        confidence: "low",
        strategy: "no_target",
        reversible: true,
      },
      snapshotId: null,
      appliedRevision: null,
      receipt: MOCK_COMIC_NO_SOURCE_PROGRESS_RECEIPT,
    };
  }

  async comicProgressRevert(
    _request: ComicProgressMigrationRevertRequestDto,
  ): Promise<ComicProgressMigrationRevertResultDto> {
    return { reverted: false };
  }

  async readerTocGet(request: ReaderTocGetRequest): Promise<ReaderTocResultDto> {
    const session = this.activeSessions.get(request.sessionId);
    if (!session) {
      throw new HavenError({
        code: "RESOURCE_NOT_FOUND",
        userMessage: "阅读会话不存在或已关闭",
        retryable: false,
      });
    }
    if (session.engine !== "reader") {
      throw new HavenError({
        code: "FORMAT_UNSUPPORTED",
        userMessage: "当前会话不是阅读会话",
        retryable: false,
      });
    }
    return { schemaVersion: 1, sessionId: session.sessionId, items: DEMO_READER_TOC };
  }

  async readerSearch(request: ReaderSearchRequest): Promise<ReaderSearchResultDto> {
    const session = this.activeSessions.get(request.sessionId);
    if (!session) {
      throw new HavenError({
        code: "RESOURCE_NOT_FOUND",
        userMessage: "阅读会话不存在或已关闭",
        retryable: false,
      });
    }
    if (session.engine !== "reader") {
      throw new HavenError({
        code: "FORMAT_UNSUPPORTED",
        userMessage: "当前会话不是阅读会话",
        retryable: false,
      });
    }
    const query = request.query.trim();
    if (!query || query.length > 128) {
      return { schemaVersion: 1, sessionId: session.sessionId, hits: [] };
    }
    const lower = query.toLowerCase();
    const hits = DEMO_READER_TOC.filter((item) => item.title.toLowerCase().includes(lower))
      .slice(0, 5)
      .map((item, idx) => ({
        chapterId: `chapter-${idx + 1}`,
        chapterTitle: item.title,
        chapterIndex: idx,
        paragraphIndex: 0,
        progressionInChapter: item.progression,
        textAnchor: { exact: query, prefix: item.title.slice(0, 10), suffix: item.title.slice(-10) },
        score: 1.0 - idx * 0.1,
      }));
    return { schemaVersion: 1, sessionId: session.sessionId, hits };
  }

  async readerSearchStart(
    request: ReaderSearchRequest,
    onEvent?: (event: ReaderSearchEvent) => void,
  ): Promise<ReaderSearchResultDto> {
    const result = await this.readerSearch(request)
    if (onEvent) {
      const operationId = `mock-reader-search-${Date.now()}`
      const now = new Date().toISOString()
      onEvent({
        operationId,
        sequence: 1,
        at: now,
        kind: "started",
        data: { hits: [], scannedChapters: 0, totalChapters: 1, code: null, message: null },
      })
      onEvent({
        operationId,
        sequence: 2,
        at: now,
        kind: "result",
        data: { hits: result.hits, scannedChapters: 1, totalChapters: 1, code: null, message: null },
      })
      onEvent({
        operationId,
        sequence: 3,
        at: now,
        kind: "completed",
        data: { hits: result.hits, scannedChapters: 1, totalChapters: 1, code: null, message: null },
      })
    }
    return result
  }

  async readerSearchCancel(
    request: ReaderSearchCancelRequest,
  ): Promise<ReaderSearchCancelResultDto> {
    return { operationId: request.operationId, alreadyTerminal: true }
  }

  /** Close is idempotent in the browser mock, matching the wire contract. */
  async sessionClose(request: SessionCloseRequest): Promise<SessionCloseResultDto> {
    this.activeSessions.delete(request.sessionId);
    this.comicManifests.delete(request.sessionId);
    return { schemaVersion: 1, closed: true };
  }

  /**
   * `stream_open`：Browser Demo 无 Rust 代理；返回确定性演示 URI
   * （生产路径由 Tauri client 走 haven-resource://stream/<grant>）。
   */
  async streamOpen(request: SessionOpenRequest): Promise<SessionOpenResultDto> {
    const demoContentUri = DEMO_SESSION_CONTENT[request.mediaItemId];
    if (!demoContentUri) {
      throw new HavenError({
        code: "MEDIA_ITEM_NOT_FOUND",
        userMessage: "媒体条目不存在",
        retryable: false,
      });
    }
    const sessionId = this.nextRuntimeIdentity();
    const session: SessionOpenResultDto = {
      schemaVersion: 1,
      sessionId,
      contentUri: demoContentUri,
      workId: `mock-work-${request.mediaItemId}`,
      editionId: `mock-edition-${request.mediaItemId}`,
      mediaItemId: request.mediaItemId,
      engine: request.engine,
      progress: null,
      ...(request.engine === "playback" ? { streamKind: "direct" as const } : {}),
    };
    this.activeSessions.set(sessionId, session);
    return session;
  }

  async streamClose(request: SessionCloseRequest): Promise<boolean> {
    return this.activeSessions.delete(request.sessionId);
  }

  /** `progress_save`：与后端一致的原子 CAS；无状态时仅 expectedRevision=null 可写。 */
  async progressSave(request: ProgressSaveRequest): Promise<ProgressSaveResult> {
    const current = this.progress.get(request.mediaItemId);
    const currentRevision = current?.revision ?? null;
    if (currentRevision !== request.expectedRevision) {
      throw new HavenError({
        code: "REVISION_CONFLICT",
        userMessage: "进度已被其他会话更新，请刷新后重试",
        retryable: false,
      });
    }
    const revision = `progress-mock-${this.progressRevisionCounter++}`;
    this.progress.set(request.mediaItemId, { request, revision });
    return { revision };
  }

  async progressMarkCompleted(request: ProgressMarkCompletedRequest): Promise<ProgressSaveResult> {
    const current = this.progress.get(request.mediaItemId);
    const revision = `progress-mock-${this.progressRevisionCounter++}`;
    if (current) {
      this.progress.set(request.mediaItemId, {
        ...current,
        request: { ...current.request, completion: "completed" as CompletionWire },
        revision,
      });
    } else {
      this.progress.set(request.mediaItemId, {
        request: {
          mediaItemId: request.mediaItemId,
          locator: request.initialLocator,
          completion: "completed",
          expectedRevision: null,
          keyframe: undefined,
        },
        revision,
      });
    }
    return { revision };
  }

  /** `progress_recent`：按最近保存顺序返回（Mock 仅供 UI/契约开发）。 */
  async progressRecent(request: ProgressRecentRequest): Promise<ProgressSummaryDto[]> {
    const limit = request.limit ?? 50;
    const items: ProgressSummaryDto[] = [];
    for (const [mediaItemId, state] of this.progress) {
      if (items.length >= limit) break;
      items.push({
        mediaItemId,
        completion: state.request.completion ?? "in_progress",
        progressRatio: null,
        revision: state.revision,
        updatedAt: "2026-08-18T00:00:00.000Z",
        locator: state.request.locator,
      });
    }
    return items;
  }

  /** `progress_reset`：业务操作，清进度状态不删实体。 */
  async progressReset(request: ProgressResetRequest): Promise<void> {
    const state = this.progress.get(request.mediaItemId);
    if (!state) {
      throw new HavenError({
        code: "PROGRESS_NOT_FOUND",
        userMessage: "进度不存在",
        retryable: false,
      });
    }
    const reset = { ...state, request: { ...state.request, completion: "not_started" as CompletionWire } };
    this.progress.set(request.mediaItemId, reset);
  }

  /** `history_list`：Mock 返回空列表（演示环境无历史）。 */
  async historyList(_request: HistoryListRequest): Promise<HistoryEntryDto[]> {
    return [];
  }

  /** `history_clear`：Mock 幂等无操作。 */
  async historyClear(): Promise<void> {
    // no-op
  }

  async searchHistoryList(request: SearchHistoryListRequest): Promise<SearchHistoryEntryDto[]> {
    return [...this.searchHistory.values()]
      .sort((a, b) => b.lastUsedAt.localeCompare(a.lastUsedAt))
      .slice(0, Math.min(request.limit ?? 10, 10));
  }

  async searchHistoryRecord(request: SearchHistoryRecordRequest): Promise<SearchHistoryEntryDto> {
    const term = request.term.trim();
    if (!term || term.length > 200) {
      throw new HavenError({ code: "INVALID_ARGUMENT", userMessage: "搜索词不能为空且不能超过 200 个字符", retryable: false });
    }
    const entry = { term, lastUsedAt: new Date().toISOString() };
    this.searchHistory.set(term, entry);
    return entry;
  }

  async searchHistoryRemove(request: SearchHistoryRemoveRequest): Promise<boolean> {
    return this.searchHistory.delete(request.term.trim());
  }

  async searchHistoryClear(): Promise<void> {
    this.searchHistory.clear();
  }

  /** `marker_create`：Mock 仅在内存中追加。 */
  async markerCreate(request: MarkerCreateRequest): Promise<MarkerDto> {
    const markerId = `marker-mock-${this.markerCounter++}`;
    const now = "2026-08-18T00:00:00.000Z";
    return {
      markerId,
      mediaItemId: request.mediaItemId,
      workId: "work-mock",
      editionId: "edition-mock",
      locator: request.locator,
      markerType: request.markerType,
      title: request.title,
      excerpt: request.excerpt,
      note: request.note,
      createdAt: now,
      updatedAt: now,
    };
  }

  /** `marker_list`：Mock 返回该 MediaItem 已创建的标记。 */
  async markerList(request: MarkerListRequest): Promise<MarkerDto[]> {
    return this.markers.filter((m) => m.mediaItemId === request.mediaItemId);
  }

  /** `marker_list_all`：Mock 返回内存中全部标记（足迹聚合）。 */
  async markerListAll(request: MarkerListAllRequest): Promise<MarkerDto[]> {
    const limit = request.limit ?? 100;
    return this.markers.slice(0, limit);
  }

  /** `marker_delete`：Mock 软删除（从内存列表移除）。 */
  async markerDelete(request: MarkerDeleteRequest): Promise<boolean> {
    const idx = this.markers.findIndex((m) => m.markerId === request.markerId);
    if (idx === -1) return false;
    this.markers.splice(idx, 1);
    return true;
  }

  /** `home_get`：Mock 返回空 Continue + listNormal 首页 RecentlyAdded（演示环境零进度数据）。 */
  async homeGet(): Promise<HomeDto> {
    const cards = (listNormal as PageDto<WorkCardDto>).items;
    return {
      schemaVersion: 1,
      continueItems: [],
      recentlyAdded: cards,
      shelves: [],
    };
  }

  /** `settings_get`：已保存 → 当前值 + revision；从未保存 → 共享 Fixture 默认值 + null。 */
  async settingsGet(section: string): Promise<SettingsSnapshot> {
    const parsed = parseSettingsSection(section);
    if (!parsed) {
      throw new HavenError(settingsInvalidArgument as never);
    }
    const state = this.settings.get(parsed);
    if (state) return { value: state.value, revision: state.revision };
    const snapshot: SettingsSnapshot = parsed === "general"
      ? (settingsGeneralDefault as SettingsSnapshot)
      : parsed === "appearance"
        ? (settingsAppearanceDefault as SettingsSnapshot)
        : parsed === "playback"
        ? (settingsPlaybackDefault as SettingsSnapshot)
        : parsed === "reading"
          ? (settingsReadingDefault as SettingsSnapshot)
          : parsed === "comic"
            ? (settingsComicDefault as SettingsSnapshot)
            : parsed === "downloads"
              ? (settingsDownloadsDefault as SettingsSnapshot)
              : (settingsPrivacyDefault as SettingsSnapshot);
    // 守卫先行：Mock 永不以裸 as 返回非契约形状。
    if (!guardSettingsSnapshot(snapshot)) throw new Error("settings default fixture 形状非法");
    return snapshot;
  }

  /** `settings_update`：原子 CAS 语义（expected 校验先于一切；幂等不写不发事件）。 */
  async settingsUpdate(request: SettingsUpdateRequest): Promise<SettingsUpdateResult> {
    const section = parseSettingsSection(request.section);
    if (!section) {
      throw new HavenError(settingsInvalidArgument as never);
    }
    if (request.patch.section !== section) {
      throw new HavenError(settingsInvalidArgument as never);
    }

    // 事务边界内读取 authoritative current（读到的即提交时状态）。
    const current = this.settings.get(section);
    const currentValue = current?.value ?? defaultSettingsValue(section);
    const currentRevision = current?.revision ?? null;

    // expected 校验先于一切（R-MAIN-01：含幂等短路）。
    const revisionMatches = currentRevision === request.expectedRevision;
    if (!revisionMatches) {
      throw new HavenError(settingsConflictError as never);
    }

    const nextValue = applySettingsPatch(currentValue, request.patch);

    // 幂等：值与 authoritative current 相同 → 不写状态，返回当前 revision，不发事件。
    if (settingsValuesEqual(nextValue, currentValue)) {
      return { value: nextValue, revision: currentRevision, changed: false };
    }

    const revision = `set-mock-${this.settingsRevisionCounter++}`;
    this.settings.set(section, { value: nextValue, revision });
    this.settingsChangedEvents.push({
      schemaVersion: 1,
      at: new Date().toISOString(),
      operationId: `set-op-${revision}`,
      sequence: 1,
      section,
      revision,
    });
    const result: SettingsUpdateResult = { value: nextValue, revision, changed: true };
    if (!guardSettingsUpdateResult(result)) throw new Error("settings update result 形状非法");
    return result;
  }

  // ---- 界面自定义字体（BE-INTERFACE-FONT-001）----
  //
  // Mock 环境没有 Native 选择器、没有本机字体目录、也不读取真实文件：
  // 这里只返回确定性的样例数据，让列表/搜索/预览/删除流程可重复。
  // 它不声称任何一项是真实系统事实——Tauri surface 才是唯一事实源。

  async interfaceFontSystemList(): Promise<InterfaceFontFamily[]> {
    return MOCK_SYSTEM_FONT_FAMILIES.map((family) => ({ ...family }));
  }

  async interfaceFontAssetList(): Promise<InterfaceFontAsset[]> {
    return this.fontAssets.map((asset) => ({ ...asset }));
  }

  async interfaceFontAssetImport(): Promise<InterfaceFontImportResult> {
    const asset: InterfaceFontAsset = {
      id: `00000000-0000-4000-8000-${String(0x1000 + this.fontAssetCounter++).padStart(12, "0")}`,
      familyName: `导入示例字体 ${this.fontAssetCounter}`,
      fileName: `MockDisplay${this.fontAssetCounter}.woff2`,
      extension: "woff2",
      mimeType: "font/woff2",
      byteSize: 40960,
      createdAt: Date.now(),
    };
    if (!guardInterfaceFontAsset(asset)) throw new Error("mock 导入字体形状非法");
    this.fontAssets.unshift(asset);
    return { asset: { ...asset }, deduplicated: false };
  }

  async interfaceFontAssetDelete(assetId: string): Promise<void> {
    if (!isCanonicalInterfaceFontAssetId(assetId)) {
      throw new HavenError(interfaceFontAssetNotFoundError);
    }
    // 与真实后端同一不变量：正在被界面设置引用的字体拒绝删除。
    const appearance = this.settings.get("appearance")?.value;
    const activeId = appearance?.section === "appearance" ? appearance.interfaceFontAssetId : undefined;
    if (activeId === assetId) {
      throw new HavenError(interfaceFontAssetInUseError);
    }
    const index = this.fontAssets.findIndex((asset) => asset.id === assetId);
    if (index < 0) {
      throw new HavenError(interfaceFontAssetNotFoundError);
    }
    this.fontAssets.splice(index, 1);
  }

  async preferenceGet(request: PreferenceGetRequest): Promise<PreferenceGetResult> {
    const edition = this.preferences.get(this.preferenceKey("edition", request.mediaItemId, request.editionId));
    const media = this.preferences.get(this.preferenceKey("media_item", request.mediaItemId, request.editionId));
    const readingPatch = media?.readingPatch ?? edition?.readingPatch ?? null;
    const comicPatch = media?.comicPatch ?? edition?.comicPatch ?? null;
    const globalReading = this.settings.get("reading")?.value;
    const globalComic = this.settings.get("comic")?.value;
    const baseReading = globalReading?.section === "reading"
      ? globalReading
      : defaultSettingsValue("reading") as Extract<SettingsValue, { section: "reading" }>;
    const baseComic = globalComic?.section === "comic"
      ? globalComic
      : defaultSettingsValue("comic") as Extract<SettingsValue, { section: "comic" }>;
    // Mirror the Rust effective merge exactly: global -> edition -> media item,
    // applying each sparse patch independently so fields from an edition are not
    // lost when the media item overrides a different field.
    const mergedReading = [edition?.readingPatch, media?.readingPatch]
      .filter((patch): patch is PreferenceReadingPatchWire => patch !== null && patch !== undefined)
      .reduce<Extract<SettingsValue, { section: "reading" }>>(
        (current, patch) => applySettingsPatch(
          current,
          { section: "reading", ...patch },
        ) as Extract<SettingsValue, { section: "reading" }>,
        baseReading,
      );
    const mergedComic = [edition?.comicPatch, media?.comicPatch]
      .filter((patch): patch is PreferenceComicPatchWire => patch !== null && patch !== undefined)
      .reduce<Extract<SettingsValue, { section: "comic" }>>(
        (current, patch) => applySettingsPatch(
          current,
          { section: "comic", ...patch },
        ) as Extract<SettingsValue, { section: "comic" }>,
        baseComic,
      );
    const effectiveReading = toPreferenceReadingSettings(mergedReading);
    const effectiveComic = toPreferenceComicSettings(mergedComic);
    const result: PreferenceGetResult = {
      schemaVersion: 1,
      mediaItemId: request.mediaItemId,
      editionId: request.editionId,
      readingPatch,
      comicPatch,
      editionReadingPatch: edition?.readingPatch ?? null,
      editionComicPatch: edition?.comicPatch ?? null,
      mediaItemReadingPatch: media?.readingPatch ?? null,
      mediaItemComicPatch: media?.comicPatch ?? null,
      effectiveReading,
      effectiveComic,
      mediaItemRevision: media?.revision ?? null,
      editionRevision: edition?.revision ?? null,
    };
    return result;
  }

  async preferenceUpdate(request: PreferenceUpdateRequest): Promise<PreferenceUpdateResult> {
    const key = this.preferenceKey(request.target, request.mediaItemId, request.editionId);
    const current = this.preferences.get(key);
    const currentRevision = current?.revision ?? null;
    if (currentRevision !== request.expectedRevision) {
      throw new HavenError(settingsConflictError as never);
    }
    const next: PreferenceState = {
      readingPatch: request.readingPatch,
      comicPatch: request.comicPatch,
      revision: currentRevision,
    };
    const changed = !current
      || JSON.stringify(current.readingPatch) !== JSON.stringify(next.readingPatch)
      || JSON.stringify(current.comicPatch) !== JSON.stringify(next.comicPatch);
    if (changed) {
      next.revision = "pref-mock-" + this.preferenceRevisionCounter++;
      this.preferences.set(key, next);
    }
    return {
      result: await this.preferenceGet(request),
      target: request.target,
      revision: next.revision,
      changed,
    };
  }

  private preferenceKey(target: PreferenceTargetWire, mediaItemId: string, editionId: string): string {
    return target + ":" + mediaItemId + ":" + editionId;
  }

  // ---- 外观（Appearance Stage 1B）：内存状态 + CAS ----
  //
  // 这套 Mock 只镜像后端的**事实**：CAS、幂等、显式空布局与「从未保存」的区别，
  // 默认布局逐字段等于 `HomeLayout::default()`（三个真实模块）。它不伪造文件选择，
  // 也不在 localStorage / 模块级变量里留第二份事实源。

  async appearanceAssetsList(
    request: AppearanceAssetsListRequestWire,
  ): Promise<AppearanceAssetsWire> {
    const kind = request.kind;
    if (kind !== null && kind !== "font" && kind !== "static_wallpaper" && kind !== "dynamic_wallpaper") {
      throw new HavenError(settingsInvalidArgument as never);
    }
    // 空库就是空列表：不伪造任何资产（与后端空库语义一致）。
    const assets = kind === null
      ? [...this.appearanceAssets]
      : this.appearanceAssets.filter((asset) => asset.kind === kind);
    return { schemaVersion: 1, assets };
  }

  /** 没有原生文件选择器：Mock 不假装选中文件，资产只能由 `appearanceAssets` 预置。 */
  async appearanceAssetImport(
    _request: AppearanceAssetImportRequestWire,
  ): Promise<AppearanceAssetWire> {
    throw new HavenError({
      code: "OPERATION_CANCELLED",
      userMessage: "演示环境没有原生文件选择器，未选择任何文件",
      retryable: false,
    });
  }

  /** 删除：只有规范小写 UUID 是合法 ID；未知（但合法）ID 是幂等空操作。 */
  async appearanceAssetDelete(
    request: AppearanceAssetDeleteRequestWire,
  ): Promise<AppearanceAssetDeleteResultWire> {
    if (!CANONICAL_LOCAL_ID_PATTERN.test(request.assetId)) {
      throw new HavenError({
        code: "INVALID_ID",
        userMessage: "资产标识格式非法",
        retryable: false,
      });
    }
    const index = this.appearanceAssets.findIndex((asset) => asset.assetId === request.assetId);
    if (index < 0) return { deleted: false, fileRemoved: false };
    this.appearanceAssets.splice(index, 1);
    return { deleted: true, fileRemoved: true };
  }

  /** 从未保存过 → 领域默认布局 + `revision=null`；保存过空布局 → 空 modules + 非空 revision。 */
  async homeLayoutGet(): Promise<HomeLayoutSnapshotWire> {
    if (!this.homeLayoutState) return { layout: defaultHomeLayout(), revision: null };
    return {
      layout: cloneHomeLayout(this.homeLayoutState.layout),
      revision: this.homeLayoutState.revision,
    };
  }

  /** 保存（CAS）：expected 对不上 → REVISION_CONFLICT 且零写入；同值 → changed=false。 */
  async homeLayoutSave(request: HomeLayoutSaveRequestWire): Promise<HomeLayoutMutationResultWire> {
    if (!guardHomeLayout(request.layout)) throw mockInvalidHomeLayout();
    const layout = canonicalHomeLayout(request.layout);
    const current = this.homeLayoutState;
    if ((current?.revision ?? null) !== request.expectedRevision) {
      throw new HavenError(settingsConflictError as never);
    }
    if (current && homeLayoutsEqual(current.layout, layout)) {
      return { layout: cloneHomeLayout(layout), revision: current.revision, changed: false };
    }
    const revision = `appearance-mock-${this.homeLayoutRevisionCounter++}`;
    this.homeLayoutState = { layout, revision };
    return { layout: cloneHomeLayout(layout), revision, changed: true };
  }

  /** 重置：删除已保存的自定义；从未保存过时是幂等空操作（changed=false）。 */
  async homeLayoutReset(request: HomeLayoutResetRequestWire): Promise<HomeLayoutMutationResultWire> {
    const current = this.homeLayoutState;
    if ((current?.revision ?? null) !== request.expectedRevision) {
      throw new HavenError(settingsConflictError as never);
    }
    if (!current) return { layout: defaultHomeLayout(), revision: null, changed: false };
    this.homeLayoutState = null;
    return { layout: defaultHomeLayout(), revision: null, changed: true };
  }

  // ---- 设置页总览布局（048）：与首页布局同一套语义，但状态与 revision 各自独立 ----

  /** 从未保存过 → 领域默认总览布局 + `revision=null`；保存过空布局 → 空 modules + 非空 revision。 */
  async overviewLayoutGet(): Promise<OverviewLayoutSnapshotWire> {
    if (!this.overviewLayoutState) return { layout: defaultOverviewLayout(), revision: null };
    return {
      layout: cloneOverviewLayout(this.overviewLayoutState.layout),
      revision: this.overviewLayoutState.revision,
    };
  }

  /** 保存（CAS）：expected 对不上 → REVISION_CONFLICT 且零写入；同值 → changed=false。 */
  async overviewLayoutSave(request: OverviewLayoutSaveRequestWire): Promise<OverviewLayoutMutationResultWire> {
    if (!guardOverviewLayout(request.layout)) throw mockInvalidOverviewLayout();
    const layout = canonicalOverviewLayout(request.layout);
    const current = this.overviewLayoutState;
    if ((current?.revision ?? null) !== request.expectedRevision) {
      throw new HavenError(settingsConflictError as never);
    }
    if (current && overviewLayoutsEqual(current.layout, layout)) {
      return { layout: cloneOverviewLayout(layout), revision: current.revision, changed: false };
    }
    const revision = `overview-mock-${this.overviewLayoutRevisionCounter++}`;
    this.overviewLayoutState = { layout, revision };
    return { layout: cloneOverviewLayout(layout), revision, changed: true };
  }

  /** 重置：删除已保存的自定义；从未保存过时是幂等空操作（changed=false）。 */
  async overviewLayoutReset(request: OverviewLayoutResetRequestWire): Promise<OverviewLayoutMutationResultWire> {
    const current = this.overviewLayoutState;
    if ((current?.revision ?? null) !== request.expectedRevision) {
      throw new HavenError(settingsConflictError as never);
    }
    if (!current) return { layout: defaultOverviewLayout(), revision: null, changed: false };
    this.overviewLayoutState = null;
    return { layout: defaultOverviewLayout(), revision: null, changed: true };
  }

  /**
   * 阅读总览：Mock 没有已登记的内容会话事实，所以恒定返回**显式空态**——不伪造任何
   * 阅读分钟数。空态由 `emptyReadingOverview` 统一构造（与领域 `aggregate` 在
   * `session_count == 0` 时的输出逐字段一致），因此页面在演示环境走的是与真实后端
   * 同一条「无数据」分支。
   */
  async readingOverviewGet(request: ReadingOverviewGetRequestWire): Promise<ReadingOverviewWire> {
    // 越界窗口后端会拒绝（READING_OVERVIEW_INVALID_RANGE）；Mock 不该给出假结果。
    if (!guardReadingOverviewGetRequest(request)) {
      throw new HavenError({
        code: "READING_OVERVIEW_INVALID_RANGE",
        userMessage: "总览统计窗口超出允许范围",
        retryable: false,
      });
    }
    return emptyReadingOverview(request, Date.now());
  }

  async storageLocationList(): Promise<StorageLocationDto[]> {
    return [{
      locationId: "0196f0d2-0000-7000-8000-00000000d100",
      displayName: "本地离线库",
      providerType: "local",
      status: "connected",
    }];
  }

  async downloadCreate(request: DownloadCreateRequest): Promise<DownloadTaskDto> {
    const existing = this.downloadTasks.find((task) => (
      task.sourceResourceId === request.sourceResourceId
      && task.targetStorageId === request.targetStorageId
      && !["failed", "cancelled"].includes(task.state)
      && (task.state !== "completed" || task.offlineResourceId !== null)
    ));
    if (existing) return existing;
    const now = new Date().toISOString();
    const task: DownloadTaskDto = {
      schemaVersion: 1,
      taskId: `0196f0d2-0000-7000-8000-${(this.downloadTasks.length + 1).toString(16).padStart(12, "0")}`,
      workId: null,
      editionId: null,
      mediaItemId: RESOURCE_FIXTURE_MEDIA_ITEM_ID,
      sourceResourceId: request.sourceResourceId,
      targetStorageId: request.targetStorageId,
      offlineResourceId: null,
      title: "新下载任务",
      mediaType: "book",
      category: "book",
      posterUri: null,
      state: "queued",
      bytesTotal: null,
      bytesDownloaded: 0,
      progressRatio: null,
      speedBps: null,
      etaSeconds: null,
      createdAt: now,
      updatedAt: now,
    };
    this.downloadTasks.unshift(task);
    return task;
  }

  async downloadList(request: DownloadListRequest): Promise<DownloadTaskDto[]> {
    return this.downloadTasks.slice(0, request.limit ?? 100);
  }

  async downloadPause(request: DownloadTaskActionRequest): Promise<DownloadTaskDto> {
    return this.setDownloadState(request.taskId, "paused");
  }

  async downloadResume(request: DownloadTaskActionRequest): Promise<DownloadTaskDto> {
    return this.setDownloadState(request.taskId, "downloading");
  }

  async downloadCancel(request: DownloadTaskActionRequest): Promise<DownloadTaskDto> {
    return this.setDownloadState(request.taskId, "cancelled");
  }

  async downloadRetry(request: DownloadTaskActionRequest): Promise<DownloadTaskDto> {
    return this.setDownloadState(request.taskId, "queued");
  }

  async downloadRemoveRecord(request: DownloadTaskActionRequest): Promise<DownloadMutationResultDto> {
    const index = this.downloadTasks.findIndex((task) => task.taskId === request.taskId);
    if (index === -1) throw this.downloadNotFound();
    if (!["completed", "failed", "cancelled"].includes(this.downloadTasks[index].state)) {
      throw new HavenError({
        code: "DOWNLOAD_TASK_NOT_REMOVABLE",
        userMessage: "进行中的下载任务不能从列表移除",
        retryable: false,
      });
    }
    const [task] = this.downloadTasks.splice(index, 1);
    return {
      schemaVersion: 1,
      taskId: task.taskId,
      recordRemoved: true,
      offlineResourceRemoved: false,
    };
  }

  async downloadDeleteOffline(request: DownloadTaskActionRequest): Promise<DownloadMutationResultDto> {
    const index = this.downloadTasks.findIndex((task) => task.taskId === request.taskId);
    if (index === -1) throw this.downloadNotFound();
    if (this.downloadTasks[index].state !== "completed" || !this.downloadTasks[index].offlineResourceId) {
      throw new HavenError({
        code: "DOWNLOAD_NOT_COMPLETED",
        userMessage: "下载尚未完成，没有可管理的离线文件",
        retryable: false,
      });
    }
    this.downloadTasks[index] = { ...this.downloadTasks[index], offlineResourceId: null };
    return {
      schemaVersion: 1,
      taskId: request.taskId,
      recordRemoved: false,
      offlineResourceRemoved: true,
    };
  }

  async downloadRevealOffline(request: DownloadTaskActionRequest): Promise<DownloadRevealResultDto> {
    const task = this.downloadTasks.find((item) => item.taskId === request.taskId);
    if (!task) {
      throw this.downloadNotFound();
    }
    if (task.state !== "completed" || !task.offlineResourceId) {
      throw new HavenError({
        code: "DOWNLOAD_NOT_COMPLETED",
        userMessage: "下载尚未完成，没有可管理的离线文件",
        retryable: false,
      });
    }
    return { schemaVersion: 1, taskId: request.taskId };
  }

  async downloadSubscribe(
    _subscriptionId: string,
    _onEvent: (event: DownloadEvent) => void,
  ): Promise<() => Promise<void>> {
    // Mock 环境没有 Rust Worker；页面仍通过列表 Query 展示稳定的演示任务。
    return async () => undefined;
  }

  // ---- v0.2 契约冻结（契约 §36；CONTRACT-V02-*）----

  /** `source_registry_list`：共享 Fixture 目录 + 本地 enabled 叠加（深拷贝防调用方改写）。 */
  async sourceRegistryList(): Promise<SourceRegistryDto> {
    const registry = JSON.parse(JSON.stringify(sourceRegistryNormal)) as SourceRegistryDto;
    registry.sources = registry.sources.map((source) => ({
      ...source,
      enabled: this.sourceEnabled.get(source.sourceId) ?? source.enabled,
    }));
    for (const [sourceId, source] of this.customSources) {
      const feed = source.kind === "feed";
      const comic = source.kind === "komga" || source.kind === "kavita";
      const comicLabel = source.kind === "kavita" ? "Kavita" : "Komga";
      const descriptor: SourceDescriptorDto = {
        sourceId,
        displayName: source.displayName,
        kinds: feed || comic
          ? ["search", "online_read", "offline_download"]
          : ["search", "offline_download"],
        categories: feed ? ["periodical"] : comic ? ["comic"] : ["book"],
        mode: "single",
        notes: feed
          ? "这是你添加的 RSS/Atom 订阅源；Haven 只读取该订阅源自身提供的条目内容，不会打开条目链接指向的网页。地址必须使用 HTTPS，且不能带查询串。"
          : comic
            ? `这是你添加的 ${comicLabel} 漫画库；搜索、章节列表、在线逐页阅读与 CBZ 下载都只访问该地址。API key 只保存在系统凭据管理器并作为请求头发送，不会出现在地址、日志或来源身份里。`
            : "这是你添加的自定义 OPDS 书库；可在来源设置中编辑地址或配置访问凭据。",
        enabled: this.customSourceEnabled.get(sourceId) ?? false,
        health: "unknown",
        endpointConfigured: source.endpoint.length > 0,
        credentialConfigured: this.customCredentialConfigured.has(sourceId),
        lastChecked: null,
        latencyMs: null,
        successRate: null,
      };
      registry.sources.push(descriptor);
    }
    return registry;
  }

  /** `source_registry_set`：幂等；未知 sourceId → INVALID_ARGUMENT（与 fixture 同码）。 */
  async sourceRegistrySet(request: SourceRegistrySetRequest): Promise<SourceRegistrySetResult> {
    const registry = await this.sourceRegistryList();
    if (!registry.sources.some((source) => source.sourceId === request.sourceId)) {
      throw new HavenError(sourceSetErrorUnknown as never);
    }
    if (this.customSources.has(request.sourceId)) {
      this.customSourceEnabled.set(request.sourceId, request.enabled);
    } else {
      this.sourceEnabled.set(request.sourceId, request.enabled);
    }
    return { sourceId: request.sourceId, enabled: request.enabled };
  }

  /**
   * `search_source_start`：镜像 V2-A 后端语义——started 同步先发；
   * 已启用来源逐个发零结果 source_result；终态 completed。
   * V2-A 无参与者，不伪造命中数据。
   */
  async searchSourceStart(
    request: SearchSourceStartRequest,
    onEvent?: (event: SearchSourceEvent) => void,
  ): Promise<SearchStartResultDto> {
    const query = request.query.trim();
    if (!query || query.length > 200) {
      throw new HavenError({
        code: "INVALID_ARGUMENT",
        userMessage: "搜索词非法",
        retryable: false,
      });
    }
    if (request.limitPerSource !== null && (request.limitPerSource === 0 || request.limitPerSource > 50)) {
      throw new HavenError({
        code: "INVALID_ARGUMENT",
        userMessage: "单来源数量超出允许范围",
        retryable: false,
      });
    }
    const queryKey = `${query}\u0001${request.category ?? ""}\u0001${request.limitPerSource ?? 20}`;
    for (const op of this.searchOperations.values()) {
      if (!op.finished && op.queryKey === queryKey) {
        return { operationId: this.operationIdOf(op), taskId: "", alreadyRunning: true };
      }
    }
    const operationId = `op-search-mock-${this.searchOperationCounter++}`;
    const record = { queryKey, finished: false, onEvent, nextSequence: 1 };
    this.searchOperations.set(operationId, record);

    const emit = (kind: SearchSourceEventKind, sourceId: string | null) => {
      if (!record.onEvent || record.finished) return;
      const event: SearchSourceEvent = {
        operationId,
        sequence: record.nextSequence++,
        at: new Date().toISOString(),
        kind,
        data: { sourceId, works: [], code: null, message: null },
      };
      record.onEvent(event);
    };

    emit("started", null);
    const registry = await this.sourceRegistryList();
    const deliverRest = async (): Promise<void> => {
      for (const source of registry.sources) {
        if (record.finished) return;
        if (!source.enabled) continue;
        emit("source_result", source.sourceId);
        await Promise.resolve();
      }
      if (!record.finished) {
        // 先发终态再落 finished 标志：emit 守卫会拦截 finished 后的投递。
        emit("completed", null);
        record.finished = true;
      }
    };
    void deliverRest();
    return { operationId, taskId: `task-${operationId}`, alreadyRunning: false };
  }

  /** `search_source_cancel`：幂等；未知 → RESOURCE_NOT_FOUND；运行中发 cancelled 终态。 */
  async searchSourceCancel(
    request: SearchSourceCancelRequest,
  ): Promise<SearchSourceCancelResultDto> {
    const record = this.searchOperations.get(request.operationId);
    if (!record) {
      throw new HavenError({
        code: "RESOURCE_NOT_FOUND",
        userMessage: "搜索操作不存在",
        retryable: false,
      });
    }
    if (record.finished) {
      return { operationId: request.operationId, alreadyTerminal: true };
    }
    record.finished = true;
    if (record.onEvent) {
      record.onEvent({
        operationId: request.operationId,
        sequence: record.nextSequence++,
        at: new Date().toISOString(),
        kind: "cancelled",
        data: { sourceId: null, works: [], code: null, message: null },
      });
    }
    return { operationId: request.operationId, alreadyTerminal: false };
  }

  /** `credential_status`：内存 profile 集合投影；凭据存储不提供写入时间 → updatedAt null。 */
  async credentialStatus(request: CredentialStatusRequest): Promise<CredentialStatusDto> {
    return {
      configured: this.credentialProfiles.has(this.credentialKey(request)),
      updatedAt: null,
    };
  }

  /** `credential_set`：幂等覆盖；secret 不落任何可读状态。 */
  async credentialSet(request: CredentialSetRequest): Promise<void> {
    if (request.secret.length === 0) {
      throw new HavenError({
        code: "INVALID_ARGUMENT",
        userMessage: "凭据内容不能为空",
        retryable: false,
      });
    }
    this.credentialProfiles.add(this.credentialKey(request));
  }

  /** `credential_delete`：幂等；不存在视为成功。 */
  async credentialDelete(request: CredentialDeleteRequest): Promise<void> {
    this.credentialProfiles.delete(this.credentialKey(request));
  }

  // ---- A2 AI Provider 基础切片 ----
  // Mock 只复现 Application service 的**决策**（CAS、空态、凭据存在性），
  // 不复现网络：模型目录直接来自共享 fixture，绝不会从模型名推断能力。
  //
  // 端点校验在这里只做形状检查（http/https、无查询参数）。主机策略
  // （公网、多标签、允许端口、DNS 固定）由 Rust 侧唯一实现；在 TS 里再写一份
  // 安全策略只会制造两套会漂移的真相。

  async aiProviderProfileList(): Promise<AiProviderProfileListResultDto> {
    const profiles = [...this.aiProfiles.values()]
      .sort((left, right) => left.profileId.localeCompare(right.profileId))
      .map((profile) => this.withCredentialState(profile));
    return { schemaVersion: 1, profiles };
  }

  async aiProviderProfileGet(request: AiProviderProfileGetRequest): Promise<AiProviderProfileDto> {
    return this.withCredentialState(this.requireAiProfile(request.profileId));
  }

  async aiProviderProfileUpsert(
    request: AiProviderProfileUpsertRequest,
  ): Promise<AiProviderProfileDto> {
    if (request.kind !== "openai_compatible") {
      throw new HavenError({
        code: "INVALID_ARGUMENT",
        userMessage: "当前只支持 OpenAI 兼容协议",
        retryable: false,
      });
    }
    if (!/^[A-Za-z0-9][A-Za-z0-9_-]*$/.test(request.profileId)) {
      throw new HavenError({
        code: "AI_PROVIDER_PROFILE_ID_INVALID",
        userMessage: "profile id 只允许 ASCII 字母、数字、连字符与下划线",
        retryable: false,
      });
    }
    if (!/^https?:\/\/[^/?#\s]+/.test(request.endpoint) || request.endpoint.includes("?")) {
      throw new HavenError({
        code: "AI_PROVIDER_ENDPOINT_INVALID",
        userMessage: "API 地址必须是 http/https URL，且不得包含查询参数",
        retryable: false,
      });
    }
    const current = this.aiProfiles.get(request.profileId);
    const expected = request.expectedRevision;
    const matches = expected === null ? current === undefined : current?.revision === expected;
    if (!matches) {
      throw new HavenError({
        code: "AI_PROVIDER_PROFILE_REVISION_CONFLICT",
        userMessage: "AI Provider 配置已被其它操作修改，请重新读取后再试",
        retryable: true,
      });
    }
    const revision = `mock-ai-rev-${this.aiProfileRevisionCounter++}`;
    const now = "2026-09-18T00:00:00Z";
    const stored: AiProviderProfileDto = {
      schemaVersion: 1,
      profileId: request.profileId,
      displayName: request.displayName,
      kind: request.kind,
      endpoint: request.endpoint.replace(/\/+$/, ""),
      enabled: request.enabled,
      selectedModelId: request.selectedModelId,
      credentialConfigured: false,
      revision,
      createdAt: current?.createdAt ?? now,
      updatedAt: now,
    };
    this.aiProfiles.set(request.profileId, stored);
    return this.withCredentialState(stored);
  }

  async aiProviderProfileDelete(
    request: AiProviderProfileDeleteRequest,
  ): Promise<AiProviderProfileDeleteResultDto> {
    const profile = this.requireAiProfile(request.profileId);
    // 与后端同样的 CAS 语义：过期版本在**清理凭据之前**被拒绝。
    // Mock 若在这里放行，UI 的冲突分支在开发环境里永远不会被走到，
    // 而后端会拒绝并保留数据 —— 这种分歧比"少一个 Mock 分支"危险得多。
    if (request.expectedRevision !== null && request.expectedRevision !== profile.revision) {
      throw new HavenError({
        code: "AI_PROVIDER_PROFILE_REVISION_CONFLICT",
        userMessage: "AI Provider 配置已被其它操作修改，请重新读取后再试",
        retryable: true,
      });
    }
    // 与后端同顺序：先清理凭据、再删除行。凭据清理失败会中止整次删除。
    const credentialDeleted = this.credentialProfiles.delete(`ai:${profile.profileId}`);
    this.aiProfiles.delete(profile.profileId);
    return { profileId: profile.profileId, credentialDeleted };
  }

  async aiProviderModelsList(
    request: AiProviderModelsListRequest,
  ): Promise<AiProviderModelsCatalogDto> {
    const profile = this.requireAiProfile(request.profileId);
    if (!profile.enabled) {
      return { schemaVersion: 1, profileId: profile.profileId, state: "disabled", models: [] };
    }
    if (!this.credentialProfiles.has(`ai:${profile.profileId}`)) {
      // 没有密钥就不发请求，也不猜模型：这是 UI 显示「无可用模型」的正常路径。
      return { schemaVersion: 1, profileId: profile.profileId, state: "no_credential", models: [] };
    }
    // 共享 fixture 目录：能力字段全部来自 fixture 的显式声明。
    const models = aiProviderModelsReady.models.map((model) => ({
      modelId: model.modelId,
      displayName: model.displayName,
      created: model.created,
      ownedBy: model.ownedBy,
      chat: model.chat as AiModelCapabilityDto,
      vision: model.vision as AiModelCapabilityDto,
      embedding: model.embedding as AiModelCapabilityDto,
    }));
    return models.length === 0
      ? { schemaVersion: 1, profileId: profile.profileId, state: "empty", models: [] }
      : { schemaVersion: 1, profileId: profile.profileId, state: "ready", models };
  }

  private requireAiProfile(profileId: string): AiProviderProfileDto {
    const profile = this.aiProfiles.get(profileId);
    if (!profile) {
      throw new HavenError({
        code: "AI_PROVIDER_PROFILE_NOT_FOUND",
        userMessage: `未找到 AI Provider 配置：${profileId}`,
        retryable: false,
      });
    }
    return profile;
  }

  private withCredentialState(profile: AiProviderProfileDto): AiProviderProfileDto {
    return {
      ...profile,
      credentialConfigured: this.credentialProfiles.has(`ai:${profile.profileId}`),
    };
  }

  /** `media_state_get`：已知作品返回共享 Fixture 聚合；其余诚实空态。 */
  async mediaStateGet(request: MediaStateGetRequest): Promise<MediaStateDto> {
    if (request.workId.length !== 36) {
      throw new HavenError({
        code: "WORK_NOT_FOUND",
        userMessage: "作品不存在",
        retryable: false,
      });
    }
    if (request.workId === (listNormal as PageDto<WorkCardDto>).items[0]?.workId) {
      const state = JSON.parse(JSON.stringify(mediaStateNormal)) as MediaStateDto;
      state.workId = request.workId;
      return state;
    }
    return {
      schemaVersion: 2,
      workId: request.workId,
      favorite: false,
      progress: null,
      historySummary: null,
      markerCount: 0,
      rating: null,
    };
  }

  /** `enrichment_status`：mock 无流水线执行，诚实返回 []（契约 §36.8）。 */
  async enrichmentStatus(_request: EnrichmentStatusRequest): Promise<EnrichmentStateDto[]> {
    return [];
  }

  /** `source_registry_set_endpoint`：端点只入内存 Map；响应仅布尔投影。 */
  async sourceRegistrySetEndpoint(
    request: SourceEndpointSetRequest,
  ): Promise<SourceEndpointSetResult> {
    const registry = await this.sourceRegistryList();
    if (!registry.sources.some((source) => source.sourceId === request.sourceId)) {
      throw new HavenError(sourceSetErrorUnknown as never);
    }
    const endpoint = request.endpoint.trim();
    if (endpoint && !endpoint.startsWith("http://") && !endpoint.startsWith("https://")) {
      throw new HavenError({
        code: "INVALID_ARGUMENT",
        userMessage: "端点必须是 http/https 绝对地址",
        retryable: false,
      });
    }
    if (endpoint) {
      this.sourceEndpoints.set(request.sourceId, endpoint.replace(/\/+$/, ""));
    } else {
      this.sourceEndpoints.delete(request.sourceId);
    }
    return {
      sourceId: request.sourceId,
      endpointConfigured: this.sourceEndpoints.has(request.sourceId),
    };
  }

  // ---- V2-H 收尾批次：自定义来源（OPDS 书库 / RSS/Atom 订阅源；Mock：内存态）----

  private customSources = new Map<
    string,
    { displayName: string; endpoint: string; kind: "opds" | "feed" | "komga" | "kavita" }
  >();
  private customSourceEnabled = new Map<string, boolean>();
  private customCredentialConfigured = new Set<string>();

  /** 订阅源地址策略（与后端一致）：HTTPS、无 userinfo/fragment、无查询串。 */
  private validateFeedEndpoint(endpoint: string): void {
    if (!endpoint.startsWith("https://")) {
      throw new HavenError({
        code: "INVALID_ARGUMENT",
        userMessage: "订阅源地址必须使用 HTTPS",
        retryable: false,
      });
    }
    if (endpoint.includes("?")) {
      throw new HavenError({
        code: "INVALID_ARGUMENT",
        userMessage: "订阅源地址不能包含查询串",
        retryable: false,
      });
    }
    // 只检查 authority 里的 userinfo 与整串的 fragment 分隔符，避免误伤路径中的
    // 合法字符。真实校验由后端 URL 策略完成。
    const authority = endpoint.slice("https://".length).split(/[/?#]/)[0];
    if (authority.includes("@") || endpoint.includes("#")) {
      throw new HavenError({
        code: "INVALID_ARGUMENT",
        userMessage: "订阅源地址不安全",
        retryable: false,
      });
    }
  }

  async sourceAdd(
    request: import("./generated/wire").SourceAddRequest,
  ): Promise<import("./generated/wire").SourceAddResult> {
    const name = request.displayName.trim();
    const endpoint = request.endpoint.trim();
    // 缺省 kind 仍是 OPDS：与新增该字段之前的请求保持向后兼容。
    const kind = request.kind ?? "opds";
    if (!name || name.length > 100) {
      throw new HavenError({ code: "INVALID_ARGUMENT", userMessage: "显示名非法", retryable: false });
    }
    if (kind === "feed" || kind === "komga" || kind === "kavita") {
      // 订阅源与自托管漫画库共用同一条地址策略：HTTPS、无 userinfo/fragment、
      // 无查询串（API key 只走请求头，不需要写进地址）。
      this.validateFeedEndpoint(endpoint);
    } else if (!endpoint.startsWith("http://") && !endpoint.startsWith("https://")) {
      throw new HavenError({
        code: "INVALID_ARGUMENT",
        userMessage: "端点必须是 http/https 绝对地址",
        retryable: false,
      });
    }
    if (this.customSources.size >= 20) {
      throw new HavenError({
        code: "INVALID_ARGUMENT",
        userMessage: "自定义来源数量已达上限",
        retryable: false,
      });
    }
    const suffix = `${Date.now().toString(16)}${this.customSources.size}`;
    const sourceId =
      kind === "feed"
        ? `custom-feed-${suffix}`
        : kind === "komga"
          ? `custom-komga-${suffix}`
          : kind === "kavita"
            ? `custom-kavita-${suffix}`
            : `custom-${suffix}`;
    this.customSources.set(sourceId, { displayName: name, endpoint, kind });
    this.customSourceEnabled.set(sourceId, false);
    return { schemaVersion: 1, sourceId };
  }

  async sourceUpdate(
    request: import("./generated/wire").SourceUpdateRequest,
  ): Promise<import("./generated/wire").SourceUpdateResult> {
    const record = this.customSources.get(request.sourceId);
    if (!record || !request.sourceId.startsWith("custom-")) {
      if (request.sourceId.startsWith("custom-")) {
        throw new HavenError({ code: "RESOURCE_NOT_FOUND", userMessage: "自定义来源不存在", retryable: false });
      }
      throw new HavenError(sourceSetErrorUnknown as never);
    }
    if (request.displayName !== null) {
      const name = request.displayName.trim();
      if (!name || name.length > 100) {
        throw new HavenError({ code: "INVALID_ARGUMENT", userMessage: "显示名非法", retryable: false });
      }
      record.displayName = name;
    }
    if (request.endpoint !== null) {
      const nextEndpoint = request.endpoint.trim();
      if (record.kind === "feed" || record.kind === "komga" || record.kind === "kavita") {
        this.validateFeedEndpoint(nextEndpoint);
      } else if (!nextEndpoint.startsWith("http://") && !nextEndpoint.startsWith("https://")) {
        throw new HavenError({
          code: "INVALID_ARGUMENT",
          userMessage: "端点必须是 http/https 绝对地址",
          retryable: false,
        });
      }
      record.endpoint = nextEndpoint.replace(/\/+$/, "");
    }
    return { schemaVersion: 1, sourceId: request.sourceId };
  }

  async sourceRemove(
    request: import("./generated/wire").SourceRemoveRequest,
  ): Promise<import("./generated/wire").SourceRemoveResult> {
    const credentialDeleted = this.customCredentialConfigured.delete(request.sourceId);
    const removed = this.customSources.delete(request.sourceId);
    this.customSourceEnabled.delete(request.sourceId);
    if (!removed && !credentialDeleted) {
      throw new HavenError({ code: "RESOURCE_NOT_FOUND", userMessage: "自定义来源不存在", retryable: false });
    }
    return { schemaVersion: 1, sourceId: request.sourceId, credentialDeleted };
  }

  async sourceSetCredential(request: import("./generated/wire").SourceSetCredentialRequest): Promise<void> {
    const record = this.customSources.get(request.sourceId);
    if (!record) {
      throw new HavenError({ code: "RESOURCE_NOT_FOUND", userMessage: "自定义来源不存在", retryable: false });
    }
    if (record.kind === "feed") {
      // 订阅源没有受控的 HTTP 认证路径：写入一条不会被任何请求消费的凭据
      // 只会制造假能力。
      throw new HavenError({
        code: "INVALID_ARGUMENT",
        userMessage: "订阅源不使用单独的访问凭据",
        retryable: false,
      });
    }
    if (request.secret === null) {
      this.customCredentialConfigured.delete(request.sourceId);
      return;
    }
    if (request.secret.length === 0) {
      throw new HavenError({ code: "INVALID_ARGUMENT", userMessage: "凭据内容不能为空", retryable: false });
    }
    this.customCredentialConfigured.add(request.sourceId);
  }

  /**
   * `source_work_import`：Mock 环境无真实入库；候选句柄形状合法时返回
   * 确定性假身份（Browser Demo 隔离，不进生产路径）。
   */
  async sourceWorkImport(request: SourceWorkImportRequest): Promise<SourceWorkImportResult> {
    if (request.operationId.length === 0) {
      throw new HavenError({
        code: "RESOURCE_NOT_FOUND",
        userMessage: "搜索候选不存在或已过期",
        retryable: false,
      });
    }
    const suffix = ((request.index + 1) >>> 0).toString(16).padStart(12, "0");
    return {
      schemaVersion: 1,
      workId: `0196f0d2-0000-7000-8000-${suffix}`,
      mediaItemId: `0196f0d2-0000-7000-8000-${(suffix + "01").slice(-12)}`,
    };
  }

  private operationIdOf(record: { onEvent?: unknown }): string {
    for (const [id, value] of this.searchOperations) {
      if (value === record) return id;
    }
    return "";
  }

  private credentialKey(
    request: CredentialStatusRequest | CredentialSetRequest | CredentialDeleteRequest,
  ): string {
    return `${request.provider}:${request.profileId ?? "default"}`;
  }

  private setDownloadState(taskId: string, state: DownloadStateDto): DownloadTaskDto {
    const index = this.downloadTasks.findIndex((task) => task.taskId === taskId);
    if (index === -1) {
      throw this.downloadNotFound();
    }
    const updated = {
      ...this.downloadTasks[index],
      state,
      speedBps: state === "downloading" ? this.downloadTasks[index].speedBps : null,
      etaSeconds: state === "downloading" ? this.downloadTasks[index].etaSeconds : null,
      updatedAt: new Date().toISOString(),
    };
    this.downloadTasks[index] = updated;
    return updated;
  }

  private downloadNotFound(): HavenError {
    return new HavenError({
      code: "DOWNLOAD_TASK_NOT_FOUND",
      userMessage: "下载任务不存在",
      retryable: false,
    });
  }

  async trendingBoardsGet(): Promise<import("./generated/wire").TrendingBoardsDto> {
    return { schemaVersion: 1, boards: [] };
  }

  async trendingBoardsRefresh(): Promise<import("./generated/wire").TrendingBoardsDto> {
    return { schemaVersion: 1, boards: [] };
  }

  async appInfoGet(): Promise<AppInfoDto> {
    return appInfoMock as AppInfoDto;
  }

  async errorReportPreviewGet(request: ErrorReportPreviewRequest): Promise<ErrorReportPreviewDto> {
    const reportId = `mock-report-${this.runtimeIdentityCounter++}`;
    return {
      schemaVersion: 1,
      reportId,
      level: request.level,
      createdAt: new Date().toISOString(),
      appVersion: "Mock",
      operatingSystem: "浏览器 Mock",
      runtimeMode: "mock",
      stableErrorCodes: request.stableErrorCodes,
      errorSummary: request.stableErrorCodes.length
        ? `稳定错误码：${request.stableErrorCodes.join("、")}`
        : "未捕获到稳定错误码；本报告只包含脱敏运行信息",
      redaction: {
        status: "passed",
        removedFields: ["absolute_paths", "credentials", "cookies", "signed_urls", "user_content"],
        containsSensitiveData: false,
      },
      details: request.level === "basic" ? null : {
        protocolVersion: "mock",
        databaseVersion: "mock",
        sourcePackVersion: null,
        diagnosticLines: [],
      },
      requiresConfirmation: true,
    };
  }

  async errorReportConfirm(request: ErrorReportConfirmRequest): Promise<ErrorReportConfirmResultDto> {
    if (!request.reportId.startsWith("mock-report-")) {
      throw new HavenError({ code: "ERROR_REPORT_EXPIRED", userMessage: "诊断报告已失效，请重新生成", retryable: true });
    }
    return { schemaVersion: 1, reportId: request.reportId, confirmed: true };
  }

  async errorReportExport(request: ErrorReportActionRequest): Promise<ErrorReportActionResultDto> {
    if (!request.reportId.startsWith("mock-report-")) {
      throw new HavenError({ code: "ERROR_REPORT_EXPIRED", userMessage: "诊断报告已失效，请重新生成", retryable: true });
    }
    throw new HavenError({ code: "ERROR_REPORT_EXPORT_UNAVAILABLE", userMessage: "浏览器预览不写入本地报告文件", retryable: false });
  }

  async errorReportOpenIssue(request: ErrorReportActionRequest): Promise<ErrorReportActionResultDto> {
    if (!request.reportId.startsWith("mock-report-")) {
      throw new HavenError({ code: "ERROR_REPORT_EXPIRED", userMessage: "诊断报告已失效，请重新生成", retryable: true });
    }
    throw new HavenError({ code: "ERROR_REPORT_ISSUE_UNAVAILABLE", userMessage: "浏览器预览不打开外部 Issue 页面", retryable: false });
  }

  async openDataDirectory(): Promise<void> {
    throw new HavenError({ code: "APP_DIRECTORY_UNAVAILABLE", userMessage: "Mock 环境不提供本地目录", retryable: false });
  }

  async openLogsDirectory(): Promise<void> {
    throw new HavenError({ code: "APP_DIRECTORY_UNAVAILABLE", userMessage: "Mock 环境不提供本地目录", retryable: false });
  }

  async openCacheDirectory(): Promise<void> {
    throw new HavenError({ code: "APP_DIRECTORY_UNAVAILABLE", userMessage: "Mock 环境不提供本地目录", retryable: false });
  }

  async cacheClear(scope: CacheScopeDto): Promise<CacheClearResultDto> {
    if (scope !== "artwork") {
      throw new HavenError({ code: "CACHE_SCOPE_UNAVAILABLE", userMessage: "当前版本没有可清理的该类技术缓存", retryable: false });
    }
    return { scope, removedEntries: 0n };
  }

  async videoScreenshotBegin(): Promise<VideoScreenshotBeginResultDto> {
    const uploadId = `mock-screenshot-${this.screenshotUploadCounter++}`;
    this.screenshotUploads.set(uploadId, { nextSequence: 0, totalBytes: 0 });
    return { schemaVersion: 1, uploadId, maxChunkBytes: 64 * 1024, maxTotalBytes: 8 * 1024 * 1024 };
  }

  async videoScreenshotChunk(request: VideoScreenshotChunkRequest): Promise<void> {
    const upload = this.screenshotUploads.get(request.uploadId);
    if (!upload) {
      throw new HavenError({ code: "SCREENSHOT_UPLOAD_EXPIRED", userMessage: "截图上传已失效", retryable: true });
    }
    if (request.sequence !== upload.nextSequence || request.bytes.length > 64 * 1024) {
      throw new HavenError({ code: "SCREENSHOT_PAYLOAD_INVALID", userMessage: "截图分块无效", retryable: false });
    }
    upload.totalBytes += request.bytes.length;
    if (upload.totalBytes > 8 * 1024 * 1024) {
      this.screenshotUploads.delete(request.uploadId);
      throw new HavenError({ code: "SCREENSHOT_TOO_LARGE", userMessage: "截图数据过大", retryable: false });
    }
    upload.nextSequence += 1;
  }

  async videoScreenshotCommit(uploadId: string): Promise<VideoScreenshotResultDto> {
    this.screenshotUploads.delete(uploadId);
    throw new HavenError({ code: "SCREENSHOT_SAVE_FAILED", userMessage: "浏览器预览不提供本地截图保存", retryable: false });
  }

  async videoScreenshotCancel(uploadId: string): Promise<void> {
    this.screenshotUploads.delete(uploadId);
  }

  async updateCheck(): Promise<UpdaterCheckResult> {
    throw new HavenError({
      code: "UPDATER_UNAVAILABLE",
      userMessage: "浏览器预览不连接自动更新服务，请使用独立桌面版本",
      retryable: false,
    });
  }

  async updateInstall(): Promise<UpdaterInstallResult> {
    throw new HavenError({
      code: "UPDATER_UNAVAILABLE",
      userMessage: "浏览器预览不安装桌面更新",
      retryable: false,
    });
  }

  async castDiscover(): Promise<import("./generated/wire").CastDiscoverResult> {
    return { schemaVersion: 1, devices: [] };
  }

  async castPlay(): Promise<import("./generated/wire").CastPlayResult> {
    throw new HavenError({ code: "CAST_DEVICE_UNREACHABLE", userMessage: "演示环境不支持投屏", retryable: false });
  }

  async castStatus(): Promise<import("./generated/wire").CastStatusDto> {
    return { schemaVersion: 1, transportState: "unknown", positionMs: null, durationMs: null };
  }

  async castStop(): Promise<import("./generated/wire").CastStopResult> {
    return { schemaVersion: 1, stopped: true };
  }

  // ---- A1 智能配置推荐（Agent 全局设置 Typed IPC，Mock/开发契约） ----
  //
  // 这里实现的是**契约形状**，不是模型能力：Mock 不调用任何 Provider，也不伪造
  // 模型名或"AI 已生成"结论。它只让开发与契约测试能走通
  // Context → Proposal → 用户批准 → Receipt 的 Typed 闭环，digest 与回执都来自
  // 本对象内的确定性状态机（页面必须把它标注为 Mock/预览）。
  //
  // 与真实实现的差异（刻意保留）：
  // - contextId/contextHash 是 Mock 自算的确定性摘要，不是 Rust 的 SHA-256 canonical 摘要；
  // - 没有一次性 Approval Token 参与（真实路径由 Rust 内部生成并消费）。

  async agentCapabilityManifestGet(): Promise<AgentCapabilityManifestDto> {
    return {
      agentApiVersion: 1,
      capabilities: {
        settingsRead: true,
        settingsProposal: true,
        librarySummaryRead: true,
        settingSourcesRead: true,
        resourcePreferenceRead: true,
        resourcePreferenceProposal: true,
        mediaCapabilitiesRead: true,
        onboardingRead: true,
        metadataProposal: false,
        renameProposal: false,
        secretRead: false,
        filesystemWrite: false,
      },
    }
  }

  /**
   * Mock 不伪造模型调用或 AI 推荐；没有真实 Provider 时给出与 Rust 相同的显式空能力错误。
   * 这样 UI 可以开发错误/未配置状态，但不会把演示数据误显示为模型生成结果。
   */
  async aiSettingsRecommendationGenerate(
    _request: AiSettingsRecommendationGenerateRequest,
  ): Promise<AiSettingsRecommendationDto> {
    throw new HavenError({
      code: "AI_PROVIDER_RECOMMENDATION_UNAVAILABLE",
      userMessage: "当前演示环境未接入 AI 设置推荐",
      retryable: false,
    });
  }

  async agentSettingsContextGet(): Promise<AgentSettingsContextDto> {
    const { revision, value } = this.mockReadingState()
    return this.buildAgentContext(revision, value)
  }

  async agentSettingsProposalCreate(
    request: AgentSettingsProposalCreateRequest,
  ): Promise<AgentSettingsProposalDto> {
    const { revision, value } = this.mockReadingState()
    const context = this.buildAgentContext(revision, value)
    // 与 Rust 相同的 fail-closed 判据：context id/hash 与 base revision 必须逐字匹配
    // 最近一次读取的结果，否则零写入地拒绝。
    if (request.baseRevision !== context.revision) {
      throw new HavenError({
        code: "AGENT_SETTINGS_BASE_REVISION_MISMATCH",
        userMessage: "设置版本在生成提案前已变化，请重新读取设置",
        retryable: false,
      })
    }
    if (request.contextHash !== context.contextHash) {
      throw new HavenError({
        code: "AGENT_SETTINGS_CONTEXT_STALE",
        userMessage: "设置上下文已过期，请重新读取设置后再生成提案",
        retryable: false,
      })
    }
    if (request.contextId !== context.contextId) {
      throw new HavenError({
        code: "AGENT_SETTINGS_CONTEXT_ID_MISMATCH",
        userMessage: "设置上下文 ID 与当前设置不一致，请重新读取设置",
        retryable: false,
      })
    }

    const changes = mockReadingChanges(value, request.patch)
    if (changes.length === 0) {
      throw new HavenError({
        code: "AGENT_SETTINGS_PATCH_CONFLICT",
        userMessage: "提案没有产生任何阅读设置改动",
        retryable: false,
      })
    }

    const key = `mock-proposal-${this.agentProposalCounter}`
    this.agentProposalCounter += 1
    const now = new Date()
    const record: MockAgentProposalRecord = {
      proposalId: key,
      digest: mockDigest({
        contextHash: context.contextHash,
        baseRevision: context.revision,
        patch: request.patch,
      }),
      status: "pending",
      baseRevision: context.revision,
      changes,
      patch: request.patch,
      createdAt: now.toISOString(),
      expiresAt: new Date(now.getTime() + 24 * 60 * 60 * 1000).toISOString(),
    }
    this.agentProposals.set(key, record)
    return mockProposalDto(record)
  }

  async agentSettingsProposalGet(
    request: AgentSettingsProposalGetRequest,
  ): Promise<AgentSettingsProposalGetResultDto> {
    const record = this.requireMockProposal(request.proposalId)
    return {
      proposal: mockProposalDto(record),
      receipt: record.receipt ? mockReceiptDto(record) : null,
    }
  }

  async agentSettingsProposalReject(
    request: AgentSettingsProposalRejectRequest,
  ): Promise<AgentSettingsProposalRejectResultDto> {
    const record = this.requireMockProposal(request.proposalId)
    if (request.expectedDigest !== record.digest) {
      throw new HavenError({
        code: "SETTING_PROPOSAL_DIGEST_MISMATCH",
        userMessage: "确认的摘要与提案不一致",
        retryable: false,
      })
    }
    // 与 Rust reject 语义一致：重复拒绝本身幂等；已应用/已过期是终态，
    // 不能被重新解释为一次成功的拒绝。Mock 不能把这些状态静默吞掉。
    if (record.status === "applied" || record.status === "expired") {
      throw new HavenError({
        code: "SETTING_PROPOSAL_NOT_PENDING",
        userMessage: "提案已进入终态，不能再次拒绝",
        retryable: false,
      })
    }
    if (record.status === "pending" || record.status === "conflict") {
      record.status = "rejected"
    }
    return { proposal: mockProposalDto(record) }
  }

  async agentSettingsProposalApprove(
    request: AgentSettingsProposalApproveRequest,
  ): Promise<AgentSettingsProposalApproveResultDto> {
    const record = this.requireMockProposal(request.proposalId)
    // UI 只能回传它显示的那份 digest；Mock 侧同样不接受"自己算一个"的确认。
    if (request.expectedDigest !== record.digest) {
      throw new HavenError({
        code: "SETTING_PROPOSAL_DIGEST_MISMATCH",
        userMessage: "确认的摘要与提案不一致",
        retryable: false,
      })
    }
    if (record.status === "rejected" || record.status === "expired") {
      throw new HavenError({
        code: "SETTING_PROPOSAL_NOT_PENDING",
        userMessage: "提案已进入终态，不能再次批准",
        retryable: false,
      })
    }
    // 已应用：幂等返回原回执（与真实路径一致，不重复写入）。
    if (record.status === "applied" && record.receipt) {
      return { proposal: mockProposalDto(record), receipt: mockReceiptDto(record) }
    }

    // Rust 在事务内先重读提案，再以 now >= expiresAt 将 pending 原子收敛为
    // expired，并以稳定错误返回；此分支不写目标设置，也不产生回执。
    if ((record.status === "pending" || record.status === "conflict") && new Date() >= new Date(record.expiresAt)) {
      record.status = "expired"
      throw new HavenError({
        code: "SETTING_PROPOSAL_EXPIRED",
        userMessage: "提案已过期，未执行任何修改",
        retryable: false,
      })
    }

    const { revision, value } = this.mockReadingState()
    if (revision !== record.baseRevision) {
      record.status = "conflict"
      throw new HavenError({
        code: "REVISION_CONFLICT",
        userMessage: "设置版本已变化，提案不能应用",
        retryable: false,
      })
    }

    const nextValue = applySettingsPatch(value, mockReadingPatch(record.patch))
    const nextRevision = `set-mock-${this.settingsRevisionCounter++}`
    this.settings.set("reading", { value: nextValue, revision: nextRevision })
    record.status = "applied"
    record.receipt = {
      appliedRevision: nextRevision,
      appliedAt: new Date().toISOString(),
      changed: !settingsValuesEqual(nextValue, value),
    }
    return { proposal: mockProposalDto(record), receipt: mockReceiptDto(record) }
  }

  async agentSettingChangeReceiptGet(
    request: AgentSettingChangeReceiptGetRequest,
  ): Promise<AgentSettingChangeReceiptDto | null> {
    const record = this.requireMockProposal(request.proposalId)
    if (!record.receipt) return null
    return mockReceiptDto(record)
  }

  async agentResourcePreferenceProposalCreate(
    _request: AgentResourcePreferenceProposalCreateRequest,
  ): Promise<AgentResourcePreferenceProposalDto> {
    throw new HavenError({
      code: "HAVEN_CAPABILITY_UNAVAILABLE",
      userMessage: "当前能力未开放",
      retryable: false,
    });
  }

  async agentResourcePreferenceProposalGet(
    _request: AgentResourcePreferenceProposalGetRequest,
  ): Promise<AgentResourcePreferenceProposalGetResultDto> {
    throw new HavenError({
      code: "HAVEN_CAPABILITY_UNAVAILABLE",
      userMessage: "当前能力未开放",
      retryable: false,
    });
  }

  async agentResourcePreferenceProposalApprove(
    _request: AgentResourcePreferenceProposalApproveRequest,
  ): Promise<AgentResourcePreferenceProposalApproveResultDto> {
    throw new HavenError({
      code: "HAVEN_CAPABILITY_UNAVAILABLE",
      userMessage: "当前能力未开放",
      retryable: false,
    });
  }

  async agentTraceGet(request: AgentTraceGetRequest): Promise<AgentTraceGetResultDto> {
    return { schemaVersion: 1, sessionId: request.sessionId, events: [] };
  }

  /** authoritative 阅读设置（与 `settingsGet("reading")` 同源，不另建状态）。 */
  private mockReadingState(): { revision: string | null; value: SettingsValue } {
    const state = this.settings.get("reading")
    if (state) return { revision: state.revision, value: state.value }
    const fixture = settingsReadingDefault as SettingsSnapshot
    return { revision: fixture.revision, value: fixture.value }
  }

  private buildAgentContext(revision: string | null, value: SettingsValue): AgentSettingsContextDto {
    const reading = readingSettingsOf(value)
    const contextHash = mockDigest({ section: "reading", revision, reading })
    return {
      schemaVersion: 1,
      contextId: mockUuidFromDigest(contextHash),
      contextHash,
      subject: { section: "reading" },
      revision,
      reading: {
        section: "reading",
        fontFamily: reading.fontFamily,
        customFontFamily: reading.customFontFamily ?? null,
        fontSize: reading.fontSize,
        lineHeight: reading.lineHeight,
        contentWidth: reading.contentWidth,
        theme: reading.theme,
        customBackground: reading.customBackground ?? null,
        customText: reading.customText ?? null,
        fontWeight: reading.fontWeight,
        letterSpacing: reading.letterSpacing,
        systemAuto: reading.systemAuto,
        pagination: reading.pagination,
        redactedFields: [],
      },
      capabilities: {
        agentApiVersion: 1,
        capabilities: {
          settingsRead: true,
          settingsProposal: true,
          librarySummaryRead: true,
          settingSourcesRead: true,
          resourcePreferenceRead: true,
          resourcePreferenceProposal: true,
          mediaCapabilitiesRead: true,
          onboardingRead: true,
          metadataProposal: false,
          renameProposal: false,
          secretRead: false,
          filesystemWrite: false,
        },
      },
    }
  }

  private requireMockProposal(proposalId: string): MockAgentProposalRecord {
    const record = this.agentProposals.get(proposalId)
    if (!record) {
      throw new HavenError({
        code: "SETTING_PROPOSAL_NOT_FOUND",
        userMessage: "提案不存在",
        retryable: false,
      })
    }
    return record
  }

  // ---- A5 外部 Agent Broker（Mock/开发契约；契约 §4.5 默认关闭） ----
  //
  // 这里只复现「默认关闭 → 用户显式开启 → 显式关闭」这一个状态机。Mock 不监听、
  // 不绑定 Named Pipe / Unix socket、不建任何连接，也没有可被外部进程发现的端点：
  // enable 只是把状态置为 listening，端点值是上面那个明确标注的演示标识。
  //
  // `busy` / `unavailable` 是 Rust 对「端点被另一实例占用」与「平台不支持或端点无法
  // 解析」的 fail-closed 结论。浏览器里根本没有端点可解析，因此 Mock 不伪造这两种
  // 状态——页面在这两个分支上的行为只能由 Rust 侧与单元测试覆盖。

  async agentBrokerStatus(): Promise<AgentBrokerStatusResultDto> {
    return this.agentBrokerProjection()
  }

  /** 幂等：已开启时重复 enable 返回同一端点，与 Rust 的 enable 语义一致。 */
  async agentBrokerEnable(): Promise<AgentBrokerStatusResultDto> {
    this.agentBrokerListening = true
    return this.agentBrokerProjection()
  }

  /** 幂等：未开启时 disable 也返回 disabled（Rust 侧 disable 不报错）。 */
  async agentBrokerDisable(): Promise<AgentBrokerStatusResultDto> {
    this.agentBrokerListening = false
    return this.agentBrokerProjection()
  }

  /**
   * 浏览器预览的内置技能目录。
   *
   * **刻意不使用真实技能 id**：真实目录是编译进二进制的构建产物，浏览器里没有它。
   * 用一个看起来像真的条目会让预览变成"技能已在栖阅里生效"的假证据——这正是
   * AI_SYSTEM.md §6 禁止的读法。预览只证明界面与状态机接线正确。
   */
  private readonly mockAgentSkills = new Map<string, AgentSkillActivationWire>()

  async agentSkillList(): Promise<AgentSkillListResultWire> {
    if (this.mockAgentSkills.size === 0) {
      this.mockAgentSkills.set(MOCK_PREVIEW_SKILL_ID, "disabled")
    }
    return {
      schemaVersion: 1,
      skills: [...this.mockAgentSkills.entries()]
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([skillId, state]) => ({
          schemaVersion: 1 as const,
          skillId,
          description: MOCK_PREVIEW_SKILL_DESCRIPTION,
          instructionsChars: MOCK_PREVIEW_SKILL_CHARS,
          state,
        })),
    }
  }

  async agentSkillSetEnabled(
    request: AgentSkillSetEnabledRequestWire,
  ): Promise<AgentSkillStateWire> {
    if (request.skillId !== MOCK_PREVIEW_SKILL_ID) {
      throw new HavenError({
        code: "AGENT_SKILL_UNKNOWN",
        userMessage: "预览环境只有一项示例技能",
        retryable: false,
      })
    }
    this.mockAgentSkills.set(request.skillId, request.enabled ? "enabled" : "disabled")
    return {
      schemaVersion: 1,
      skillId: request.skillId,
      description: MOCK_PREVIEW_SKILL_DESCRIPTION,
      instructionsChars: MOCK_PREVIEW_SKILL_CHARS,
      state: request.enabled ? "enabled" : "disabled",
    }
  }

  /**
   * 浏览器预览的客户端自动配置状态。
   *
   * **刻意返回"被拦下"而不是一个好看的演示状态**：预览里既没有安装包资源目录，也没有
   * 用户真实的 `~/.codex/config.toml` / `~/.claude.json`。把这里做成"已配置"会变成
   * "栖阅已经替你写好了客户端配置"的假证据——而那正是这条能力最不能出错的地方。
   */
  async mcpClientConfigStatus(): Promise<McpClientConfigStatusWire> {
    const target = (
      id: McpClientTargetWire,
      label: string,
    ): McpClientTargetStatusWire => ({
      target: id,
      label,
      // 浏览器里无从得知用户的配置路径，因此不编一个出来。
      configPath: null,
      state: "blocked",
      writable: false,
      detail: "浏览器预览没有安装包资源目录，也不会去碰你机器上的客户端配置文件。",
    })
    return {
      schemaVersion: 1,
      runtimeReady: false,
      runtimeDetail: "浏览器预览里没有随包分发的 MCP 运行时。",
      targets: [target("codex", "Codex"), target("claude_code", "Claude Code")],
    }
  }

  /** 预览环境**不写任何客户端配置**：连尝试都不做，直接如实失败。 */
  async mcpClientConfigApply(
    request: McpClientConfigureRequestWire,
  ): Promise<McpClientConfigStatusWire> {
    throw new HavenError({
      code: "MCP_CLIENT_CONFIG_BLOCKED",
      userMessage: `浏览器预览不会写入 ${request.target} 的配置文件；请在桌面版里操作。`,
      retryable: false,
    })
  }

  // ---- Film/TV Provider 基础切片：TVBox / FongMi 配置预览 ----
  //
  // 浏览器预览里没有 Rust 侧的受控 HTTP、URL 策略、逐跳 DNS 固定与响应大小上限，
  // 因此这里**不假装取回过任何配置**：只做与后端同形的输入校验（空 / 超长），然后
  // 如实失败。返回一份编造的解析摘要会让 UI 开发者以为预览链路已经接通了。
  async tvboxConfigPreview(request: TvboxConfigPreviewRequest): Promise<TvboxConfigPreviewDto> {
    const url = request.url.trim()
    if (!url) {
      throw new HavenError({
        code: "INVALID_ARGUMENT",
        userMessage: "TVBox 配置地址不能为空",
        retryable: false,
      })
    }
    if (url.length > MAX_TVBOX_CONFIG_URL_CHARS) {
      throw new HavenError({
        code: "INVALID_ARGUMENT",
        userMessage: "TVBox 配置地址过长",
        retryable: false,
      })
    }
    throw new HavenError({
      code: "HAVEN_CAPABILITY_UNAVAILABLE",
      userMessage: "演示环境不获取远端配置；请在桌面应用里预览。",
      retryable: false,
    })
  }

  // ---- Film/TV Provider 基础切片：TVBox / FongMi 配置保存 ----
  //
  // 演示环境同样不假装导入成功：登记来源需要 Rust 侧的受控 HTTP、解析器与 SQLite
  // 缓存，这里都没有。返回一个编造的 sourceId 会让 UI 开发者以为导入链路已经接通，
  // 之后按那个 ID 调用只会得到"来源不存在"。
  async tvboxConfigSave(request: TvboxConfigSaveRequest): Promise<TvboxConfigSaveResult> {
    const displayName = request.displayName.trim()
    if (!displayName) {
      throw new HavenError({
        code: "INVALID_ARGUMENT",
        userMessage: "显示名不能为空",
        retryable: false,
      })
    }
    if (displayName.length > MAX_TVBOX_DISPLAY_NAME_CHARS) {
      throw new HavenError({
        code: "INVALID_ARGUMENT",
        userMessage: "显示名不能超过 100 字符",
        retryable: false,
      })
    }
    const url = request.url.trim()
    if (!url) {
      throw new HavenError({
        code: "INVALID_ARGUMENT",
        userMessage: "TVBox 配置地址不能为空",
        retryable: false,
      })
    }
    if (url.length > MAX_TVBOX_CONFIG_URL_CHARS) {
      throw new HavenError({
        code: "INVALID_ARGUMENT",
        userMessage: "TVBox 配置地址过长",
        retryable: false,
      })
    }
    throw new HavenError({
      code: "HAVEN_CAPABILITY_UNAVAILABLE",
      userMessage: "演示环境不导入远端配置；请在桌面应用里导入。",
      retryable: false,
    })
  }

  // ---- 云盘（Google Drive 只读切片；Mock/开发契约） ----
  //
  // 浏览器预览里没有系统浏览器授权回调、没有 Rust 侧的 Drive 适配器、租约注册表与
  // 凭据存储，因此这里**不伪造任何"已连接"状态**：列表如实回答 `oauthAvailable: false`
  // 与空账户表，其余动作一律显式失败。返回一个编造的账户或浏览页会让 UI 开发者以为
  // 授权链路已经接通，而用户点下去只会得到"账户不存在"。
  async cloudStorageList(): Promise<CloudStorageListWire> {
    return { oauthAvailable: false, accounts: [] }
  }

  async cloudAccountConnectBegin(
    _request: CloudConnectBeginRequestWire,
  ): Promise<CloudConnectAttemptWire> {
    throw cloudUnsupported("发起云盘授权")
  }

  async cloudAccountConnectPoll(_attemptId: string): Promise<CloudConnectPollWire> {
    throw cloudUnsupported("轮询云盘授权")
  }

  async cloudAccountConnectComplete(_attemptId: string): Promise<CloudAccountWire> {
    throw cloudUnsupported("完成云盘授权")
  }

  async cloudAccountConnectCancel(_attemptId: string): Promise<CloudConnectStatusWire> {
    throw cloudUnsupported("取消云盘授权")
  }

  async cloudAccountDisconnect(
    _request: CloudAccountDisconnectRequestWire,
  ): Promise<CloudAccountWire> {
    throw cloudUnsupported("断开云盘账户")
  }

  async cloudFolderBindingGet(_locationId: string): Promise<CloudFolderWire | null> {
    throw cloudUnsupported("读取云盘目录绑定")
  }

  async cloudFolderRemove(_locationId: string): Promise<boolean> {
    throw cloudUnsupported("移除云盘目录")
  }

  async cloudBrowseRoot(_accountId: string): Promise<CloudBrowsePageWire> {
    throw cloudUnsupported("浏览云盘")
  }

  async cloudBrowseFolder(_folderHandle: string): Promise<CloudBrowsePageWire> {
    throw cloudUnsupported("浏览云盘目录")
  }

  async cloudBrowseNextPage(_cursor: string): Promise<CloudBrowsePageWire> {
    throw cloudUnsupported("翻页浏览云盘目录")
  }

  async cloudBrowseLocation(_locationId: string): Promise<CloudBrowsePageWire> {
    throw cloudUnsupported("浏览已登记云盘目录")
  }

  async cloudRegisterFolder(_request: CloudRegisterFolderRequestWire): Promise<string> {
    throw cloudUnsupported("登记云盘目录")
  }

  async cloudImportPdf(_request: CloudImportPdfRequestWire): Promise<CloudObjectWire> {
    throw cloudUnsupported("从云盘导入 PDF")
  }

  private agentBrokerProjection(): AgentBrokerStatusResultDto {
    // endpoint 不是秘密（契约 §4.2）：它只在 listening 时出现，且从不进错误文案。
    return this.agentBrokerListening
      ? {
          schemaVersion: 1,
          status: "listening",
          endpoint: MOCK_AGENT_BROKER_ENDPOINT,
          reason: null,
        }
      : { schemaVersion: 1, status: "disabled", endpoint: null, reason: null }
  }

  private nextRuntimeIdentity(): string {
    const suffix = (this.runtimeIdentityCounter++).toString(16).padStart(12, "0");
    return `0196f0d2-0000-7000-8000-${suffix}`;
  }
}
