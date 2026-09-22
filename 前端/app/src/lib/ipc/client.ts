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
  AgentSettingsContextDto,
  AgentResourcePreferenceProposalApproveRequest,
  AgentResourcePreferenceProposalApproveResultDto,
  AgentResourcePreferenceProposalCreateRequest,
  AgentResourcePreferenceProposalDto,
  AgentResourcePreferenceProposalGetRequest,
  AgentResourcePreferenceProposalGetResultDto,
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
  comicChapterCatalogGet(request: ComicChapterCatalogGetRequest): Promise<ComicChapterCatalogDto>;
  comicChapterCatalogRegisteredGet(
    request: ComicChapterCatalogGetRequest,
  ): Promise<ComicRegisteredChapterCatalogDto>;
  comicChapterCatalogRefresh(request: ComicChapterCatalogGetRequest): Promise<ComicChapterCatalogDto>;
  comicWorkChapterCatalogGet(
    request: ComicWorkChapterCatalogRequestDto,
  ): Promise<ComicWorkChapterCatalogDto>;
  comicWorkChapterCatalogRefresh(
    request: ComicWorkChapterCatalogRequestDto,
  ): Promise<ComicWorkChapterCatalogDto>;
  comicChapterSourceCandidatesGet(
    request: ComicChapterSourceCandidatesGetRequestDto,
  ): Promise<ComicChapterSourceCandidatesDto>;
  comicProgressMigrate(
    request: ComicProgressMigrationRequestDto,
  ): Promise<ComicProgressMigrationResultDto>;
  comicProgressRemap(
    request: ComicPageProgressRemapRequestDto,
  ): Promise<ComicProgressMigrationResultDto>;
  comicProgressRevert(
    request: ComicProgressMigrationRevertRequestDto,
  ): Promise<ComicProgressMigrationRevertResultDto>;
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
  progressMarkCompleted(request: ProgressMarkCompletedRequest): Promise<ProgressSaveResult>;
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
  // ---- V1.0.0 报刊产品化 ----
  /**
   * 按 Work 身份读取「期刊 → 卷 → 期 → 文章」层级。响应只含本地身份与展示事实，
   * 不含正文 URL、Cookie、请求头、本地路径或签名资源地址。
   */
  periodicalTreeGet(request: PeriodicalTreeGetRequest): Promise<PeriodicalTreeDto>;
  // ---- A1 智能配置推荐（Agent 全局设置 Typed IPC） ----
  /**
   * 服务端固定的 Agent 能力清单（只含本切片已实现能力，不接受任何入参）。
   */
  agentCapabilityManifestGet(): Promise<AgentCapabilityManifestDto>;
  /**
   * 读取全局阅读设置上下文：脱敏快照 + revision + context id/hash + 能力清单。
   * 返回里没有 secret、绝对路径、正文或任意 JSON。
   */
  agentSettingsContextGet(): Promise<AgentSettingsContextDto>;
  /**
   * 创建设置提案（只创建，不写设置）。必须回传最近一次读取得到的
   * contextId / contextHash / baseRevision。
   */
  agentSettingsProposalCreate(
    request: AgentSettingsProposalCreateRequest,
  ): Promise<AgentSettingsProposalDto>;
  /** 回读提案与（可能存在的）回执。 */
  agentSettingsProposalGet(
    request: AgentSettingsProposalGetRequest,
  ): Promise<AgentSettingsProposalGetResultDto>;
  /** 拒绝提案（零设置写入）。 */
  agentSettingsProposalReject(
    request: AgentSettingsProposalRejectRequest,
  ): Promise<AgentSettingsProposalRejectResultDto>;
  /**
   * 用户批准：唯一写入口。只提交提案 ID 与 UI 显示的那份 canonical digest；
   * 一次性审批令牌由 Rust 内部生成并消费，不出现在请求或响应里。
   */
  agentSettingsProposalApprove(
    request: AgentSettingsProposalApproveRequest,
  ): Promise<AgentSettingsProposalApproveResultDto>;
  /** 读取变更回执（未应用返回 null）。 */
  agentSettingChangeReceiptGet(
    request: AgentSettingChangeReceiptGetRequest,
  ): Promise<AgentSettingChangeReceiptDto | null>;
  /** 创建资源级偏好提案（只创建，不写入 authoritative 偏好）。 */
  agentResourcePreferenceProposalCreate(
    request: AgentResourcePreferenceProposalCreateRequest,
  ): Promise<AgentResourcePreferenceProposalDto>;
  /** 回读资源级提案与（可能存在的）回执，供未来 UI 重建同一份 Diff。 */
  agentResourcePreferenceProposalGet(
    request: AgentResourcePreferenceProposalGetRequest,
  ): Promise<AgentResourcePreferenceProposalGetResultDto>;
  /** 用户批准资源级提案；CAS 与 Receipt 仍由 Rust Application service 执行。 */
  agentResourcePreferenceProposalApprove(
    request: AgentResourcePreferenceProposalApproveRequest,
  ): Promise<AgentResourcePreferenceProposalApproveResultDto>;
  /** 读取指定 Agent 会话的有界轨迹（当前仅 typed backend，尚无 UI）。 */
  agentTraceGet(request: AgentTraceGetRequest): Promise<AgentTraceGetResultDto>;
  // ---- A2 AI Provider 基础切片（docs/architecture/AI_SYSTEM.md） ----
  /**
   * 列出全部 AI Provider Profile 与其凭据配置状态。
   * 返回里没有 API key、credentialRef 或凭据 target 名。
   */
  aiProviderProfileList(): Promise<AiProviderProfileListResultDto>;
  /** 读取单个 AI Provider Profile（不存在抛 NOT_FOUND）。 */
  aiProviderProfileGet(request: AiProviderProfileGetRequest): Promise<AiProviderProfileDto>;
  /**
   * 写入 AI Provider Profile（非敏感字段）。CAS：`expectedRevision` 不对即冲突。
   * API key 不经过这里，走 `credentialSet({ provider: "ai" })`。
   */
  aiProviderProfileUpsert(
    request: AiProviderProfileUpsertRequest,
  ): Promise<AiProviderProfileDto>;
  /** 删除 Profile：先清理 `haven:ai:<profileId>` 凭据，再 CAS 删除行。 */
  aiProviderProfileDelete(
    request: AiProviderProfileDeleteRequest,
  ): Promise<AiProviderProfileDeleteResultDto>;
  /**
   * 读取 Provider 的模型目录（只读、有界、非敏感投影）。
   * 未配置密钥 / 被禁用 / 目录为空都是空目录 + 明确 state，不是错误。
   */
  aiProviderModelsList(request: AiProviderModelsListRequest): Promise<AiProviderModelsCatalogDto>;
  /**
   * 让已配置 Provider 生成一次结构化阅读设置建议；返回值只包含 pending Proposal，
   * 不包含 Apply / Approval / Token，也不会绕过 Haven 的确认链。
   */
  aiSettingsRecommendationGenerate(
    request: AiSettingsRecommendationGenerateRequest,
  ): Promise<AiSettingsRecommendationDto>;
  // ---- A5 外部 Agent Broker（默认关闭；docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md §4.5）----
  //
  // 三个命令都不接受入参：端点由 Rust 按平台解析，调用方无法指定，也没有
  // `invoke(commandName, args)` 这类自由分发入口。状态是运行时事实，不是用户授权开关。
  /**
   * 读取当前状态（默认 `disabled`）。**不**开启端点、不探测连接。
   * `endpoint` 只在 `listening` 时出现；它不是秘密，是可复制的本地配置值。
   */
  agentBrokerStatus(): Promise<AgentBrokerStatusResultDto>;
  /** 用户显式开启外部 Agent 接入；已开启时幂等返回同一端点。失败即 fail closed。 */
  agentBrokerEnable(): Promise<AgentBrokerStatusResultDto>;
  /** 用户显式关闭：停止监听、断开在途连接，返回关闭后的状态（`endpoint` 为 null）。 */
  agentBrokerDisable(): Promise<AgentBrokerStatusResultDto>;
}
