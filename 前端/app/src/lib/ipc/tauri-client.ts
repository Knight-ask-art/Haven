// TauriHavenClient（IPC-FE-001：HavenClient 的真实 Tauri invoke 实现）。
// 契约：plan/FRONTEND_BACKEND_CONTRACT.md §14.5 冻结矩阵；命令参数统一以
// { request } 传递（与 src-tauri 命令签名及 ipc_e2e_test 调用体一致，字段 camelCase）。
// 错误：命令返回 Result<_, ErrorDto>，reject 的即契约 ErrorDto 形状，
// 经 toHavenError 归一（非契约形状兜底 INTERNAL_ERROR，见 errors.ts）。

import { Channel, invoke } from "@tauri-apps/api/core";
import { check, type Update } from "@tauri-apps/plugin-updater";

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
} from "./generated/wire";
import type { UpdaterCheckResult, UpdaterInstallResult } from "./client";
import { HavenError, toHavenError } from "./errors.js";
import type {
  SettingsSectionWire,
  SettingsSnapshot,
  SettingsUpdateRequest,
  SettingsUpdateResult,
  PreferenceGetRequest,
  PreferenceGetResult,
  PreferenceUpdateRequest,
  PreferenceUpdateResult,
} from "./settings-wire";
import {
  guardPreferenceGetResult,
  guardPreferenceUpdateResult,
  guardSettingsSnapshot,
  guardSettingsUpdateResult,
} from "./settings-wire.js";

/** 真实 IPC Client（仅 Tauri WebView 内可用；浏览器环境由 runtime.ts 拦截回落 Mock）。 */
export class TauriHavenClient implements HavenClient {
  private pendingUpdate: Update | null = null;

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

  async comicPageManifestGet(
    request: ComicPageManifestGetRequest,
  ): Promise<ComicPageManifestDto> {
    try {
      return await invoke<ComicPageManifestDto>("comic_page_manifest_get", { request });
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
    try {
      if (this.pendingUpdate) {
        await this.pendingUpdate.close();
        this.pendingUpdate = null;
      }
      const update = await check({ timeout: 10_000 });
      if (!update) {
        return {
          status: "up_to_date",
          currentVersion: null,
          availableVersion: null,
          releaseNotes: null,
          publishedAt: null,
        };
      }
      this.pendingUpdate = update;
      return {
        status: "available",
        currentVersion: update.currentVersion,
        availableVersion: update.version,
        releaseNotes: safeUpdateText(update.body),
        publishedAt: safeUpdateText(update.date),
      };
    } catch (error) {
      throw toHavenError(error);
    }
  }

  async updateInstall(): Promise<UpdaterInstallResult> {
    const update = this.pendingUpdate;
    if (!update) {
      throw new HavenError({
        code: "UPDATER_NO_UPDATE",
        userMessage: "没有可安装的更新，请先检查更新",
        retryable: true,
      });
    }
    try {
      // The official plugin verifies the signature before launching the Windows
      // installer. On Windows it exits the current process so the installer can
      // replace the application atomically.
      await update.downloadAndInstall();
      this.pendingUpdate = null;
      return { status: "installed" };
    } catch (error) {
      throw toHavenError(error);
    }
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
}

function safeUpdateText(value: string | undefined): string | null {
  if (!value) return null;
  const normalized = Array.from(value, (character) => {
    const codePoint = character.codePointAt(0) ?? 0;
    return codePoint <= 0x1f || codePoint === 0x7f ? " " : character;
  }).join("").trim();
  return normalized ? normalized.slice(0, 2000) : null;
}
