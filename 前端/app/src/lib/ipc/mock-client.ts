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
  PreferenceReadingSettingsDto,
} from "./generated/wire";
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
} from "./settings-wire";
import type { EditionListByWorkRequest, EditionListByWorkResultDto } from "../../features/media/ipc/edition-wire";
import type { EditionDetailDto, EditionGetRequest } from "./generated/wire";
import {
  applySettingsPatch,
  defaultSettingsValue,
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
import settingsConflictError from "../../../../../contracts/ipc/v1/fixtures/settings/update.error-revision-conflict.json" with { type: "json" };
import settingsInvalidArgument from "../../../../../contracts/ipc/v1/fixtures/settings/update.error-invalid-argument.json" with { type: "json" };
import sourceRegistryNormal from "../../../../../contracts/ipc/v1/fixtures/source/registry.normal.json" with { type: "json" };
import sourceSetErrorUnknown from "../../../../../contracts/ipc/v1/fixtures/source/set.error-unknown-source.json" with { type: "json" };
import mediaStateNormal from "../../../../../contracts/ipc/v1/fixtures/media-state/state.normal.json" with { type: "json" };
import appInfoMock from "../../../../../contracts/ipc/v1/fixtures/app-info/mock.json" with { type: "json" };
import { HavenError } from "./errors.js";

const RESOURCE_FIXTURE_MEDIA_ITEM_ID = "0196f0d2-0000-7000-8000-000000000000";
const MEDIA_ITEM_ID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

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
}

/** 共享 Fixture 驱动的 Mock Client（契约冻结前置的最小消费实现）。 */
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
  private searchOperationCounter = 1;
  private readonly sourceEnabled = new Map<string, boolean>();
  private readonly sourceEndpoints = new Map<string, string>();
  private readonly credentialProfiles = new Set<string>();
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

  constructor(emptyLibrary = false, options: MockHavenClientOptions = {}) {
    this.emptyLibrary = emptyLibrary;
    this.favorites = new Map();
    this.settings = new Map();
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
      const descriptor: SourceDescriptorDto = {
        sourceId,
        displayName: source.displayName,
        kinds: ["search", "offline_download"],
        categories: ["book"],
        mode: "single",
        notes: "这是你添加的自定义 OPDS 书库；可在来源设置中编辑地址或配置访问凭据。",
        enabled: this.customSourceEnabled.get(sourceId) ?? false,
        health: "unknown",
        endpointConfigured: source.endpoint.length > 0,
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

  // ---- V2-H 收尾批次：自定义 OPDS 书源（Mock：内存态，不落 localStorage）----

  private customSources = new Map<string, { displayName: string; endpoint: string }>();
  private customSourceEnabled = new Map<string, boolean>();
  private customCredentialConfigured = new Set<string>();

  async sourceAdd(
    request: import("./generated/wire").SourceAddRequest,
  ): Promise<import("./generated/wire").SourceAddResult> {
    const name = request.displayName.trim();
    const endpoint = request.endpoint.trim();
    if (!name || name.length > 100) {
      throw new HavenError({ code: "INVALID_ARGUMENT", userMessage: "显示名非法", retryable: false });
    }
    if (!endpoint.startsWith("http://") && !endpoint.startsWith("https://")) {
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
    const sourceId = `custom-${Date.now().toString(16)}${this.customSources.size}`;
    this.customSources.set(sourceId, { displayName: name, endpoint });
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
      if (!nextEndpoint.startsWith("http://") && !nextEndpoint.startsWith("https://")) {
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
    if (!this.customSources.has(request.sourceId)) {
      throw new HavenError({ code: "RESOURCE_NOT_FOUND", userMessage: "自定义来源不存在", retryable: false });
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

  private nextRuntimeIdentity(): string {
    const suffix = (this.runtimeIdentityCounter++).toString(16).padStart(12, "0");
    return `0196f0d2-0000-7000-8000-${suffix}`;
  }
}
