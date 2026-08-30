// HavenClient 接口（IPC-FE-001 最小前置；冻结前只定义首批三条 API，无 Tauri 依赖）。
// 契约：plan/FRONTEND_BACKEND_CONTRACT.md §14.5 冻结矩阵（CONTRACT-LIBRARY-LIST-001 /
// CONTRACT-FAVORITE-SET-001 / CONTRACT-SCAN-START-001）。

import type {
  FavoriteSetRequest,
  FavoriteSetResult,
  HomeDto,
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
import type { EditionListByWorkRequest, EditionListByWorkResultDto } from "../../features/media/ipc/edition-wire";
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

/** 仅传递脱敏更新元数据；远端 raw JSON 和签名内容不离开 Tauri 客户端。 */
export interface UpdaterCheckResult {
  status: "up_to_date" | "available";
  currentVersion: string | null;
  availableVersion: string | null;
  releaseNotes: string | null;
  publishedAt: string | null;
}

export interface UpdaterInstallResult {
  status: "installed";
}

export interface HavenClient {
  libraryList(request: LibraryListRequest): Promise<PageDto<WorkCardDto>>;
  favoriteSet(request: FavoriteSetRequest): Promise<FavoriteSetResult>;
  /** onEvent：扫描 Channel 回调（Tauri 环境逐事件推送；Mock 环境不触发）。 */
  libraryScanStart(
    request: LibraryScanStartRequest,
    onEvent?: (event: LibraryScanEvent) => void,
  ): Promise<ScanStartResult>;
  workGet(request: WorkGetRequest): Promise<WorkDetailHeaderDto>;
  editionListByWork(request: EditionListByWorkRequest): Promise<EditionListByWorkResultDto>;
  editionGet(request: EditionGetRequest): Promise<EditionDetailDto>;
  resourceListByMediaItem(request: ResourceListByMediaItemRequest): Promise<ResourceListDto>;
  sessionOpen(request: SessionOpenRequest): Promise<SessionOpenResultDto>;
  comicPageManifestGet(request: ComicPageManifestGetRequest): Promise<ComicPageManifestDto>;
  readerTocGet(request: ReaderTocGetRequest): Promise<ReaderTocResultDto>;
  readerSearch(request: ReaderSearchRequest): Promise<ReaderSearchResultDto>;
  readerSearchStart(
    request: ReaderSearchRequest,
    onEvent?: (event: ReaderSearchEvent) => void,
  ): Promise<ReaderSearchResultDto>;
  readerSearchCancel(request: ReaderSearchCancelRequest): Promise<ReaderSearchCancelResultDto>;
  sessionClose(request: SessionCloseRequest): Promise<SessionCloseResultDto>;
  progressSave(request: ProgressSaveRequest): Promise<ProgressSaveResult>;
  progressRecent(request: ProgressRecentRequest): Promise<ProgressSummaryDto[]>;
  progressReset(request: ProgressResetRequest): Promise<void>;
  historyList(request: HistoryListRequest): Promise<HistoryEntryDto[]>;
  historyClear(): Promise<void>;
  searchHistoryList(request: SearchHistoryListRequest): Promise<SearchHistoryEntryDto[]>;
  searchHistoryRecord(request: SearchHistoryRecordRequest): Promise<SearchHistoryEntryDto>;
  searchHistoryRemove(request: SearchHistoryRemoveRequest): Promise<boolean>;
  searchHistoryClear(): Promise<void>;
  markerCreate(request: MarkerCreateRequest): Promise<MarkerDto>;
  markerList(request: MarkerListRequest): Promise<MarkerDto[]>;
  markerListAll(request: MarkerListAllRequest): Promise<MarkerDto[]>;
  markerDelete(request: MarkerDeleteRequest): Promise<boolean>;
  homeGet(): Promise<HomeDto>;
  settingsGet(section: SettingsSectionWire): Promise<SettingsSnapshot>;
  settingsUpdate(request: SettingsUpdateRequest): Promise<SettingsUpdateResult>;
  preferenceGet(request: PreferenceGetRequest): Promise<PreferenceGetResult>;
  preferenceUpdate(request: PreferenceUpdateRequest): Promise<PreferenceUpdateResult>;
  storageLocationList(): Promise<StorageLocationDto[]>;
  downloadCreate(request: DownloadCreateRequest): Promise<DownloadTaskDto>;
  downloadList(request: DownloadListRequest): Promise<DownloadTaskDto[]>;
  downloadPause(request: DownloadTaskActionRequest): Promise<DownloadTaskDto>;
  downloadResume(request: DownloadTaskActionRequest): Promise<DownloadTaskDto>;
  downloadCancel(request: DownloadTaskActionRequest): Promise<DownloadTaskDto>;
  downloadRetry(request: DownloadTaskActionRequest): Promise<DownloadTaskDto>;
  downloadRemoveRecord(request: DownloadTaskActionRequest): Promise<DownloadMutationResultDto>;
  downloadDeleteOffline(request: DownloadTaskActionRequest): Promise<DownloadMutationResultDto>;
  downloadRevealOffline(request: DownloadTaskActionRequest): Promise<DownloadRevealResultDto>;
  downloadSubscribe(
    subscriptionId: string,
    onEvent: (event: DownloadEvent) => void,
  ): Promise<() => Promise<void>>;
  // ---- v0.2 契约冻结（契约 §36；CONTRACT-V02-*）----
  sourceRegistryList(): Promise<SourceRegistryDto>;
  sourceRegistrySet(request: SourceRegistrySetRequest): Promise<SourceRegistrySetResult>;
  /** 端点只写后端持久化；响应不含端点地址（契约 §36.2 演进，V2-B）。 */
  sourceRegistrySetEndpoint(request: SourceEndpointSetRequest): Promise<SourceEndpointSetResult>;
  /** V2-H 收尾批次：自定义 OPDS 书源生命周期与凭据。 */
  sourceAdd(
    request: import("./generated/wire").SourceAddRequest,
  ): Promise<import("./generated/wire").SourceAddResult>;
  sourceUpdate(
    request: import("./generated/wire").SourceUpdateRequest,
  ): Promise<import("./generated/wire").SourceUpdateResult>;
  sourceRemove(
    request: import("./generated/wire").SourceRemoveRequest,
  ): Promise<import("./generated/wire").SourceRemoveResult>;
  sourceSetCredential(request: import("./generated/wire").SourceSetCredentialRequest): Promise<void>;
  searchSourceStart(
    request: SearchSourceStartRequest,
    onEvent?: (event: SearchSourceEvent) => void,
  ): Promise<SearchStartResultDto>;
  searchSourceCancel(request: SearchSourceCancelRequest): Promise<SearchSourceCancelResultDto>;
  /** 导入搜索候选（幂等）；返回真实 Work/MediaItem 身份。 */
  sourceWorkImport(request: SourceWorkImportRequest): Promise<SourceWorkImportResult>;
  credentialStatus(request: CredentialStatusRequest): Promise<CredentialStatusDto>;
  credentialSet(request: CredentialSetRequest): Promise<void>;
  credentialDelete(request: CredentialDeleteRequest): Promise<void>;
  /** V2-G 接线前在真实 Tauri 环境 fail closed（契约 §36.10 批次归属）。 */
  mediaStateGet(request: MediaStateGetRequest): Promise<MediaStateDto>;
  /** V2-F 接线前返回空记录集（契约 §36.8：无流水线记录不伪造 pending）。 */
  enrichmentStatus(request: EnrichmentStatusRequest): Promise<EnrichmentStateDto[]>;
  /** 远端流播放会话（V2-B；返回 haven-resource://stream/<grant> 代理 URI）。 */
  streamOpen(request: SessionOpenRequest): Promise<SessionOpenResultDto>;
  streamClose(request: SessionCloseRequest): Promise<boolean>;
  trendingBoardsGet(): Promise<import("./generated/wire").TrendingBoardsDto>;
  trendingBoardsRefresh(): Promise<import("./generated/wire").TrendingBoardsDto>;
  appInfoGet(): Promise<AppInfoDto>;
  errorReportPreviewGet(request: ErrorReportPreviewRequest): Promise<ErrorReportPreviewDto>;
  errorReportConfirm(request: ErrorReportConfirmRequest): Promise<ErrorReportConfirmResultDto>;
  errorReportExport(request: ErrorReportActionRequest): Promise<ErrorReportActionResultDto>;
  errorReportOpenIssue(request: ErrorReportActionRequest): Promise<ErrorReportActionResultDto>;
  openDataDirectory(): Promise<void>;
  openLogsDirectory(): Promise<void>;
  openCacheDirectory(): Promise<void>;
  cacheClear(scope: CacheScopeDto): Promise<CacheClearResultDto>;
  videoScreenshotBegin(): Promise<VideoScreenshotBeginResultDto>;
  videoScreenshotChunk(request: VideoScreenshotChunkRequest): Promise<void>;
  videoScreenshotCommit(uploadId: string): Promise<VideoScreenshotResultDto>;
  videoScreenshotCancel(uploadId: string): Promise<void>;
  updateCheck(): Promise<UpdaterCheckResult>;
  updateInstall(): Promise<UpdaterInstallResult>;
  castDiscover(request: import("./generated/wire").CastDiscoverRequest): Promise<import("./generated/wire").CastDiscoverResult>;
  castPlay(request: import("./generated/wire").CastPlayRequest): Promise<import("./generated/wire").CastPlayResult>;
  castStatus(request: import("./generated/wire").CastStatusRequest): Promise<import("./generated/wire").CastStatusDto>;
  castStop(request: import("./generated/wire").CastStopRequest): Promise<import("./generated/wire").CastStopResult>;
}
