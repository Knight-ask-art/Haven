// Fixture 装载验证（C-07 / R-C04：TypeScript 端）。
// 双保险：
// 1. 编译期：类型断言进入 wire.ts（tsc 检查字段形状）。
// 2. 运行时：守卫函数验证关键契约不变量（schemaVersion 字面量、闭合枚举、
//    T|null 形状、revision 存在性）——本脚本必须被实际执行（npm run fixtures:check
//    会编译后运行，不再只做 --noEmit）。
// 禁止用裸 `as` 绕过结构验证。

import type {
  ComicPageManifestDto,
  CredentialStatusDto,
  EnrichmentStateDto,
  ErrorDto,
  FavoriteChangedDto,
  FavoriteSetRequest,
  FavoriteSetResult,
  LibraryChangedDto,
  LibraryScanEvent,
  LibraryShelvesDto,
  MediaStateDto,
  MetadataChangedDto,
  PageDto,
  ReaderTocResultDto,
  ScanCancelResultDto,
  ScanPhase,
  ScanStartResult,
  SearchSourceCancelResultDto,
  SearchSourceEvent,
  SearchStartResultDto,
  SourceRegistryDto,
  SourceCategoryDto,
  StorageLocationDto,
  ResourceListDto,
  WorkCardDto,
} from "../src/lib/ipc/generated/wire";
import { isComicPageManifestDto } from "../src/lib/ipc/comic-page-manifest.js";
import { isReaderTocResultDto, isTocItemDto } from "../src/lib/ipc/reader-toc.js";

import listNormal from "../../../contracts/ipc/v1/fixtures/library/list.normal.json" with { type: "json" };
import listEmpty from "../../../contracts/ipc/v1/fixtures/library/list.empty.json" with { type: "json" };
import listError from "../../../contracts/ipc/v1/fixtures/library/list.error.json" with { type: "json" };
import listMissingArtwork from "../../../contracts/ipc/v1/fixtures/library/list.missing-artwork.json" with { type: "json" };
import shelvesNormal from "../../../contracts/ipc/v1/fixtures/library/shelves.normal.json" with { type: "json" };
import favoriteSet from "../../../contracts/ipc/v1/fixtures/favorite/set.normal.json" with { type: "json" };
import favoriteSuccess from "../../../contracts/ipc/v1/fixtures/favorite/set.success.json" with { type: "json" };
import favoriteFirstFalse from "../../../contracts/ipc/v1/fixtures/favorite/set.first-false.json" with { type: "json" };
import favoriteRepeated from "../../../contracts/ipc/v1/fixtures/favorite/set.repeated-idempotent.json" with { type: "json" };
import favoriteError from "../../../contracts/ipc/v1/fixtures/favorite/set.error-work-not-found.json" with { type: "json" };
import scanCompleted from "../../../contracts/ipc/v1/fixtures/scan/terminal.completed.json" with { type: "json" };
import scanCancelled from "../../../contracts/ipc/v1/fixtures/scan/terminal.cancelled.json" with { type: "json" };
import scanFailed from "../../../contracts/ipc/v1/fixtures/scan/terminal.failed.json" with { type: "json" };
import scanWarning from "../../../contracts/ipc/v1/fixtures/scan/warning.json" with { type: "json" };
import scanAlreadyRunning from "../../../contracts/ipc/v1/fixtures/scan/already-running.json" with { type: "json" };
import scanCancelAccepted from "../../../contracts/ipc/v1/fixtures/scan/cancel.accepted.json" with { type: "json" };
import scanCancelTerminal from "../../../contracts/ipc/v1/fixtures/scan/cancel.terminal.json" with { type: "json" };
import storageListNormal from "../../../contracts/ipc/v1/fixtures/storage/list.normal.json" with { type: "json" };
import storageListEmpty from "../../../contracts/ipc/v1/fixtures/storage/list.empty.json" with { type: "json" };
import libraryChanged from "../../../contracts/ipc/v1/fixtures/events/library.changed.json" with { type: "json" };
import favoriteChanged from "../../../contracts/ipc/v1/fixtures/events/favorite.changed.json" with { type: "json" };
import errorCatalog from "../../../contracts/ipc/v1/fixtures/errors/catalog.json" with { type: "json" };
import resourceMixedAvailability from "../../../contracts/ipc/v1/fixtures/resource/list.mixed-availability.json" with { type: "json" };
import comicPageManifest from "../../../contracts/ipc/v1/fixtures/comic/page-manifest.normal.json" with { type: "json" };
import comicPageManifestEmpty from "../../../contracts/ipc/v1/fixtures/comic/page-manifest.empty.json" with { type: "json" };
import comicPageManifestPartial from "../../../contracts/ipc/v1/fixtures/comic/page-manifest.partial-unavailable.json" with { type: "json" };
import readerTocNormal from "../../../contracts/ipc/v1/fixtures/reader/toc.normal.json" with { type: "json" };
import readerTocEmpty from "../../../contracts/ipc/v1/fixtures/reader/toc.empty.json" with { type: "json" };
// v0.2 契约冻结（契约 §36；CONTRACT-V02-*）
import sourceRegistryNormal from "../../../contracts/ipc/v1/fixtures/source/registry.normal.json" with { type: "json" };
import sourceSetRequest from "../../../contracts/ipc/v1/fixtures/source/set.request.json" with { type: "json" };
import sourceSetSuccess from "../../../contracts/ipc/v1/fixtures/source/set.success.json" with { type: "json" };
import sourceSetErrorUnknown from "../../../contracts/ipc/v1/fixtures/source/set.error-unknown-source.json" with { type: "json" };
import searchStartRequest from "../../../contracts/ipc/v1/fixtures/search/start.request.json" with { type: "json" };
import searchStartSuccess from "../../../contracts/ipc/v1/fixtures/search/start.success.json" with { type: "json" };
import searchStartAlreadyRunning from "../../../contracts/ipc/v1/fixtures/search/start.already-running.json" with { type: "json" };
import searchSourceStarted from "../../../contracts/ipc/v1/fixtures/search/source.started.json" with { type: "json" };
import searchSourceResult from "../../../contracts/ipc/v1/fixtures/search/source.source-result.json" with { type: "json" };
import searchSourceWarning from "../../../contracts/ipc/v1/fixtures/search/source.warning.json" with { type: "json" };
import searchSourceCompleted from "../../../contracts/ipc/v1/fixtures/search/source.completed.json" with { type: "json" };
import searchSourceFailed from "../../../contracts/ipc/v1/fixtures/search/source.failed.json" with { type: "json" };
import searchSourceCancelled from "../../../contracts/ipc/v1/fixtures/search/source.cancelled.json" with { type: "json" };
import searchCancelAccepted from "../../../contracts/ipc/v1/fixtures/search/cancel.accepted.json" with { type: "json" };
import searchCancelTerminal from "../../../contracts/ipc/v1/fixtures/search/cancel.terminal.json" with { type: "json" };
import credentialStatusConfigured from "../../../contracts/ipc/v1/fixtures/credential/status.configured.json" with { type: "json" };
import credentialStatusNotConfigured from "../../../contracts/ipc/v1/fixtures/credential/status.not-configured.json" with { type: "json" };
import credentialSetRequest from "../../../contracts/ipc/v1/fixtures/credential/set.request.json" with { type: "json" };
import credentialDeleteRequest from "../../../contracts/ipc/v1/fixtures/credential/delete.request.json" with { type: "json" };
import mediaStateNormal from "../../../contracts/ipc/v1/fixtures/media-state/state.normal.json" with { type: "json" };
import enrichmentStatusRequest from "../../../contracts/ipc/v1/fixtures/enrichment/status.request.json" with { type: "json" };
import enrichmentStatePending from "../../../contracts/ipc/v1/fixtures/enrichment/state.pending.json" with { type: "json" };
import enrichmentStateEnriched from "../../../contracts/ipc/v1/fixtures/enrichment/state.enriched.json" with { type: "json" };
import metadataChangedEvent from "../../../contracts/ipc/v1/fixtures/events/metadata.changed.json" with { type: "json" };
import resourceRemoteStream from "../../../contracts/ipc/v1/fixtures/resource/list.remote-stream.json" with { type: "json" };

// 统计实际已 import 的契约样例；Mock 动态行为结果不计入该数量。
const loadedBaseFixtureSamples: readonly unknown[] = [
  listNormal,
  listEmpty,
  listError,
  listMissingArtwork,
  shelvesNormal,
  favoriteSet,
  favoriteSuccess,
  favoriteFirstFalse,
  favoriteRepeated,
  favoriteError,
  scanCompleted,
  scanCancelled,
  scanFailed,
  scanWarning,
  scanAlreadyRunning,
  scanCancelAccepted,
  scanCancelTerminal,
  storageListNormal,
  storageListEmpty,
  libraryChanged,
  favoriteChanged,
  errorCatalog,
  resourceMixedAvailability,
  comicPageManifest,
  comicPageManifestEmpty,
  comicPageManifestPartial,
  readerTocNormal,
  readerTocEmpty,
  // v0.2 契约冻结（契约 §36）
  sourceRegistryNormal,
  sourceSetRequest,
  sourceSetSuccess,
  sourceSetErrorUnknown,
  searchStartRequest,
  searchStartSuccess,
  searchStartAlreadyRunning,
  searchSourceStarted,
  searchSourceResult,
  searchSourceWarning,
  searchSourceCompleted,
  searchSourceFailed,
  searchSourceCancelled,
  searchCancelAccepted,
  searchCancelTerminal,
  credentialStatusConfigured,
  credentialStatusNotConfigured,
  credentialSetRequest,
  credentialDeleteRequest,
  mediaStateNormal,
  enrichmentStatusRequest,
  enrichmentStatePending,
  enrichmentStateEnriched,
  metadataChangedEvent,
  resourceRemoteStream,
];

const TERMINAL: ScanPhase[] = ["completed", "cancelled", "failed"];
const LABEL_HINTS = ["start", "continue", "open"];
const STORAGE_LOCATION_FIELDS = new Set(["locationId", "displayName", "providerType", "status"]);
const FORBIDDEN_STORAGE_LOCATION_FIELDS = [
  "rootPath",
  "rootRef",
  "root_ref",
  "credentialRef",
  "credential_ref",
];
const SCAN_PHASES = [
  "started",
  "enumerating",
  "detecting",
  "fingerprinting",
  "indexing",
  "item_indexed",
  "warning",
  ...TERMINAL,
];
const STORAGE_PROVIDERS = ["local", "web_dav", "one_drive", "google_drive"];
const STORAGE_STATUSES = [
  "connected",
  "disconnected",
  "auth_expired",
  "unavailable",
  "read_only",
  "error",
  "disabled",
  "missing",
];
const RESOURCE_TYPES = [
  "local_file", "cloud_file", "http_file", "video_stream", "hls_stream", "dash_stream",
  "publication_file", "comic_archive", "image_sequence", "article_snapshot", "remote_chapter",
  "remote_page_set", "remote_stream",
];
const AVAILABILITIES = [
  "available", "offline_available", "temporarily_unavailable", "source_unavailable",
  "storage_unavailable", "missing", "unknown",
];
// v0.2 闭合枚举（契约 §36）
const SOURCE_KINDS = ["search", "online_read", "offline_download"];
const SOURCE_CATEGORIES = ["video", "book", "comic", "periodical"];
const SOURCE_MODES = ["single", "collection"];
const SOURCE_HEALTHS = ["unknown", "ok", "degraded", "down"];
const SEARCH_EVENT_KINDS = ["started", "source_result", "warning", "completed", "cancelled", "failed"];
const EXTERNAL_ID_PROVIDERS = ["tmdb", "bangumi", "anilist", "tvmaze", "gutenberg", "openlibrary"];
const LOCATOR_KINDS = ["video", "book", "pdf", "comic", "article"];
const ENRICHMENT_STATUSES = ["pending", "enriched", "failed"];

function fail(msg: string, data: unknown): never {
  throw new Error(`${msg}: ${JSON.stringify(data).slice(0, 200)}`);
}

// ---- 守卫：关键契约不变量（schemaVersion 字面量 / 闭合枚举 / T|null 形状）----

function guardPage(dto: unknown): dto is PageDto<WorkCardDto> {
  const v = dto as Record<string, unknown>;
  if (v.schemaVersion !== 1) fail("PageDto.schemaVersion 必须为 1", v);
  if (!Array.isArray(v.items)) fail("PageDto.items 必须是数组", v);
  if (v.nextCursor !== null && typeof v.nextCursor !== "string") fail("nextCursor 形状", v);
  if (v.total !== null && typeof v.total !== "number") fail("total 形状", v);
  return true;
}

function guardError(dto: unknown): dto is ErrorDto {
  const v = dto as Record<string, unknown>;
  if (typeof v.code !== "string" || v.code.length === 0) fail("ErrorDto.code", v);
  if (typeof v.userMessage !== "string") fail("ErrorDto.userMessage", v);
  if (typeof v.retryable !== "boolean") fail("ErrorDto.retryable", v);
  return true;
}

function guardShelves(dto: unknown): dto is LibraryShelvesDto {
  const v = dto as Record<string, unknown>;
  if (v.schemaVersion !== 1) fail("LibraryShelvesDto.schemaVersion", v);
  if (!Array.isArray(v.shelves)) fail("LibraryShelvesDto.shelves", v);
  return true;
}

function guardScanEvent(dto: unknown): dto is LibraryScanEvent {
  const v = dto as Record<string, unknown>;
  if (typeof v.operationId !== "string") fail("ScanEvent.operationId", v);
  if (typeof v.sequence !== "number" || v.sequence < 1) fail("ScanEvent.sequence", v);
  if (typeof v.at !== "string" || !v.at.includes("T")) fail("ScanEvent.at RFC3339", v);
  if (!SCAN_PHASES.includes(v.kind as string)) fail("ScanEvent.kind 闭合枚举", v);
  return true;
}

function guardFavoriteRequest(dto: unknown): dto is FavoriteSetRequest {
  const v = dto as Record<string, unknown>;
  if (typeof v.workId !== "string" || v.workId.length === 0) fail("FavoriteSetRequest.workId", v);
  if (typeof v.favorite !== "boolean") fail("FavoriteSetRequest.favorite", v);
  return true;
}

function guardFavoriteResult(dto: unknown): dto is FavoriteSetResult {
  const v = dto as Record<string, unknown>;
  if (typeof v.workId !== "string" || v.workId.length === 0) fail("FavoriteSetResult.workId", v);
  if (typeof v.favorite !== "boolean") fail("FavoriteSetResult.favorite", v);
  // R-FAV-002：revision 允许 null（从未变更，含首次 favorite=false）；非 null 时必须为非空 string。
  if (v.revision !== null && (typeof v.revision !== "string" || v.revision.length === 0)) {
    fail("FavoriteSetResult.revision 必须为 null 或非空 string", v);
  }
  return true;
}

function guardScanStartResult(dto: unknown): dto is ScanStartResult {
  const v = dto as Record<string, unknown>;
  if (v.schemaVersion !== 1) fail("ScanStartResult.schemaVersion", v);
  if (typeof v.operationId !== "string" || v.operationId.length === 0) fail("ScanStartResult.operationId", v);
  if (typeof v.taskId !== "string" || v.taskId.length === 0) fail("ScanStartResult.taskId", v);
  if (typeof v.alreadyRunning !== "boolean") fail("ScanStartResult.alreadyRunning", v);
  return true;
}

function guardStorageLocationList(dto: unknown): dto is StorageLocationDto[] {
  if (!Array.isArray(dto)) fail("StorageLocationDto[]", dto);
  for (const item of dto) {
    const v = item as Record<string, unknown>;
    if (typeof v.locationId !== "string" || v.locationId.length === 0) fail("StorageLocationDto.locationId", v);
    if (typeof v.displayName !== "string") fail("StorageLocationDto.displayName", v);
    if (!STORAGE_PROVIDERS.includes(v.providerType as string)) fail("StorageLocationDto.providerType", v);
    if (!STORAGE_STATUSES.includes(v.status as string)) fail("StorageLocationDto.status", v);
    for (const field of FORBIDDEN_STORAGE_LOCATION_FIELDS) {
      if (Object.prototype.hasOwnProperty.call(v, field)) {
        fail(`StorageLocationDto 不得包含 ${field}`, v);
      }
    }
    for (const field of Object.keys(v)) {
      if (!STORAGE_LOCATION_FIELDS.has(field)) fail(`StorageLocationDto 未知字段 ${field}`, v);
    }
  }
  return true;
}

function guardResourceList(dto: unknown): dto is ResourceListDto {
  const v = dto as Record<string, unknown>;
  if (v.schemaVersion !== 1 || !Array.isArray(v.items)) fail("ResourceListDto", v);
  for (const item of v.items) {
    const r = item as Record<string, unknown>;
    if (typeof r.resourceId !== "string" || r.resourceId.length === 0) fail("ResourceSummaryDto.resourceId", r);
    if (!RESOURCE_TYPES.includes(r.resourceType as string)) fail("ResourceSummaryDto.resourceType", r);
    if (!AVAILABILITIES.includes(r.availability as string)) fail("ResourceSummaryDto.availability", r);
    if (r.mimeType !== null && typeof r.mimeType !== "string") fail("ResourceSummaryDto.mimeType", r);
    if (r.size !== null && typeof r.size !== "number") fail("ResourceSummaryDto.size", r);
    for (const key of ["storageDisplayName", "sourceDisplayName"]) {
      if (r[key] !== null && typeof r[key] !== "string") fail(`ResourceSummaryDto.${key}`, r);
    }
    for (const key of ["isOffline", "isLocal", "requiresReauthorization"]) {
      if (typeof r[key] !== "boolean") fail(`ResourceSummaryDto.${key}`, r);
    }
  }
  return true;
}

function guardComicPageManifest(dto: unknown): dto is ComicPageManifestDto {
  if (!isComicPageManifestDto(dto)) fail("ComicPageManifestDto 严格契约", dto);
  return true;
}

function guardReaderToc(dto: unknown): dto is ReaderTocResultDto {
  if (!isReaderTocResultDto(dto)) fail("ReaderTocResultDto 严格契约", dto);
  return true;
}

function guardScanCancelResult(dto: unknown): dto is ScanCancelResultDto {
  const v = dto as Record<string, unknown>;
  if (typeof v.taskId !== "string" || v.taskId.length === 0) fail("ScanCancelResultDto.taskId", v);
  if (typeof v.alreadyTerminal !== "boolean") fail("ScanCancelResultDto.alreadyTerminal", v);
  if (!["completed", "cancelled", "failed"].includes(v.phase as string)) fail("ScanCancelResultDto.phase", v);
  if (!v.alreadyTerminal && v.phase !== "cancelled") fail("scan_cancel 受理结果必须为 cancelled", v);
  return true;
}

function guardLibraryChanged(dto: unknown): dto is LibraryChangedDto {
  const v = dto as Record<string, unknown>;
  if (v.schemaVersion !== 1) fail("LibraryChangedDto.schemaVersion", v);
  if (typeof v.at !== "string") fail("LibraryChangedDto.at", v);
  if (typeof v.operationId !== "string") fail("LibraryChangedDto.operationId", v);
  if (typeof v.sequence !== "number" || v.sequence < 1) fail("LibraryChangedDto.sequence", v);
  if (v.revision !== null && typeof v.revision !== "string") fail("LibraryChangedDto.revision", v);
  return true;
}

function guardFavoriteChanged(dto: unknown): dto is FavoriteChangedDto {
  const v = dto as Record<string, unknown>;
  if (v.schemaVersion !== 1) fail("FavoriteChangedDto.schemaVersion", v);
  if (typeof v.workId !== "string" || v.workId.length === 0) fail("FavoriteChangedDto.workId", v);
  if (typeof v.favorite !== "boolean") fail("FavoriteChangedDto.favorite", v);
  if (typeof v.operationId !== "string") fail("FavoriteChangedDto.operationId", v);
  if (typeof v.revision !== "string" || v.revision.length === 0) {
    fail("FavoriteChangedDto.revision 必须非空（与 FavoriteSetResult 同源）", v);
  }
  return true;
}

// ---- v0.2 守卫（契约 §36）----

function guardSourceRegistry(dto: unknown): dto is SourceRegistryDto {
  const v = dto as Record<string, unknown>;
  if (v.schemaVersion !== 2) fail("SourceRegistryDto.schemaVersion 必须为 2", v);
  if (!Array.isArray(v.sources)) fail("SourceRegistryDto.sources 必须是数组", v);
  for (const item of v.sources) {
    const s = item as Record<string, unknown>;
    if (typeof s.sourceId !== "string" || s.sourceId.length === 0) fail("SourceDescriptorDto.sourceId", s);
    if (typeof s.displayName !== "string") fail("SourceDescriptorDto.displayName", s);
    if (!Array.isArray(s.kinds) || !s.kinds.every((k: string) => SOURCE_KINDS.includes(k))) fail("kinds 闭合枚举", s);
    if (!Array.isArray(s.categories) || s.categories.length === 0 || !s.categories.every((category: string) => SOURCE_CATEGORIES.includes(category))) fail("categories 闭合枚举", s);
    if (typeof s.mode !== "string" || !SOURCE_MODES.includes(s.mode)) fail("mode 闭合枚举", s);
    if (typeof s.notes !== "string" || s.notes.trim().length === 0) fail("SourceDescriptorDto.notes", s);
    if ((s.notes as string).includes("目录登记") || (s.notes as string).includes("待接入")) {
      fail("内置来源不能使用目录登记或待接入状态文案", s);
    }
    if (typeof s.enabled !== "boolean") fail("SourceDescriptorDto.enabled", s);
    if (!SOURCE_HEALTHS.includes(s.health as string)) fail("SourceDescriptorDto.health 闭合枚举", s);
    if (typeof s.endpointConfigured !== "boolean") fail("SourceDescriptorDto.endpointConfigured", s);
    for (const forbidden of ["endpointUrl", "endpoint", "url", "credentialRef"]) {
      if (Object.prototype.hasOwnProperty.call(s, forbidden)) fail(`来源描述符禁止 ${forbidden}`, s);
    }
  }
  return true;
}

function guardSearchStartResult(dto: unknown): dto is SearchStartResultDto {
  const v = dto as Record<string, unknown>;
  if (typeof v.operationId !== "string" || v.operationId.length === 0) fail("SearchStartResultDto.operationId", v);
  if (typeof v.taskId !== "string") fail("SearchStartResultDto.taskId", v);
  if (typeof v.alreadyRunning !== "boolean") fail("SearchStartResultDto.alreadyRunning", v);
  return true;
}

function guardSearchEvent(dto: unknown): dto is SearchSourceEvent {
  const v = dto as Record<string, unknown>;
  if (typeof v.operationId !== "string" || v.operationId.length === 0) fail("SearchSourceEvent.operationId", v);
  if (typeof v.sequence !== "number" || v.sequence < 1) fail("SearchSourceEvent.sequence 从 1 递增", v);
  if (typeof v.at !== "string" || !v.at.includes("T")) fail("SearchSourceEvent.at RFC3339", v);
  if (!SEARCH_EVENT_KINDS.includes(v.kind as string)) fail("kind 闭合枚举", v);
  const data = v.data as Record<string, unknown> | undefined;
  if (!data || !Array.isArray(data.works)) fail("SearchSourceEventData.works 必须是数组", v);
  const kind = v.kind as string;
  const sourceIdOk = ["source_result", "warning"].includes(kind)
    ? typeof data!.sourceId === "string"
    : data!.sourceId === null;
  if (!sourceIdOk) fail(`kind=${kind} 的 sourceId 空值语义`, v);
  const codeOk = ["warning", "failed"].includes(kind)
    ? typeof data!.code === "string" && (data!.code as string).length > 0
    : data!.code === null;
  if (!codeOk) fail(`kind=${kind} 的 code 空值语义`, v);
  // V2-H 收尾批次：warning 携带安全文案 message（不含 URL/路径）；其余 null。
  const messageOk = kind === "warning"
    ? typeof data!.message === "string" && (data!.message as string).length > 0
    : data!.message === null || data!.message === undefined;
  if (!messageOk) fail(`kind=${kind} 的 message 空值语义`, v);
  return true;
}

function guardSearchCancelResult(dto: unknown): dto is SearchSourceCancelResultDto {
  const v = dto as Record<string, unknown>;
  if (typeof v.operationId !== "string" || v.operationId.length === 0) fail("operationId", v);
  if (typeof v.alreadyTerminal !== "boolean") fail("alreadyTerminal", v);
  return true;
}

function guardCredentialStatus(dto: unknown): dto is CredentialStatusDto {
  const v = dto as Record<string, unknown>;
  if (typeof v.configured !== "boolean") fail("CredentialStatusDto.configured", v);
  if (v.updatedAt !== null && typeof v.updatedAt !== "string") fail("CredentialStatusDto.updatedAt", v);
  for (const forbidden of ["secret", "credentialRef", "target"]) {
    if (Object.prototype.hasOwnProperty.call(v, forbidden)) fail(`凭据状态投影禁止 ${forbidden}`, v);
  }
  return true;
}

function guardMediaState(dto: unknown): dto is MediaStateDto {
  const v = dto as Record<string, unknown>;
  if (v.schemaVersion !== 2) fail("MediaStateDto.schemaVersion 必须为 2", v);
  if (typeof v.workId !== "string" || v.workId.length !== 36) fail("MediaStateDto.workId", v);
  if (typeof v.favorite !== "boolean") fail("MediaStateDto.favorite", v);
  if (v.rating !== null) fail("MediaStateDto.rating 在 v0.2 恒为 null（§36.9 预留位）", v);
  if (v.progress !== null) {
    const p = v.progress as Record<string, unknown>;
    if (typeof p.editionId !== "string" || typeof p.mediaItemId !== "string") fail("progress ids", p);
    if (!LOCATOR_KINDS.includes(p.locatorKind as string)) fail("locatorKind 闭合枚举", p);
    if (typeof p.updatedAt !== "string") fail("progress.updatedAt", p);
  }
  if (v.historySummary !== null) {
    const h = v.historySummary as Record<string, unknown>;
    if (typeof h.lastOpenedAt !== "string" || typeof h.openCount !== "number") fail("historySummary 形状", h);
  }
  if (typeof v.markerCount !== "number") fail("markerCount", v);
  return true;
}

function guardEnrichmentState(dto: unknown): dto is EnrichmentStateDto {
  const v = dto as Record<string, unknown>;
  if (typeof v.workId !== "string" || v.workId.length !== 36) fail("EnrichmentStateDto.workId", v);
  if (!ENRICHMENT_STATUSES.includes(v.status as string)) fail("status 闭合枚举", v);
  if (v.sourceId !== null && typeof v.sourceId !== "string") fail("sourceId", v);
  if (v.error !== null && typeof v.error !== "string") fail("error", v);
  return true;
}

function guardMetadataChanged(dto: unknown): dto is MetadataChangedDto {
  const v = dto as Record<string, unknown>;
  if (v.schemaVersion !== 1) fail("MetadataChangedDto.schemaVersion", v);
  if (typeof v.at !== "string" || !v.at.includes("T")) fail("MetadataChangedDto.at RFC3339", v);
  if (typeof v.operationId !== "string") fail("MetadataChangedDto.operationId", v);
  if (typeof v.sequence !== "number" || v.sequence < 1) fail("MetadataChangedDto.sequence", v);
  if (typeof v.workId !== "string" || v.workId.length !== 36) fail("MetadataChangedDto.workId", v);
  if (!ENRICHMENT_STATUSES.includes(v.status as string)) fail("MetadataChangedDto.status 闭合枚举", v);
  return true;
}

// ---- 装载：守卫通过后断言为 wire.ts 类型（守卫先行，杜绝裸 as）----

const check: PageDto<WorkCardDto> = guardPage(listNormal) ? listNormal : fail("listNormal", listNormal);
const checkEmpty: PageDto<WorkCardDto> = guardPage(listEmpty) ? listEmpty : fail("listEmpty", listEmpty);
const checkMissingArtwork: PageDto<WorkCardDto> = guardPage(listMissingArtwork)
  ? listMissingArtwork
  : fail("listMissingArtwork", listMissingArtwork);
const checkErr: ErrorDto = guardError(listError) ? listError : fail("listError", listError);
const checkShelves: LibraryShelvesDto = guardShelves(shelvesNormal)
  ? shelvesNormal
  : fail("shelvesNormal", shelvesNormal);
const checkFavorite: FavoriteSetRequest = guardFavoriteRequest(favoriteSet)
  ? favoriteSet
  : fail("favoriteSet", favoriteSet);
const checkFavoriteSuccess: FavoriteSetResult = guardFavoriteResult(favoriteSuccess)
  ? favoriteSuccess
  : fail("favoriteSuccess", favoriteSuccess);
const checkFavoriteFirstFalse: FavoriteSetResult = guardFavoriteResult(favoriteFirstFalse)
  ? favoriteFirstFalse
  : fail("favoriteFirstFalse", favoriteFirstFalse);
const checkFavoriteRepeated: FavoriteSetRequest = guardFavoriteRequest(favoriteRepeated)
  ? favoriteRepeated
  : fail("favoriteRepeated", favoriteRepeated);
const checkFavoriteErr: ErrorDto = guardError(favoriteError)
  ? favoriteError
  : fail("favoriteError", favoriteError);
const checkCompleted: LibraryScanEvent = guardScanEvent(scanCompleted)
  ? scanCompleted
  : fail("scanCompleted", scanCompleted);
const checkCancelled: LibraryScanEvent = guardScanEvent(scanCancelled)
  ? scanCancelled
  : fail("scanCancelled", scanCancelled);
const checkFailed: LibraryScanEvent = guardScanEvent(scanFailed)
  ? scanFailed
  : fail("scanFailed", scanFailed);
const checkWarning: LibraryScanEvent = guardScanEvent(scanWarning)
  ? scanWarning
  : fail("scanWarning", scanWarning);
const checkAlreadyRunning: ScanStartResult = guardScanStartResult(scanAlreadyRunning)
  ? scanAlreadyRunning
  : fail("scanAlreadyRunning", scanAlreadyRunning);
const checkScanCancelAccepted: ScanCancelResultDto = guardScanCancelResult(scanCancelAccepted)
  ? scanCancelAccepted
  : fail("scanCancelAccepted", scanCancelAccepted);
const checkScanCancelTerminal: ScanCancelResultDto = guardScanCancelResult(scanCancelTerminal)
  ? scanCancelTerminal
  : fail("scanCancelTerminal", scanCancelTerminal);
const checkStorageList: StorageLocationDto[] = guardStorageLocationList(storageListNormal)
  ? storageListNormal
  : fail("storageListNormal", storageListNormal);
const checkStorageListEmpty: StorageLocationDto[] = guardStorageLocationList(storageListEmpty)
  ? storageListEmpty
  : fail("storageListEmpty", storageListEmpty);
const checkLibraryChanged: LibraryChangedDto = guardLibraryChanged(libraryChanged)
  ? libraryChanged
  : fail("libraryChanged", libraryChanged);
const checkFavoriteChanged: FavoriteChangedDto = guardFavoriteChanged(favoriteChanged)
  ? favoriteChanged
  : fail("favoriteChanged", favoriteChanged);
const checkResourceList: ResourceListDto = guardResourceList(resourceMixedAvailability)
  ? resourceMixedAvailability
  : fail("resourceMixedAvailability", resourceMixedAvailability);
const checkComicPageManifest: ComicPageManifestDto = guardComicPageManifest(comicPageManifest)
  ? comicPageManifest
  : fail("comicPageManifest", comicPageManifest);
const checkComicPageManifestEmpty: ComicPageManifestDto = guardComicPageManifest(comicPageManifestEmpty)
  ? comicPageManifestEmpty
  : fail("comicPageManifestEmpty", comicPageManifestEmpty);
const checkComicPageManifestPartial: ComicPageManifestDto = guardComicPageManifest(comicPageManifestPartial)
  ? comicPageManifestPartial
  : fail("comicPageManifestPartial", comicPageManifestPartial);
const checkReaderToc: ReaderTocResultDto = guardReaderToc(readerTocNormal)
  ? readerTocNormal
  : fail("readerTocNormal", readerTocNormal);
const checkReaderTocEmpty: ReaderTocResultDto = guardReaderToc(readerTocEmpty)
  ? readerTocEmpty
  : fail("readerTocEmpty", readerTocEmpty);

// v0.2 装载（契约 §36）
const checkSourceRegistry: SourceRegistryDto = guardSourceRegistry(sourceRegistryNormal)
  ? sourceRegistryNormal
  : fail("sourceRegistryNormal", sourceRegistryNormal);
const checkSearchStart: SearchStartResultDto = guardSearchStartResult(searchStartSuccess)
  ? searchStartSuccess
  : fail("searchStartSuccess", searchStartSuccess);
const checkSearchAlreadyRunning: SearchStartResultDto = guardSearchStartResult(searchStartAlreadyRunning)
  ? searchStartAlreadyRunning
  : fail("searchStartAlreadyRunning", searchStartAlreadyRunning);
const checkSearchStarted: SearchSourceEvent = guardSearchEvent(searchSourceStarted)
  ? searchSourceStarted
  : fail("searchSourceStarted", searchSourceStarted);
const checkSearchResult: SearchSourceEvent = guardSearchEvent(searchSourceResult)
  ? searchSourceResult
  : fail("searchSourceResult", searchSourceResult);
const checkSearchWarning: SearchSourceEvent = guardSearchEvent(searchSourceWarning)
  ? searchSourceWarning
  : fail("searchSourceWarning", searchSourceWarning);
const checkSearchCompleted: SearchSourceEvent = guardSearchEvent(searchSourceCompleted)
  ? searchSourceCompleted
  : fail("searchSourceCompleted", searchSourceCompleted);
const checkSearchFailed: SearchSourceEvent = guardSearchEvent(searchSourceFailed)
  ? searchSourceFailed
  : fail("searchSourceFailed", searchSourceFailed);
const checkSearchCancelled: SearchSourceEvent = guardSearchEvent(searchSourceCancelled)
  ? searchSourceCancelled
  : fail("searchSourceCancelled", searchSourceCancelled);
const checkSearchCancelAccepted: SearchSourceCancelResultDto = guardSearchCancelResult(searchCancelAccepted)
  ? searchCancelAccepted
  : fail("searchCancelAccepted", searchCancelAccepted);
const checkSearchCancelTerminal: SearchSourceCancelResultDto = guardSearchCancelResult(searchCancelTerminal)
  ? searchCancelTerminal
  : fail("searchCancelTerminal", searchCancelTerminal);
const checkCredentialConfigured: CredentialStatusDto = guardCredentialStatus(credentialStatusConfigured)
  ? credentialStatusConfigured
  : fail("credentialStatusConfigured", credentialStatusConfigured);
const checkCredentialNotConfigured: CredentialStatusDto = guardCredentialStatus(credentialStatusNotConfigured)
  ? credentialStatusNotConfigured
  : fail("credentialStatusNotConfigured", credentialStatusNotConfigured);
const checkMediaState: MediaStateDto = guardMediaState(mediaStateNormal)
  ? mediaStateNormal
  : fail("mediaStateNormal", mediaStateNormal);
const checkEnrichmentPending: EnrichmentStateDto = guardEnrichmentState(enrichmentStatePending)
  ? enrichmentStatePending
  : fail("enrichmentStatePending", enrichmentStatePending);
const checkEnrichmentEnriched: EnrichmentStateDto = guardEnrichmentState(enrichmentStateEnriched)
  ? enrichmentStateEnriched
  : fail("enrichmentStateEnriched", enrichmentStateEnriched);
const checkMetadataChanged: MetadataChangedDto = guardMetadataChanged(metadataChangedEvent)
  ? metadataChangedEvent
  : fail("metadataChangedEvent", metadataChangedEvent);
const checkResourceRemoteStream: ResourceListDto = guardResourceList(resourceRemoteStream)
  ? resourceRemoteStream
  : fail("resourceRemoteStream", resourceRemoteStream);

// ---- 语义断言 ----

if (checkEmpty.items.length !== 0) throw new Error("empty fixture 必须为空列表");
if (checkMissingArtwork.items[0]?.posterUri !== null) throw new Error("缺图 fixture posterUri 必须 null");
if (checkErr.code !== "CURSOR_EXPIRED") throw new Error("error fixture code");
if (checkShelves.schemaVersion !== 1) throw new Error("shelves schemaVersion");
if (checkCompleted.kind !== "completed") throw new Error("completed kind");
if (checkCancelled.kind !== "cancelled") throw new Error("cancelled kind");
if (checkFailed.kind !== "failed") throw new Error("failed kind");
if (checkWarning.kind !== "warning") throw new Error("warning kind");
if (!checkAlreadyRunning.alreadyRunning) throw new Error("already-running 幂等语义");
if (checkScanCancelAccepted.alreadyTerminal || checkScanCancelAccepted.phase !== "cancelled") {
  throw new Error("scan_cancel 受理 fixture 语义");
}
if (!checkScanCancelTerminal.alreadyTerminal || checkScanCancelTerminal.phase !== "completed") {
  throw new Error("scan_cancel 终态 fixture 语义");
}
if (checkStorageList.length !== 1 || checkStorageList[0]?.status !== "connected") {
  throw new Error("storage list normal fixture 语义");
}
if (checkStorageListEmpty.length !== 0) throw new Error("storage list empty fixture 必须为空数组");
if (checkResourceList.items.length !== 2 || !checkResourceList.items[0]?.isLocal) {
  throw new Error("resource mixed availability fixture 语义");
}
if (checkComicPageManifest.pages[1]?.availability !== "ready") {
  throw new Error("comic page manifest fixture 语义");
}
if (checkComicPageManifestEmpty.pageCount !== 0 || checkComicPageManifestEmpty.pages.length !== 0) {
  throw new Error("comic empty manifest fixture 语义");
}
if (
  checkComicPageManifestPartial.pages[1]?.availability !== "unavailable"
  || checkComicPageManifestPartial.pages[1]?.contentUri !== null
  || checkComicPageManifestPartial.pages[2]?.pageIndex !== 2
) {
  throw new Error("comic partial unavailable manifest fixture 语义");
}
if (checkReaderToc.items.length !== 5) throw new Error("toc normal fixture 条目数");
if (!checkReaderToc.items.every((item) => isTocItemDto(item))) {
  throw new Error("toc normal fixture 条目形状");
}
if (checkReaderToc.items.some((item) => item.progression < 0 || item.progression > 1)) {
  throw new Error("toc progression 必须落在 0..1");
}
if (checkReaderToc.items[2]?.depth !== 1) throw new Error("toc 子级 depth 语义");
if (checkReaderTocEmpty.items.length !== 0) throw new Error("toc empty fixture 必须为空列表");
if (checkFavorite.favorite !== true) throw new Error("favorite set");
if (checkFavoriteSuccess.favorite !== true) throw new Error("favorite success");
if (!checkFavoriteSuccess.revision) throw new Error("favorite success revision");
if (checkFavoriteRepeated.workId !== checkFavoriteSuccess.workId) throw new Error("重复提交同一目标");
if (checkFavoriteErr.code !== "WORK_NOT_FOUND") throw new Error("favorite error code");
if (checkLibraryChanged.revision === null) throw new Error("library.changed 应携带 revision");
if (checkFavoriteChanged.workId.length !== 36) throw new Error("favorite.changed workId");
if (checkFavoriteChanged.revision !== checkFavoriteSuccess.revision) {
  throw new Error("favorite.changed 必须与 FavoriteSetResult 使用同一 revision（R-FAV-001）");
}

// labelHint 闭合枚举（C-04：生成物与契约一致，前端可 exhaustive render）。
const hint = check.items[0]?.primaryAction?.labelHint;
if (!hint || !LABEL_HINTS.includes(hint)) throw new Error("labelHint 闭合枚举");

// Error Catalog 机器可读（TS 端同样验证关键码存在）。
const catalog = errorCatalog as {
  codes: Array<{ code: string; retryable: boolean | string }>;
};
const codes = new Set(catalog.codes.map((c) => c.code));
for (const required of [
  "INVALID_CURSOR",
  "CURSOR_EXPIRED",
  "REVISION_CONFLICT",
  "LOCATOR_KIND_INCOMPATIBLE",
  "CREDENTIAL_ACCESS_FAILED",
  "DATABASE_ERROR",
]) {
  if (!codes.has(required)) throw new Error(`catalog 缺少 ${required}`);
}

// ---- v0.2 语义断言（契约 §36）----

if (checkSourceRegistry.sources.length < 12) throw new Error("来源注册表内置目录不完整");
for (const category of SOURCE_CATEGORIES) {
  const count = checkSourceRegistry.sources.filter((source) => source.categories.includes(category as SourceCategoryDto)).length;
  if (count < 3) throw new Error(`${category} 分类至少需要三个可搜索来源`);
}
if (checkSourceRegistry.sources.some((source) => source.sourceId === "tmdb")) {
  throw new Error("未接入凭据链路的 TMDB 不得出现在内置目录");
}
const searchableBuiltinSourceIds = new Set([
  "tvmaze",
  "bangumi",
  "anilist",
  "itunes",
  "gutenberg",
  "archive",
  "mangadex",
  "arxiv",
  "europepmc",
  "wikisource",
  "crossref",
  "openalex",
  "cms10",
  "m3u",
  "opds_gutenberg",
]);
for (const source of checkSourceRegistry.sources) {
  if (!searchableBuiltinSourceIds.has(source.sourceId)) {
    throw new Error(`内置来源缺少真实搜索 Provider：${source.sourceId}`);
  }
}
const cms10 = checkSourceRegistry.sources.find((s) => s.sourceId === "cms10");
if (!cms10 || !cms10.endpointConfigured || !cms10.kinds.includes("online_read")) {
  throw new Error("cms10 描述符必须演示端点已配置 + online_read capability");
}
if ((sourceSetRequest as { sourceId: string }).sourceId !== (sourceSetSuccess as { sourceId: string }).sourceId) {
  throw new Error("source_registry_set 请求与结果必须同 sourceId");
}
if ((sourceSetErrorUnknown as ErrorDto).code !== "INVALID_ARGUMENT") throw new Error("未知 source 错误码");

if (checkSearchStart.alreadyRunning) throw new Error("start.success 必须是新任务");
if (!checkSearchAlreadyRunning.alreadyRunning) throw new Error("already-running 幂等语义");
if (checkSearchStarted.sequence !== 1) throw new Error("started 必须是 sequence=1");
if (!(checkSearchResult.data.works.length > 0)) throw new Error("source_result 必须携带 works");
for (const work of checkSearchResult.data.works) {
  if (!work.externalIds.every((id) => EXTERNAL_ID_PROVIDERS.includes(id.provider))) {
    throw new Error("Source 候选 externalIds.provider 闭合枚举");
  }
}
if (checkSearchWarning.data.code !== "SOURCE_UNAVAILABLE") throw new Error("warning 稳定错误码");
if (checkSearchCompleted.kind !== "completed" || checkSearchFailed.kind !== "failed"
  || checkSearchCancelled.kind !== "cancelled") {
  throw new Error("search 终态种类");
}
if (checkSearchFailed.data.code !== "INTERNAL_ERROR") throw new Error("failed 携带稳定错误码");
if (checkSearchCancelAccepted.alreadyTerminal) throw new Error("cancel 受理语义");
if (!checkSearchCancelTerminal.alreadyTerminal) throw new Error("cancel 终态语义");

if (!checkCredentialConfigured.configured) throw new Error("credential configured 语义");
if (checkCredentialNotConfigured.updatedAt !== null) throw new Error("未配置凭据 updatedAt 必须 null");
const credentialSetFixture = credentialSetRequest as Record<string, unknown>;
if (credentialSetFixture.secret !== "fixture-not-a-real-secret") {
  throw new Error("fixture secret 必须是显式占位值（禁止真实凭据）");
}
if ((credentialDeleteRequest as { provider: string }).provider !== "webdav") {
  throw new Error("credential provider 闭合枚举值 webdav");
}

if (checkMediaState.rating !== null) throw new Error("v0.2 rating 预留位恒 null");
if (checkMediaState.markerCount !== 3 || checkMediaState.historySummary?.openCount !== 7) {
  throw new Error("media-state normal 聚合语义");
}
if ((enrichmentStatusRequest as { workId: string | null }).workId !== null) {
  throw new Error("enrichment_status null = 全部记录");
}
if (checkEnrichmentPending.status !== "pending" || checkEnrichmentEnriched.sourceId !== "gutenberg") {
  throw new Error("enrichment 记录语义");
}
if (checkMetadataChanged.workId !== checkEnrichmentEnriched.workId
  || checkMetadataChanged.status !== "enriched") {
  throw new Error("metadata.changed 与记录同源");
}
const streamItem = checkResourceRemoteStream.items[0];
if (streamItem.resourceType !== "remote_stream" || streamItem.streamKind !== "hls") {
  throw new Error("remote_stream 投影语义");
}
const remoteStreamJson = JSON.stringify(checkResourceRemoteStream);
for (const forbidden of ["http://", "https://", ".m3u8", "signedUrl"]) {
  if (remoteStreamJson.includes(forbidden)) throw new Error(`在线流投影禁止原始 URL 片段 ${forbidden}`);
}

// ---- Mock 行为消费证据：共享 Fixture 仅作 guard/基线，动态结果单独核对语义 ----

import { MockHavenClient } from "../src/lib/ipc/mock-client.js";
import { HavenError } from "../src/lib/ipc/errors.js";
import {
  loadedSettingsFixtureSamples,
  verifySettingsMockConsumption,
} from "./settings-fixtures-check.js";

async function verifyMockConsumption(): Promise<void> {
  const mock = new MockHavenClient();
  const page = await mock.libraryList({
    category: "all",
    mediaTypes: null,
    query: null,
    sort: "recently_added",
    cursor: null,
    limit: 50,
  });
  if (page.schemaVersion !== 1 || page.items.length !== 1) {
    throw new Error("Mock libraryList 必须返回共享 Fixture");
  }
  if (page.items[0]?.workId !== check.items[0]?.workId) {
    throw new Error("Mock libraryList 与直接 fixture 加载必须同源");
  }

  const favReq: FavoriteSetRequest = {
    workId: checkFavoriteSuccess.workId,
    favorite: true,
  };
  const favResult = await mock.favoriteSet(favReq);
  if (favResult.revision !== checkFavoriteSuccess.revision) {
    throw new Error("Mock favoriteSet 与 success fixture 必须同 revision");
  }

  // 重复 true → 同 revision（R-FAV-001 幂等收敛）
  const favRepeat = await mock.favoriteSet(favReq);
  if (favRepeat.revision !== favResult.revision) {
    throw new Error("重复收藏必须返回同一 revision");
  }

  // false → favorite=false + 新 revision（≠ 收藏版本）
  const favOff = await mock.favoriteSet({ workId: checkFavoriteSuccess.workId, favorite: false });
  if (favOff.favorite !== false || favOff.revision === favResult.revision) {
    throw new Error("取消收藏必须返回 false 与新 revision");
  }

  // 重复 false → 同 revision（版本保留）
  const favOffAgain = await mock.favoriteSet({
    workId: checkFavoriteSuccess.workId,
    favorite: false,
  });
  if (favOffAgain.revision !== favOff.revision) {
    throw new Error("重复取消必须返回同一 revision");
  }

  // false → true 重新收藏 → 新 token（≠ 首次 true 的 revision，也 ≠ false 的 revision）
  const favReOn = await mock.favoriteSet({
    workId: checkFavoriteSuccess.workId,
    favorite: true,
  });
  if (favReOn.favorite !== true) throw new Error("重新收藏必须返回 true");
  if (favReOn.revision === favResult.revision || favReOn.revision === favOff.revision) {
    throw new Error("状态变化必须生成全新 revision（不得复用历史 token）");
  }

  // 首次 false（从未收藏的 workId）→ revision=null（R-FAV-002）
  const firstFalse = await mock.favoriteSet({
    workId: "0196f0d2-0000-7000-8000-0000000000ff",
    favorite: false,
  });
  if (firstFalse.revision !== null) {
    throw new Error("首次 false 必须返回 revision=null（无版本历史）");
  }
  // 与共享 fixture set.first-false.json 同源比较（wire 形状与语义一致）
  if (
    checkFavoriteFirstFalse.favorite !== false ||
    checkFavoriteFirstFalse.revision !== null ||
    firstFalse.favorite !== checkFavoriteFirstFalse.favorite ||
    firstFalse.revision !== checkFavoriteFirstFalse.revision
  ) {
    throw new Error("first-false fixture 必须与 Mock 首次 false 结果同源");
  }

  // WORK_NOT_FOUND 错误场景经 HavenError 归一化（非 36 字符 uuid 视为不存在）
  try {
    await mock.favoriteSet({ workId: "not-a-uuid", favorite: true });
    throw new Error("Mock favoriteSet 应抛 HavenError");
  } catch (err) {
    if (!(err instanceof HavenError) || err.code !== "WORK_NOT_FOUND") {
      throw new Error("Mock 错误必须归一化为 HavenError(WORK_NOT_FOUND)");
    }
  }

  const scan = await mock.libraryScanStart({
    storageLocationId: "0196f0d2-0000-7000-8000-00000000000a",
  });
  if (!scan.alreadyRunning || scan.taskId !== checkAlreadyRunning.taskId) {
    throw new Error("Mock libraryScanStart 与 already-running fixture 必须一致");
  }

  const empty = new MockHavenClient(true);
  const emptyPage = await empty.libraryList({
    category: "all",
    mediaTypes: null,
    query: null,
    sort: "recently_added",
    cursor: null,
    limit: 50,
  });
  if (emptyPage.items.length !== 0) throw new Error("Mock 空库模式必须返回空列表");

  // ---- v0.2 Mock 行为消费（契约 §36）----

  // 来源注册表：set 后 list 必须反映启用状态（幂等设置返回同值）。
  const registryBefore = await mock.sourceRegistryList();
  if (registryBefore.schemaVersion !== 2 || registryBefore.sources.length < 4) {
    throw new Error("Mock sourceRegistryList 必须来自共享 Fixture 目录");
  }
  const setCms = await mock.sourceRegistrySet({
    sourceId: "cms10",
    enabled: true,
  });
  if (!setCms.enabled) throw new Error("source_registry_set 必须返回启用结果");
  const setCmsRepeat = await mock.sourceRegistrySet({ sourceId: "cms10", enabled: true });
  if (setCmsRepeat.enabled !== setCms.enabled) throw new Error("重复设置同值必须幂等同结果");
  const registryAfter = await mock.sourceRegistryList();
  if (registryAfter.sources.find((s) => s.sourceId === "cms10")?.enabled !== true) {
    throw new Error("source_registry_set 后 list 必须反映启用状态");
  }
  try {
    await mock.sourceRegistrySet({ sourceId: "not-a-source", enabled: true });
    throw new Error("Mock sourceRegistrySet 应抛 HavenError");
  } catch (err) {
    if (!(err instanceof HavenError) || err.code !== "INVALID_ARGUMENT") {
      throw new Error("未知来源必须归一化为 HavenError(INVALID_ARGUMENT)");
    }
  }

  // 渐进式搜索：started(seq=1) 开场 → completed 终态收束，sequence 严格递增；
  // 已启用来源产生零结果 source_result（V2-A 无参与者，不伪造命中数据）。
  const searchEvents: SearchSourceEvent[] = [];
  const searchOp = await mock.searchSourceStart(
    { query: "庆余年", category: null, limitPerSource: null },
    (event) => searchEvents.push(event),
  );
  if (searchOp.alreadyRunning) throw new Error("新查询不得合并为 alreadyRunning");
  await new Promise((resolve) => setTimeout(resolve, 0));
  if (searchEvents[0]?.kind !== "started" || searchEvents[0]?.sequence !== 1) {
    throw new Error("Mock 搜索必须以 started(seq=1) 开场");
  }
  const lastEvent = searchEvents[searchEvents.length - 1];
  if (lastEvent?.kind !== "completed") {
    throw new Error("Mock 搜索必须以 completed 终态收束");
  }
  for (let i = 1; i < searchEvents.length; i += 1) {
    if (searchEvents[i].sequence <= searchEvents[i - 1].sequence) {
      throw new Error("search.sequence 必须严格递增");
    }
  }
  // 同查询重复启动 → 幂等合并（同步二次调用，首个操作必然仍登记中）；
  // 不同查询 → 新操作。取消未知操作 → RESOURCE_NOT_FOUND。
  const pendingMergeOp = mock.searchSourceStart(
    { query: "幂等合并样例", category: null, limitPerSource: null },
    () => undefined,
  );
  const mergedOp = await mock.searchSourceStart({
    query: "幂等合并样例",
    category: null,
    limitPerSource: null,
  });
  if (!mergedOp.alreadyRunning) throw new Error("同查询活跃任务必须幂等合并");
  await pendingMergeOp;
  try {
    await mock.searchSourceCancel({ operationId: "op-search-nonexistent" });
    throw new Error("Mock searchSourceCancel 应抛 HavenError");
  } catch (err) {
    if (!(err instanceof HavenError) || err.code !== "RESOURCE_NOT_FOUND") {
      throw new Error("取消未知搜索操作必须归一化为 RESOURCE_NOT_FOUND");
    }
  }

  // 凭据：状态机 configured → delete 幂等；secret 不出现在任何响应。
  const credBefore = await mock.credentialStatus({ provider: "webdav", profileId: null });
  if (credBefore.configured || credBefore.updatedAt !== null) {
    throw new Error("未配置凭据必须诚实 not-configured 且 updatedAt=null");
  }
  await mock.credentialSet({ provider: "webdav", profileId: null, secret: "mock-only" });
  const credAfterSet = await mock.credentialStatus({ provider: "webdav", profileId: null });
  if (!credAfterSet.configured) throw new Error("credential_set 后 status 必须 configured");
  await mock.credentialDelete({ provider: "webdav", profileId: null });
  await mock.credentialDelete({ provider: "webdav", profileId: null });
  const credAfterDelete = await mock.credentialStatus({ provider: "webdav", profileId: null });
  if (credAfterDelete.configured) throw new Error("credential_delete 必须幂等清除");

  // 聚合状态：已知作品与共享 Fixture 同源；未知作品诚实空态；非法 ID → WORK_NOT_FOUND。
  const knownWorkId = check.items[0]?.workId;
  if (!knownWorkId) throw new Error("library fixture 必须提供已知 workId");
  const stateForKnown = await mock.mediaStateGet({ workId: knownWorkId });
  if (
    stateForKnown.schemaVersion !== 2 ||
    stateForKnown.rating !== null ||
    stateForKnown.workId !== checkMediaState.workId ||
    stateForKnown.markerCount !== checkMediaState.markerCount
  ) {
    throw new Error("media_state_get 已知作品必须与共享 Fixture 同源");
  }
  const emptyWorkState = await mock.mediaStateGet({
    workId: "0196f0d2-0000-7000-8000-00000000ffff",
  });
  if (emptyWorkState.progress !== null || emptyWorkState.markerCount !== 0) {
    throw new Error("未知作品聚合必须诚实空态");
  }
  try {
    await mock.mediaStateGet({ workId: "not-a-work-id" });
    throw new Error("Mock mediaStateGet 应抛 HavenError");
  } catch (err) {
    if (!(err instanceof HavenError) || err.code !== "WORK_NOT_FOUND") {
      throw new Error("非法 workId 必须归一化为 WORK_NOT_FOUND");
    }
  }

  // Enrichment：流水线落地前诚实空记录集（契约 §36.8：无记录不伪造 pending）。
  const enrichment = await mock.enrichmentStatus({ workId: null });
  if (!Array.isArray(enrichment) || enrichment.length !== 0) {
    throw new Error("enrichment_status 无流水线时必须返回 []");
  }

  // 阅读目录：reader 会话返回与演示会话同源的确定性目录；未知会话 → RESOURCE_NOT_FOUND。
  const readerSession = await mock.sessionOpen({ mediaItemId: "2", engine: "reader" });
  const readerToc = await mock.readerTocGet({ sessionId: readerSession.sessionId });
  if (
    readerToc.schemaVersion !== 1
    || readerToc.sessionId !== readerSession.sessionId
    || readerToc.items.length === 0
  ) {
    throw new Error("Mock readerTocGet 必须与演示会话同源");
  }
  try {
    await mock.readerTocGet({ sessionId: "11111111-1111-4111-8111-111111111111" });
    throw new Error("Mock readerTocGet 应抛 HavenError");
  } catch (err) {
    if (!(err instanceof HavenError) || err.code !== "RESOURCE_NOT_FOUND") {
      throw new Error("未知阅读会话必须归一化为 RESOURCE_NOT_FOUND");
    }
  }
}

await verifyMockConsumption();
await verifySettingsMockConsumption();
const loadedContractSampleCount = loadedBaseFixtureSamples.length + loadedSettingsFixtureSamples.length;
console.log(
  `fixtures-check OK: ${loadedContractSampleCount} loaded/guarded contract samples verified at runtime + Mock behavior consumption (含 Settings CAS/事件/表单状态机)`,
);

export {
  check,
  checkEmpty,
  checkMissingArtwork,
  checkErr,
  checkShelves,
  checkFavorite,
  checkFavoriteSuccess,
  checkFavoriteRepeated,
  checkFavoriteErr,
  checkCompleted,
  checkCancelled,
  checkFailed,
  checkWarning,
  checkAlreadyRunning,
  checkScanCancelAccepted,
  checkScanCancelTerminal,
  checkStorageList,
  checkStorageListEmpty,
  checkLibraryChanged,
  checkFavoriteChanged,
};
