// TauriHavenClient（IPC-FE-001：HavenClient 的真实 Tauri invoke 实现）。
// 契约：plan/FRONTEND_BACKEND_CONTRACT.md §14.5 冻结矩阵；命令参数统一以
// { request } 传递（与 src-tauri 命令签名及 ipc_e2e_test 调用体一致，字段 camelCase）。
// 错误：命令返回 Result<_, ErrorDto>，reject 的即契约 ErrorDto 形状，
// 经 toHavenError 归一（非契约形状兜底 INTERNAL_ERROR，见 errors.ts）。

import { Channel, invoke } from "@tauri-apps/api/core";
import { TauriUpdaterClient } from "./updater-client";

import type { HavenClient } from "./client";
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
  EditionListByWorkRequest,
  EditionSummaryDto,
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
  HomeDto,
  StorageLocationDto,
  DownloadCreateRequest,
  DownloadEvent,
  DownloadListRequest,
  DownloadMutationResultDto,
  DownloadRevealResultDto,
  DownloadTaskActionRequest,
  DownloadTaskDto,
  EditionDetailDto,
  EditionGetRequest,
  ComicChapterCatalogGetRequest,
  ComicChapterCatalogDto,
  ComicRegisteredChapterCatalogDto,
  ComicWorkChapterCatalogRequestDto,
  ComicWorkChapterCatalogDto,
  ComicChapterSourceCandidatesDto,
  ComicChapterSourceCandidatesGetRequestDto,
  ComicProgressMigrationRequestDto,
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
  SourceRegistryDto,
  SourceRegistrySetRequest,
  SourceRegistrySetResult,
  SourceEndpointSetRequest,
  SourceEndpointSetResult,
  SearchSourceStartRequest,
  SearchStartResultDto,
  SearchSourceEvent,
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
  PeriodicalTreeGetRequest,
  PeriodicalTreeDto,
  AgentBrokerStatusResultDto,
  AgentCapabilityManifestDto,
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
import { guardAgentSkillListResult, guardAgentSkillState } from "./agent-skill-wire";
import { guardAiSettingsRecommendation } from "./ai-recommendation-wire";
import type {
  AgentSkillListResultWire,
  AgentSkillSetEnabledRequestWire,
  AgentSkillStateWire,
} from "./agent-skill-wire";
import { guardMcpClientConfigStatus } from "./mcp-client-wire";
import type {
  McpClientConfigStatusWire,
  McpClientConfigureRequestWire,
} from "./mcp-client-wire";
import type { UpdaterCheckResult, UpdaterInstallResult, UpdaterProgress } from "./client";
import { toHavenError } from "./errors.js";
import type {
  SettingsSectionWire,
  SettingsSnapshot,
  SettingsUpdateRequest,
  SettingsUpdateResult,
  PreferenceGetRequest,
  PreferenceGetResult,
  PreferenceUpdateRequest,
  PreferenceUpdateResult,
  AppearanceAssetsListRequestWire,
  AppearanceAssetImportRequestWire,
  AppearanceAssetDeleteRequestWire,
  AppearanceAssetWire,
  AppearanceAssetsWire,
  AppearanceAssetDeleteResultWire,
  HomeLayoutSnapshotWire,
  HomeLayoutSaveRequestWire,
  HomeLayoutResetRequestWire,
  HomeLayoutMutationResultWire,
  OverviewLayoutSnapshotWire,
  OverviewLayoutSaveRequestWire,
  OverviewLayoutResetRequestWire,
  OverviewLayoutMutationResultWire,
  ReadingOverviewGetRequestWire,
  ReadingOverviewWire,
} from "./settings-wire";
import {
  guardPreferenceGetResult,
  guardPreferenceUpdateResult,
  guardSettingsSnapshot,
  guardSettingsUpdateResult,
  guardAppearanceAsset,
  guardAppearanceAssetDeleteResult,
  guardAppearanceAssets,
  guardHomeLayoutMutationResult,
  guardHomeLayoutSnapshot,
  guardOverviewLayoutMutationResult,
  guardOverviewLayoutSnapshot,
  guardReadingOverview,
} from "./settings-wire.js";
import type {
  InterfaceFontAsset,
  InterfaceFontFamily,
  InterfaceFontImportResult,
} from "./interface-font-wire";
import {
  guardInterfaceFontAssetList,
  guardInterfaceFontFamilyList,
  guardInterfaceFontImportResult,
} from "./interface-font-wire.js";
import { createCloudStorageIpc } from "./cloud-storage-client.js";

/** 真实 IPC Client（仅 Tauri WebView 内可用；浏览器环境由 runtime.ts 拦截回落 Mock）。 */
export class TauriHavenClient implements HavenClient {
  private readonly updater = new TauriUpdaterClient();

  // ---- 云盘（Google Drive 只读切片）----
  //
  // invoke、响应守卫与错误归一都在 lib/ipc/cloud-storage-client.ts 的共享实现里，
  // 这里只是把它的方法挂到 HavenClient 上：每条命令一行委托，不复制任何命令逻辑。
  private readonly cloudStorageIpc = createCloudStorageIpc(invoke);

  readonly cloudStorageList = this.cloudStorageIpc.cloudStorageList;
  readonly cloudAccountConnectBegin = this.cloudStorageIpc.cloudAccountConnectBegin;
  readonly cloudAccountConnectPoll = this.cloudStorageIpc.cloudAccountConnectPoll;
  readonly cloudAccountConnectComplete = this.cloudStorageIpc.cloudAccountConnectComplete;
  readonly cloudAccountConnectCancel = this.cloudStorageIpc.cloudAccountConnectCancel;
  readonly cloudAccountDisconnect = this.cloudStorageIpc.cloudAccountDisconnect;
  readonly cloudFolderBindingGet = this.cloudStorageIpc.cloudFolderBindingGet;
  readonly cloudFolderRemove = this.cloudStorageIpc.cloudFolderRemove;
  readonly cloudBrowseRoot = this.cloudStorageIpc.cloudBrowseRoot;
  readonly cloudBrowseFolder = this.cloudStorageIpc.cloudBrowseFolder;
  readonly cloudBrowseNextPage = this.cloudStorageIpc.cloudBrowseNextPage;
  readonly cloudBrowseLocation = this.cloudStorageIpc.cloudBrowseLocation;
  readonly cloudRegisterFolder = this.cloudStorageIpc.cloudRegisterFolder;
  readonly cloudImportPdf = this.cloudStorageIpc.cloudImportPdf;

  async libraryList(request: LibraryListRequest): Promise<PageDto<WorkCardDto>> {
    try {
      return await invoke<PageDto<WorkCardDto>>("library_list", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async favoriteSet(request: FavoriteSetRequest): Promise<FavoriteSetResult> {
    try {
      return await invoke<FavoriteSetResult>("favorite_set", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async libraryScanStart(
    request: LibraryScanStartRequest,
    onEvent?: (event: LibraryScanEvent) => void,
  ): Promise<ScanStartResult> {
    // library.scan 专属 Channel（契约 §14.4：高频进度走 Channel，不走事件广播）。
    // 后端命令已注册（BE-SCAN-001 第三步）；onEvent 回调经 Channel 逐事件推送。
    const channel = new Channel<LibraryScanEvent>();
    if (onEvent) channel.onmessage = onEvent;
    try {
      return await invoke<ScanStartResult>("library_scan_start", {
        request,
        onEvent: channel,
      });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async workGet(request: WorkGetRequest): Promise<WorkDetailHeaderDto> {
    try {
      return await invoke<WorkDetailHeaderDto>("work_get", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async editionListByWork(request: EditionListByWorkRequest): Promise<PageDto<EditionSummaryDto>> {
    try {
      return await invoke<PageDto<EditionSummaryDto>>("edition_list_by_work", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async editionGet(request: EditionGetRequest): Promise<EditionDetailDto> {
    try {
      return await invoke<EditionDetailDto>("edition_get", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async resourceListByMediaItem(request: ResourceListByMediaItemRequest): Promise<ResourceListDto> {
    try {
      return await invoke<ResourceListDto>("resource_list_by_media_item", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async sessionOpen(request: SessionOpenRequest): Promise<SessionOpenResultDto> {
    try {
      return await invoke<SessionOpenResultDto>("session_open", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async streamOpen(request: SessionOpenRequest): Promise<SessionOpenResultDto> {
    try {
      // V2-B：远端流受控代理会话；contentUri 为 haven-resource://stream/<grant>。
      return await invoke<SessionOpenResultDto>("stream_open", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async streamClose(request: SessionCloseRequest): Promise<boolean> {
    try {
      return await invoke<boolean>("stream_close", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async comicChapterCatalogGet(
    request: ComicChapterCatalogGetRequest,
  ): Promise<ComicChapterCatalogDto> {
    try {
      return await invoke<ComicChapterCatalogDto>("comic_chapter_catalog_get", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async comicChapterCatalogRegisteredGet(
    request: ComicChapterCatalogGetRequest,
  ): Promise<ComicRegisteredChapterCatalogDto> {
    try {
      return await invoke<ComicRegisteredChapterCatalogDto>(
        "comic_chapter_catalog_registered_get",
        { request },
      );
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async comicChapterCatalogRefresh(
    request: ComicChapterCatalogGetRequest,
  ): Promise<ComicChapterCatalogDto> {
    try {
      return await invoke<ComicChapterCatalogDto>("comic_chapter_catalog_refresh", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async comicWorkChapterCatalogGet(
    request: ComicWorkChapterCatalogRequestDto,
  ): Promise<ComicWorkChapterCatalogDto> {
    try {
      return await invoke<ComicWorkChapterCatalogDto>("comic_work_chapter_catalog_get", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async comicWorkChapterCatalogRefresh(
    request: ComicWorkChapterCatalogRequestDto,
  ): Promise<ComicWorkChapterCatalogDto> {
    try {
      return await invoke<ComicWorkChapterCatalogDto>("comic_work_chapter_catalog_refresh", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async comicChapterSourceCandidatesGet(
    request: ComicChapterSourceCandidatesGetRequestDto,
  ): Promise<ComicChapterSourceCandidatesDto> {
    try {
      return await invoke<ComicChapterSourceCandidatesDto>(
        "comic_chapter_source_candidates_get",
        { request },
      );
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async comicProgressMigrate(
    request: ComicProgressMigrationRequestDto,
  ): Promise<ComicProgressMigrationResultDto> {
    try {
      return await invoke<ComicProgressMigrationResultDto>("comic_progress_migrate", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async comicProgressRemap(
    request: ComicPageProgressRemapRequestDto,
  ): Promise<ComicProgressMigrationResultDto> {
    try {
      return await invoke<ComicProgressMigrationResultDto>("comic_progress_remap", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async comicProgressRevert(
    request: ComicProgressMigrationRevertRequestDto,
  ): Promise<ComicProgressMigrationRevertResultDto> {
    try {
      return await invoke<ComicProgressMigrationRevertResultDto>("comic_progress_revert", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async comicPageManifestGet(
    request: ComicPageManifestGetRequest,
  ): Promise<ComicPageManifestDto> {
    try {
      return await invoke<ComicPageManifestDto>("comic_page_manifest_get", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async periodicalTreeGet(request: PeriodicalTreeGetRequest): Promise<PeriodicalTreeDto> {
    try {
      return await invoke<PeriodicalTreeDto>("periodical_tree_get", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async readerTocGet(request: ReaderTocGetRequest): Promise<ReaderTocResultDto> {
    try {
      return await invoke<ReaderTocResultDto>("reader_toc_get", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async readerSearch(request: ReaderSearchRequest): Promise<ReaderSearchResultDto> {
    try {
      return await invoke<ReaderSearchResultDto>("reader_search", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async readerSearchStart(
    request: ReaderSearchRequest,
    onEvent?: (event: ReaderSearchEvent) => void,
  ): Promise<ReaderSearchResultDto> {
    const channel = new Channel<ReaderSearchEvent>()
    if (onEvent) channel.onmessage = onEvent
    try {
      return await invoke<ReaderSearchResultDto>("reader_search_start", {
        request,
        onEvent: channel,
      })
    } catch (error) {
      throw toHavenError(error)
    }
  }

  async readerSearchCancel(
    request: ReaderSearchCancelRequest,
  ): Promise<ReaderSearchCancelResultDto> {
    try {
      return await invoke<ReaderSearchCancelResultDto>("reader_search_cancel", { request })
    } catch (error) {
      throw toHavenError(error)
    }
  }

  async sessionClose(request: SessionCloseRequest): Promise<SessionCloseResultDto> {
    try {
      return await invoke<SessionCloseResultDto>("session_close", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async progressSave(request: ProgressSaveRequest): Promise<ProgressSaveResult> {
    try {
      return await invoke<ProgressSaveResult>("progress_save", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async progressMarkCompleted(request: ProgressMarkCompletedRequest): Promise<ProgressSaveResult> {
    try {
      return await invoke<ProgressSaveResult>("progress_mark_completed", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async progressRecent(request: ProgressRecentRequest): Promise<ProgressSummaryDto[]> {
    try {
      return await invoke<ProgressSummaryDto[]>("progress_recent", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async progressReset(request: ProgressResetRequest): Promise<void> {
    try {
      await invoke<void>("progress_reset", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async historyList(request: HistoryListRequest): Promise<HistoryEntryDto[]> {
    try {
      return await invoke<HistoryEntryDto[]>("history_list", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async historyClear(): Promise<void> {
    try {
      await invoke<void>("history_clear");
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async searchHistoryList(request: SearchHistoryListRequest): Promise<SearchHistoryEntryDto[]> {
    try {
      return await invoke<SearchHistoryEntryDto[]>("search_history_list", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async searchHistoryRecord(request: SearchHistoryRecordRequest): Promise<SearchHistoryEntryDto> {
    try {
      return await invoke<SearchHistoryEntryDto>("search_history_record", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async searchHistoryRemove(request: SearchHistoryRemoveRequest): Promise<boolean> {
    try {
      return await invoke<boolean>("search_history_remove", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async searchHistoryClear(): Promise<void> {
    try {
      await invoke<void>("search_history_clear");
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async markerCreate(request: MarkerCreateRequest): Promise<MarkerDto> {
    try {
      return await invoke<MarkerDto>("marker_create", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async markerList(request: MarkerListRequest): Promise<MarkerDto[]> {
    try {
      return await invoke<MarkerDto[]>("marker_list", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async markerListAll(request: MarkerListAllRequest): Promise<MarkerDto[]> {
    try {
      return await invoke<MarkerDto[]>("marker_list_all", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async markerDelete(request: MarkerDeleteRequest): Promise<boolean> {
    try {
      return await invoke<boolean>("marker_delete", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async homeGet(): Promise<HomeDto> {
    try {
      return await invoke<HomeDto>("home_get");
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async settingsGet(section: SettingsSectionWire): Promise<SettingsSnapshot> {
    try {
      const value: unknown = await invoke("settings_get", { section });
      if (!guardSettingsSnapshot(value)) throw new Error("settings_get returned invalid data");
      return value;
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async settingsUpdate(request: SettingsUpdateRequest): Promise<SettingsUpdateResult> {
    try {
      const value: unknown = await invoke("settings_update", {
        section: request.section,
        expectedRevision: request.expectedRevision,
        patch: request.patch,
      });
      if (!guardSettingsUpdateResult(value)) throw new Error("settings_update returned invalid data");
      return value;
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async interfaceFontSystemList(): Promise<InterfaceFontFamily[]> {
    try {
      const value: unknown = await invoke("interface_font_system_list");
      if (!guardInterfaceFontFamilyList(value)) {
        throw new Error("interface_font_system_list returned invalid data");
      }
      return value;
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async interfaceFontAssetList(): Promise<InterfaceFontAsset[]> {
    try {
      const value: unknown = await invoke("interface_font_asset_list");
      if (!guardInterfaceFontAssetList(value)) {
        throw new Error("interface_font_asset_list returned invalid data");
      }
      return value;
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async interfaceFontAssetImport(): Promise<InterfaceFontImportResult> {
    try {
      const value: unknown = await invoke("interface_font_asset_import");
      if (!guardInterfaceFontImportResult(value)) {
        throw new Error("interface_font_asset_import returned invalid data");
      }
      return value;
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async interfaceFontAssetDelete(assetId: string): Promise<void> {
    try {
      await invoke("interface_font_asset_delete", { assetId });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async preferenceGet(request: PreferenceGetRequest): Promise<PreferenceGetResult> {
    try {
      const value: unknown = await invoke("preference_get", { request });
      if (!guardPreferenceGetResult(value)) throw new Error("preference_get returned invalid data");
      return value;
    } catch (error) {
      throw toHavenError(error);
    }
  }
  async preferenceUpdate(request: PreferenceUpdateRequest): Promise<PreferenceUpdateResult> {
    try {
      const value: unknown = await invoke("preference_update", { request });
      if (!guardPreferenceUpdateResult(value)) throw new Error("preference_update returned invalid data");
      return value;
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async storageLocationList(): Promise<StorageLocationDto[]> {
    try {
      return await invoke<StorageLocationDto[]>("storage_location_list");
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async downloadCreate(request: DownloadCreateRequest): Promise<DownloadTaskDto> {
    return this.invokeDownload("download_create", request);
  }

  async downloadList(request: DownloadListRequest): Promise<DownloadTaskDto[]> {
    try {
      return await invoke<DownloadTaskDto[]>("download_list", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async downloadPause(request: DownloadTaskActionRequest): Promise<DownloadTaskDto> {
    return this.invokeDownload("download_pause", request);
  }

  async downloadResume(request: DownloadTaskActionRequest): Promise<DownloadTaskDto> {
    return this.invokeDownload("download_resume", request);
  }

  async downloadCancel(request: DownloadTaskActionRequest): Promise<DownloadTaskDto> {
    return this.invokeDownload("download_cancel", request);
  }

  async downloadRetry(request: DownloadTaskActionRequest): Promise<DownloadTaskDto> {
    return this.invokeDownload("download_retry", request);
  }

  async downloadRemoveRecord(request: DownloadTaskActionRequest): Promise<DownloadMutationResultDto> {
    return this.invokeDownloadManagement<DownloadMutationResultDto>("download_remove_record", request);
  }

  async downloadDeleteOffline(request: DownloadTaskActionRequest): Promise<DownloadMutationResultDto> {
    return this.invokeDownloadManagement<DownloadMutationResultDto>("download_delete_offline", request);
  }

  async downloadRevealOffline(request: DownloadTaskActionRequest): Promise<DownloadRevealResultDto> {
    return this.invokeDownloadManagement<DownloadRevealResultDto>("download_reveal_offline", request);
  }

  async downloadSubscribe(
    subscriptionId: string,
    onEvent: (event: DownloadEvent) => void,
  ): Promise<() => Promise<void>> {
    const channel = new Channel<DownloadEvent>();
    channel.onmessage = onEvent;
    try {
      await invoke<void>("download_subscribe", { subscriptionId, onEvent: channel });
    } catch (error) {
      channel.onmessage = () => undefined;
      throw toHavenError(error);
    }
    let active = true;
    return async () => {
      if (!active) return;
      active = false;
      channel.onmessage = () => undefined;
      try {
        await invoke<void>("download_unsubscribe", { subscriptionId });
      } catch (error) {
        throw toHavenError(error);
      }
    };
  }

  private async invokeDownload(
    command: string,
    request: DownloadCreateRequest | DownloadTaskActionRequest,
  ): Promise<DownloadTaskDto> {
    try {
      return await invoke<DownloadTaskDto>(command, { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  private async invokeDownloadManagement<T>(
    command: string,
    request: DownloadTaskActionRequest,
  ): Promise<T> {
    try {
      return await invoke<T>(command, { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  // ---- v0.2 契约冻结（契约 §36；CONTRACT-V02-*）----

  async sourceRegistryList(): Promise<SourceRegistryDto> {
    try {
      return await invoke<SourceRegistryDto>("source_registry_list");
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async sourceRegistrySet(request: SourceRegistrySetRequest): Promise<SourceRegistrySetResult> {
    try {
      return await invoke<SourceRegistrySetResult>("source_registry_set", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async sourceRegistrySetEndpoint(
    request: SourceEndpointSetRequest,
  ): Promise<SourceEndpointSetResult> {
    try {
      // 响应只含布尔投影；端点本身不出 IPC（契约 §36.2）。
      return await invoke<SourceEndpointSetResult>("source_registry_set_endpoint", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async sourceWorkImport(request: SourceWorkImportRequest): Promise<SourceWorkImportResult> {
    try {
      return await invoke<SourceWorkImportResult>("source_work_import", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  // ---- V2-H 收尾批次：自定义 OPDS 书源 ----

  async sourceAdd(
    request: import("./generated/wire").SourceAddRequest,
  ): Promise<import("./generated/wire").SourceAddResult> {
    try {
      return await invoke("source_add", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async sourceUpdate(
    request: import("./generated/wire").SourceUpdateRequest,
  ): Promise<import("./generated/wire").SourceUpdateResult> {
    try {
      return await invoke("source_update", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async sourceRemove(
    request: import("./generated/wire").SourceRemoveRequest,
  ): Promise<import("./generated/wire").SourceRemoveResult> {
    try {
      return await invoke("source_remove", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async sourceSetCredential(request: import("./generated/wire").SourceSetCredentialRequest): Promise<void> {
    // secret 单向写入系统 keyring（契约 §36.5 演进）；响应不含 secret。
    try {
      await invoke<void>("source_set_credential", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async searchSourceStart(
    request: SearchSourceStartRequest,
    onEvent?: (event: SearchSourceEvent) => void,
  ): Promise<SearchStartResultDto> {
    // search.source 专属 Channel（契约 §36.3）；后端命令已随 V2-A 冻结批次注册。
    const channel = new Channel<SearchSourceEvent>();
    if (onEvent) channel.onmessage = onEvent;
    try {
      return await invoke<SearchStartResultDto>("search_source_start", {
        request,
        onEvent: channel,
      });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async searchSourceCancel(request: SearchSourceCancelRequest): Promise<SearchSourceCancelResultDto> {
    try {
      return await invoke<SearchSourceCancelResultDto>("search_source_cancel", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async credentialStatus(request: CredentialStatusRequest): Promise<CredentialStatusDto> {
    try {
      return await invoke<CredentialStatusDto>("credential_status", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async credentialSet(request: CredentialSetRequest): Promise<void> {
    // Secret 单向写入 Windows Credential Store（契约 §36.5）；响应不含 secret。
    try {
      await invoke<void>("credential_set", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async credentialDelete(request: CredentialDeleteRequest): Promise<void> {
    try {
      await invoke<void>("credential_delete", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async mediaStateGet(request: MediaStateGetRequest): Promise<MediaStateDto> {
    // 契约 §36.10：media_state_get 属 V2-G 批次接线前 fail closed。
    try {
      return await invoke<MediaStateDto>("media_state_get", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async enrichmentStatus(request: EnrichmentStatusRequest): Promise<EnrichmentStateDto[]> {
    // 契约 §36.8：V2-F 批次已接线（enrichment_status 真实 IPC）。
    try {
      return await invoke<EnrichmentStateDto[]>("enrichment_status", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async trendingBoardsGet(): Promise<import("./generated/wire").TrendingBoardsDto> {
    try {
      return await invoke<import("./generated/wire").TrendingBoardsDto>("trending_boards_get");
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async trendingBoardsRefresh(): Promise<import("./generated/wire").TrendingBoardsDto> {
    try {
      return await invoke<import("./generated/wire").TrendingBoardsDto>("trending_boards_refresh");
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async appInfoGet(): Promise<AppInfoDto> {
    try {
      return await invoke<AppInfoDto>("app_info_get");
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async errorReportPreviewGet(request: ErrorReportPreviewRequest): Promise<ErrorReportPreviewDto> {
    try {
      return await invoke<ErrorReportPreviewDto>("error_report_preview_get", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async errorReportConfirm(request: ErrorReportConfirmRequest): Promise<ErrorReportConfirmResultDto> {
    try {
      return await invoke<ErrorReportConfirmResultDto>("error_report_confirm", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async errorReportExport(request: ErrorReportActionRequest): Promise<ErrorReportActionResultDto> {
    try {
      return await invoke<ErrorReportActionResultDto>("error_report_export", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async errorReportOpenIssue(request: ErrorReportActionRequest): Promise<ErrorReportActionResultDto> {
    try {
      return await invoke<ErrorReportActionResultDto>("error_report_open_issue", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async openDataDirectory(): Promise<void> {
    await this.openDirectory("open_data_directory");
  }

  async openLogsDirectory(): Promise<void> {
    await this.openDirectory("open_logs_directory");
  }

  async openCacheDirectory(): Promise<void> {
    await this.openDirectory("open_cache_directory");
  }

  async cacheClear(scope: CacheScopeDto): Promise<CacheClearResultDto> {
    try {
      return await invoke<CacheClearResultDto>("cache_clear", { scope });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async videoScreenshotBegin(): Promise<VideoScreenshotBeginResultDto> {
    try {
      return await invoke<VideoScreenshotBeginResultDto>("video_screenshot_begin");
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async videoScreenshotChunk(request: VideoScreenshotChunkRequest): Promise<void> {
    try {
      await invoke<void>("video_screenshot_chunk", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async videoScreenshotCommit(uploadId: string): Promise<VideoScreenshotResultDto> {
    try {
      return await invoke<VideoScreenshotResultDto>("video_screenshot_commit", { uploadId });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async videoScreenshotCancel(uploadId: string): Promise<void> {
    try {
      await invoke<void>("video_screenshot_cancel", { uploadId });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async updateCheck(): Promise<UpdaterCheckResult> {
    return this.updater.check();
  }

  async updateInstall(onProgress?: (progress: UpdaterProgress) => void): Promise<UpdaterInstallResult> {
    return this.updater.install(onProgress);
  }

  private async openDirectory(command: "open_data_directory" | "open_logs_directory" | "open_cache_directory"): Promise<void> {
    try {
      await invoke<void>(command);
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async castDiscover(request: import("./generated/wire").CastDiscoverRequest): Promise<import("./generated/wire").CastDiscoverResult> {
    try {
      return await invoke<import("./generated/wire").CastDiscoverResult>("cast_discover", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async castPlay(request: import("./generated/wire").CastPlayRequest): Promise<import("./generated/wire").CastPlayResult> {
    try {
      return await invoke<import("./generated/wire").CastPlayResult>("cast_play", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async castStatus(request: import("./generated/wire").CastStatusRequest): Promise<import("./generated/wire").CastStatusDto> {
    try {
      return await invoke<import("./generated/wire").CastStatusDto>("cast_status", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async castStop(request: import("./generated/wire").CastStopRequest): Promise<import("./generated/wire").CastStopResult> {
    try {
      return await invoke<import("./generated/wire").CastStopResult>("cast_stop", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  // ---- A1 智能配置推荐（Agent 全局设置 Typed IPC） ----
  //
  // 每个命令都是闭合 typed 调用：没有 agent_invoke(commandName, arbitraryArguments)
  // 这类自由分发入口，也没有任何 token 字段参与请求或响应。

  async agentCapabilityManifestGet(): Promise<AgentCapabilityManifestDto> {
    try {
      return await invoke<AgentCapabilityManifestDto>("agent_capability_manifest_get");
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async agentSettingsContextGet(): Promise<AgentSettingsContextDto> {
    try {
      return await invoke<AgentSettingsContextDto>("agent_settings_context_get");
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async agentSettingsProposalCreate(
    request: AgentSettingsProposalCreateRequest,
  ): Promise<AgentSettingsProposalDto> {
    try {
      return await invoke<AgentSettingsProposalDto>("agent_settings_proposal_create", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async agentSettingsProposalGet(
    request: AgentSettingsProposalGetRequest,
  ): Promise<AgentSettingsProposalGetResultDto> {
    try {
      return await invoke<AgentSettingsProposalGetResultDto>("agent_settings_proposal_get", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async agentSettingsProposalReject(
    request: AgentSettingsProposalRejectRequest,
  ): Promise<AgentSettingsProposalRejectResultDto> {
    try {
      return await invoke<AgentSettingsProposalRejectResultDto>("agent_settings_proposal_reject", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async agentSettingsProposalApprove(
    request: AgentSettingsProposalApproveRequest,
  ): Promise<AgentSettingsProposalApproveResultDto> {
    try {
      return await invoke<AgentSettingsProposalApproveResultDto>("agent_settings_proposal_approve", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async agentSettingChangeReceiptGet(
    request: AgentSettingChangeReceiptGetRequest,
  ): Promise<AgentSettingChangeReceiptDto | null> {
    try {
      return await invoke<AgentSettingChangeReceiptDto | null>("agent_setting_change_receipt_get", {
        request,
      });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  // ---- 外观（Appearance Stage 1B）----
  //
  // 每个方法都是闭合 typed 调用：参数一律是 typed request（字段 camelCase，与
  // src-tauri/src/commands/appearance.rs 的命令签名逐字一致），没有裸路径入口。
  // 响应先过 settings-wire 的运行时守卫，形状漂移一律 INTERNAL_ERROR，不透传。

  async appearanceAssetsList(request: AppearanceAssetsListRequestWire): Promise<AppearanceAssetsWire> {
    try {
      const value: unknown = await invoke("appearance_assets_list", { request });
      if (!guardAppearanceAssets(value)) throw new Error("appearance_assets_list returned invalid data");
      return value;
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async agentResourcePreferenceProposalCreate(
    request: AgentResourcePreferenceProposalCreateRequest,
  ): Promise<AgentResourcePreferenceProposalDto> {
    try {
      return await invoke<AgentResourcePreferenceProposalDto>(
        "agent_resource_preference_proposal_create",
        { request },
      );
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async appearanceAssetImport(
    request: AppearanceAssetImportRequestWire,
  ): Promise<AppearanceAssetWire> {
    try {
      // 文件由后端 Native 选择器选定；用户取消时后端返回 OPERATION_CANCELLED。
      const value: unknown = await invoke("appearance_asset_import", { request });
      if (!guardAppearanceAsset(value)) throw new Error("appearance_asset_import returned invalid data");
      return value;
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async agentResourcePreferenceProposalGet(
    request: AgentResourcePreferenceProposalGetRequest,
  ): Promise<AgentResourcePreferenceProposalGetResultDto> {
    try {
      return await invoke<AgentResourcePreferenceProposalGetResultDto>(
        "agent_resource_preference_proposal_get",
        { request },
      );
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async appearanceAssetDelete(
    request: AppearanceAssetDeleteRequestWire,
  ): Promise<AppearanceAssetDeleteResultWire> {
    try {
      const value: unknown = await invoke("appearance_asset_delete", { request });
      if (!guardAppearanceAssetDeleteResult(value)) {
        throw new Error("appearance_asset_delete returned invalid data");
      }
      return value;
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async agentResourcePreferenceProposalApprove(
    request: AgentResourcePreferenceProposalApproveRequest,
  ): Promise<AgentResourcePreferenceProposalApproveResultDto> {
    try {
      return await invoke<AgentResourcePreferenceProposalApproveResultDto>(
        "agent_resource_preference_proposal_approve",
        { request },
      );
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async agentTraceGet(request: AgentTraceGetRequest): Promise<AgentTraceGetResultDto> {
    try {
      return await invoke<AgentTraceGetResultDto>("agent_trace_get", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  // ---- A2 AI Provider 基础切片 ----
  // 全部走 Application service：命令层不含 SQL / HTTP；模型发现由 Rust 侧
  // Infrastructure 适配器发起，WebView 不直接访问 Provider。

  async aiProviderProfileList(): Promise<AiProviderProfileListResultDto> {
    try {
      return await invoke<AiProviderProfileListResultDto>("ai_provider_profile_list");
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async aiProviderProfileGet(
    request: AiProviderProfileGetRequest,
  ): Promise<AiProviderProfileDto> {
    try {
      return await invoke<AiProviderProfileDto>("ai_provider_profile_get", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async aiProviderProfileUpsert(
    request: AiProviderProfileUpsertRequest,
  ): Promise<AiProviderProfileDto> {
    try {
      return await invoke<AiProviderProfileDto>("ai_provider_profile_upsert", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async aiProviderProfileDelete(
    request: AiProviderProfileDeleteRequest,
  ): Promise<AiProviderProfileDeleteResultDto> {
    try {
      return await invoke<AiProviderProfileDeleteResultDto>("ai_provider_profile_delete", {
        request,
      });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async aiProviderModelsList(
    request: AiProviderModelsListRequest,
  ): Promise<AiProviderModelsCatalogDto> {
    try {
      return await invoke<AiProviderModelsCatalogDto>("ai_provider_models_list", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async aiSettingsRecommendationGenerate(
    request: AiSettingsRecommendationGenerateRequest,
  ): Promise<AiSettingsRecommendationDto> {
    try {
      const value: unknown = await invoke<unknown>("ai_settings_recommendation_generate", {
        request,
      });
      if (!guardAiSettingsRecommendation(value, request.profileId)) {
        throw toHavenError({
          code: "INTERNAL_ERROR",
          userMessage: "AI 推荐接口返回了不完整或不匹配的数据",
          retryable: false,
        });
      }
      return value;
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async homeLayoutGet(): Promise<HomeLayoutSnapshotWire> {
    try {
      // 无入参命令：与 Rust `home_layout_get(state)` 签名一致。
      const value: unknown = await invoke("home_layout_get");
      if (!guardHomeLayoutSnapshot(value)) throw new Error("home_layout_get returned invalid data");
      return value;
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async homeLayoutSave(request: HomeLayoutSaveRequestWire): Promise<HomeLayoutMutationResultWire> {
    try {
      const value: unknown = await invoke("home_layout_save", { request });
      if (!guardHomeLayoutMutationResult(value)) throw new Error("home_layout_save returned invalid data");
      return value;
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async homeLayoutReset(request: HomeLayoutResetRequestWire): Promise<HomeLayoutMutationResultWire> {
    try {
      const value: unknown = await invoke("home_layout_reset", { request });
      if (!guardHomeLayoutMutationResult(value)) throw new Error("home_layout_reset returned invalid data");
      return value;
    } catch (error) {
      throw toHavenError(error);
    }
  }

  // ---- 设置页总览布局（048）----
  //
  // 命令名与 src-tauri/src/commands/appearance.rs 的签名逐字一致；响应先过运行时守卫，
  // 形状漂移（例如后端把首页布局当成总览布局返回）一律 INTERNAL_ERROR，不透传半份布局。

  async overviewLayoutGet(): Promise<OverviewLayoutSnapshotWire> {
    try {
      // 无入参命令：与 Rust `overview_layout_get(state)` 签名一致。
      const value: unknown = await invoke("overview_layout_get");
      if (!guardOverviewLayoutSnapshot(value)) throw new Error("overview_layout_get returned invalid data");
      return value;
    } catch (error) {
      throw toHavenError(error);
    }
  }

  // ---- A5 外部 Agent Broker（默认关闭） ----
  //
  // 三个命令都无参数：端点由 Rust 按平台解析（Windows Named Pipe / Unix socket），
  // WebView 既不能指定端点，也不能选择 Broker 背后的 API 实现。这里的 `endpoint`
  // 原样透传——它是可复制的本地配置值，不是 secret（契约 §4.2）。
  // busy / unavailable 是 Rust 的 fail-closed 结论，由 `reason` 携带稳定的脱敏文案；
  // 传输层不解释、不改写这两个状态，也不把它们兜底成 `disabled`。

  async agentBrokerStatus(): Promise<AgentBrokerStatusResultDto> {
    try {
      return await invoke<AgentBrokerStatusResultDto>("agent_broker_status");
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async agentBrokerEnable(): Promise<AgentBrokerStatusResultDto> {
    try {
      return await invoke<AgentBrokerStatusResultDto>("agent_broker_enable");
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async agentBrokerDisable(): Promise<AgentBrokerStatusResultDto> {
    try {
      return await invoke<AgentBrokerStatusResultDto>("agent_broker_disable");
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async agentSkillList(): Promise<AgentSkillListResultWire> {
    try {
      const value: unknown = await invoke<unknown>("agent_skill_list");
      if (!guardAgentSkillListResult(value)) invalidAgentSkillResponse();
      return value;
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async overviewLayoutSave(request: OverviewLayoutSaveRequestWire): Promise<OverviewLayoutMutationResultWire> {
    try {
      const value: unknown = await invoke("overview_layout_save", { request });
      if (!guardOverviewLayoutMutationResult(value)) throw new Error("overview_layout_save returned invalid data");
      return value;
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async agentSkillSetEnabled(
    request: AgentSkillSetEnabledRequestWire,
  ): Promise<AgentSkillStateWire> {
    try {
      const value: unknown = await invoke<unknown>("agent_skill_set_enabled", { request });
      if (!guardAgentSkillState(value)) invalidAgentSkillResponse();
      return value;
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async overviewLayoutReset(request: OverviewLayoutResetRequestWire): Promise<OverviewLayoutMutationResultWire> {
    try {
      const value: unknown = await invoke("overview_layout_reset", { request });
      if (!guardOverviewLayoutMutationResult(value)) throw new Error("overview_layout_reset returned invalid data");
      return value;
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async mcpClientConfigStatus(): Promise<McpClientConfigStatusWire> {
    try {
      const value: unknown = await invoke<unknown>("mcp_client_config_status");
      if (!guardMcpClientConfigStatus(value)) invalidMcpClientConfigResponse();
      return value;
    } catch (error) {
      throw toHavenError(error);
    }
  }

  // ---- 阅读总览（Reading Overview）----
  //
  // 无入参以外的自由字段：请求只有窗口天数与显式 UTC 偏移（字段 camelCase，与
  // src-tauri/src/commands/reading.rs 的命令签名逐字一致）。响应先过运行时守卫，
  // 形状漂移一律 INTERNAL_ERROR，不透传半份聚合数据给页面。

  async readingOverviewGet(request: ReadingOverviewGetRequestWire): Promise<ReadingOverviewWire> {
    try {
      const value: unknown = await invoke("reading_overview_get", { request });
      if (!guardReadingOverview(value)) throw new Error("reading_overview_get returned invalid data");
      return value;
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async mcpClientConfigApply(
    request: McpClientConfigureRequestWire,
  ): Promise<McpClientConfigStatusWire> {
    try {
      const value: unknown = await invoke<unknown>("mcp_client_config_apply", { request });
      if (!guardMcpClientConfigStatus(value)) invalidMcpClientConfigResponse();
      return value;
    } catch (error) {
      throw toHavenError(error);
    }
  }

  // ---- Film/TV Provider 基础切片：TVBox / FongMi 配置预览 ----
  //
  // 参数以 { request } 传递（字段 camelCase，与 src-tauri 的命令签名逐字一致）。
  // 这里只做类型化透传：响应形状守卫由 sources-gateway 统一施加，与
  // source_registry_list 同一约定。地址只在请求方向上出现，回程里没有它。
  async tvboxConfigPreview(request: TvboxConfigPreviewRequest): Promise<TvboxConfigPreviewDto> {
    try {
      return await invoke<TvboxConfigPreviewDto>("tvbox_config_preview", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }

  // ---- Film/TV Provider 基础切片：TVBox / FongMi 配置保存 ----
  //
  // 与预览同一约定：参数以 { request } 传递，响应形状守卫由 sources-gateway 施加。
  // 地址只在请求方向上出现；回程里没有它，也没有原始配置或凭据。
  async tvboxConfigSave(request: TvboxConfigSaveRequest): Promise<TvboxConfigSaveResult> {
    try {
      return await invoke<TvboxConfigSaveResult>("tvbox_config_save", { request });
    } catch (error) {
      throw toHavenError(error);
    }
  }
}

/** 形状守卫失败：与其它 gateway 同一约定，不把畸形载荷当成合法状态渲染。 */
function invalidAgentSkillResponse(): never {
  throw toHavenError({
    code: "INTERNAL_ERROR",
    userMessage: "内置技能接口返回了非法数据",
    retryable: false,
  });
}

/**
 * 客户端自动配置的状态守卫失败。
 *
 * 这份响应决定界面会不会给出一个"写入按钮"，因此畸形载荷必须 fail closed：把一份读不懂的
 * 状态渲染成"未配置、可写"，用户按下按钮时才知道有问题，而那时我们已经在碰他的配置文件了。
 */
function invalidMcpClientConfigResponse(): never {
  throw toHavenError({
    code: "INTERNAL_ERROR",
    userMessage: "客户端配置接口返回了非法数据",
    retryable: false,
  });
}
