// 来源注册表 Gateway（V2-A/V2-B：设置页来源分区唯一数据通道）。
// - 注册表/开关：source_registry_list / source_registry_set。
// - 端点：source_registry_set_endpoint——响应只含布尔投影，端点不出 IPC。

import { getHavenClient } from "@/lib/ipc/runtime";
import { toHavenError } from "@/lib/ipc/errors";
import type {
  SourceAddRequest,
  SourceAddResult,
  SourceEndpointSetRequest,
  SourceEndpointSetResult,
  SourceRegistryDto,
  SourceRegistrySetRequest,
  SourceRegistrySetResult,
  SourceRemoveRequest,
  SourceRemoveResult,
  SourceSetCredentialRequest,
  SourceUpdateRequest,
  SourceUpdateResult,
  TvboxConfigPreviewDto,
  TvboxConfigPreviewRequest,
  TvboxConfigSaveRequest,
  TvboxConfigSaveResult,
} from "@/lib/ipc/generated/wire";

export type SourceDescriptorWire = SourceRegistryDto["sources"][number];

const SOURCE_KINDS = new Set(["search", "online_read", "offline_download"]);
const SOURCE_CATEGORIES = new Set(["video", "book", "comic", "periodical"]);
const SOURCE_MODES = new Set(["single", "collection"]);
const SOURCE_HEALTHS = new Set(["unknown", "ok", "degraded", "down"]);

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function guardSourceRegistry(value: unknown): value is SourceRegistryDto {
  if (!isRecord(value) || value.schemaVersion !== 2 || !Array.isArray(value.sources)) {
    return false;
  }
  return value.sources.every((item) => {
    if (!isRecord(item)) return false;
    return (
      typeof item.sourceId === "string"
      && item.sourceId.length > 0
      && typeof item.displayName === "string"
      && Array.isArray(item.kinds)
      && item.kinds.every((kind: string) => SOURCE_KINDS.has(kind))
      && Array.isArray(item.categories)
      && item.categories.length > 0
      && item.categories.every((category: string) => SOURCE_CATEGORIES.has(category))
      && typeof item.mode === "string"
      && SOURCE_MODES.has(item.mode)
      && typeof item.notes === "string"
      && item.notes.trim().length > 0
      && typeof item.enabled === "boolean"
      && typeof item.health === "string"
      && SOURCE_HEALTHS.has(item.health)
      && typeof item.endpointConfigured === "boolean"
      && typeof item.credentialConfigured === "boolean"
    );
  });
}

/** 内置来源目录（含启用/端点配置状态）。 */
export async function listSources(): Promise<SourceRegistryDto> {
  const value: unknown = await getHavenClient().sourceRegistryList();
  if (!guardSourceRegistry(value)) throw new Error("source_registry_list 返回了非法数据");
  return value;
}

export async function setSourceEnabled(
  request: SourceRegistrySetRequest,
): Promise<SourceRegistrySetResult> {
  try {
    return await getHavenClient().sourceRegistrySet(request);
  } catch (error) {
    throw toHavenError(error);
  }
}

export async function setSourceEndpoint(
  request: SourceEndpointSetRequest,
): Promise<SourceEndpointSetResult> {
  try {
    return await getHavenClient().sourceRegistrySetEndpoint(request);
  } catch (error) {
    throw toHavenError(error);
  }
}

// ---- V2-H 收尾批次：自定义来源（OPDS 书源 + RSS/Atom 订阅源） ----

/** 自定义源 sourceId 前缀（与后端 `custom_` 前缀一致；mock 使用 `custom-`）。 */
export const CUSTOM_SOURCE_PREFIXES = ["custom_", "custom-"];

/**
 * 用户登记的 RSS/Atom 订阅源 sourceId 前缀。
 *
 * 后端用 `custom_feed_`（比 `custom_` 更长，供最长前缀路由区分），Mock 用
 * `custom-feed-`。两者的展示与编辑语义一致，但订阅源没有凭据入口。
 */
export const FEED_SOURCE_PREFIXES = ["custom_feed_", "custom-feed-"];

/**
 * 自托管漫画库 sourceId 前缀。
 *
 * 后端用 `custom_komga_` / `custom_kavita_`（都比 `custom_` 更长，供最长前缀
 * 路由区分），Mock 用 `custom-komga-` / `custom-kavita-`。它们属于 `custom_`
 * 家族，但既不是 OPDS 书源、也不是订阅源。
 */
export const KOMGA_SOURCE_PREFIXES = ["custom_komga_", "custom-komga-"];
export const KAVITA_SOURCE_PREFIXES = ["custom_kavita_", "custom-kavita-"];

export function isCustomSourceId(sourceId: string): boolean {
  return CUSTOM_SOURCE_PREFIXES.some((prefix) => sourceId.startsWith(prefix));
}

export function isFeedSourceId(sourceId: string): boolean {
  return FEED_SOURCE_PREFIXES.some((prefix) => sourceId.startsWith(prefix));
}

export function isKomgaSourceId(sourceId: string): boolean {
  return KOMGA_SOURCE_PREFIXES.some((prefix) => sourceId.startsWith(prefix));
}

export function isKavitaSourceId(sourceId: string): boolean {
  return KAVITA_SOURCE_PREFIXES.some((prefix) => sourceId.startsWith(prefix));
}

/** 自托管漫画库（Komga/Kavita）。 */
export function isComicLibrarySourceId(sourceId: string): boolean {
  return isKomgaSourceId(sourceId) || isKavitaSourceId(sourceId);
}

/**
 * 后端来源注册表生成的 TVBox / FongMi 来源 sourceId 前缀。
 *
 * 前端只识别它，不自造：`sourceId` 一律取自 `tvbox_config_save` 的响应。
 */
export const TVBOX_SOURCE_PREFIX = "custom_tvbox_";

/** Mock 侧对应前缀（与其它自定义来源常量对称；演示环境不登记 TVBox 来源）。 */
const TVBOX_MOCK_SOURCE_PREFIX = "custom-tvbox-";
const TVBOX_SOURCE_PREFIXES = [TVBOX_SOURCE_PREFIX, TVBOX_MOCK_SOURCE_PREFIX];

/**
 * 是否是 TVBox / FongMi 配置来源。
 *
 * 它同时满足 {@link isCustomSourceId}（`custom_tvbox_` 比 `custom_` 长），所以
 * "自定义来源"的调用方要先按本函数把 TVBox 排除掉，再决定是否给出启用开关。
 */
export function isTvboxSourceId(sourceId: string): boolean {
  return TVBOX_SOURCE_PREFIXES.some((prefix) => sourceId.startsWith(prefix));
}

/** `source_add` 的来源种类；与后端 `SourceAddKind` 一致，缺省为 `opds`。 */
export type SourceAddKind = "opds" | "feed" | "komga" | "kavita";

export async function addSource(request: SourceAddRequest): Promise<SourceAddResult> {
  try {
    return await getHavenClient().sourceAdd(request);
  } catch (error) {
    throw toHavenError(error);
  }
}

export async function updateSource(request: SourceUpdateRequest): Promise<SourceUpdateResult> {
  try {
    return await getHavenClient().sourceUpdate(request);
  } catch (error) {
    throw toHavenError(error);
  }
}

export async function removeSource(request: SourceRemoveRequest): Promise<SourceRemoveResult> {
  try {
    return await getHavenClient().sourceRemove(request);
  } catch (error) {
    throw toHavenError(error);
  }
}

export async function setSourceCredential(request: SourceSetCredentialRequest): Promise<void> {
  try {
    return await getHavenClient().sourceSetCredential(request);
  } catch (error) {
    throw toHavenError(error);
  }
}

/** 来源健康 → 中文标签。 */
export const SOURCE_HEALTH_LABELS: Record<string, string> = {
  unknown: "未检测",
  ok: "正常",
  degraded: "降级",
  down: "不可用",
};

// ---- Film/TV Provider 基础切片：TVBox / FongMi 配置预览 ----

/**
 * 预览响应的**精确**键集合。
 *
 * 用精确集合而不是"包含若干必需字段"：这样任何一天有人把配置地址、原始 JSON、
 * 端点、header 值或凭据加回线上类型，守卫都会立刻拒绝，而不是安静地把它们渲染出来。
 */
const TVBOX_PREVIEW_KEYS = new Set([
  "schemaVersion",
  "siteCount",
  "liveCount",
  "parserCount",
  "skippedSiteRows",
  "skippedLiveRows",
  "skippedParserRows",
  "spiderConfigured",
  "spiderKind",
  "spiderHasIntegrityDigest",
  "httpEndpointSiteCount",
  "spiderSiteCount",
  "unclassifiedSiteCount",
  "opaqueTopLevelFieldNames",
  "unrecognizedTopLevelFieldCount",
  "withheldTopLevelFieldCount",
]);

/** 第三方实现种类的闭合集合（与后端 `TvboxConfigImplementationKindDto` 一致）。 */
const TVBOX_IMPLEMENTATION_KINDS = new Set([
  "jar",
  "javascript",
  "python",
  "json_manifest",
  "unidentified",
]);

function isCount(value: unknown): value is number {
  return typeof value === "number" && Number.isInteger(value) && value >= 0;
}

/**
 * `tvbox_config_preview` 响应守卫。
 *
 * 用 `Reflect.ownKeys` 而不是 `Object.keys`：symbol 键与非枚举键同样要参与键集合
 * 比对，否则它们能绕过"精确键集合"这条不变量。
 */
export function guardTvboxConfigPreview(value: unknown): value is TvboxConfigPreviewDto {
  if (!isRecord(value)) return false;
  const keys = Reflect.ownKeys(value);
  if (keys.length !== TVBOX_PREVIEW_KEYS.size) return false;
  if (!keys.every((key) => typeof key === "string" && TVBOX_PREVIEW_KEYS.has(key))) return false;
  return (
    value.schemaVersion === 1
    && isCount(value.siteCount)
    && isCount(value.liveCount)
    && isCount(value.parserCount)
    && isCount(value.skippedSiteRows)
    && isCount(value.skippedLiveRows)
    && isCount(value.skippedParserRows)
    && typeof value.spiderConfigured === "boolean"
    && (value.spiderKind === null
      || (typeof value.spiderKind === "string" && TVBOX_IMPLEMENTATION_KINDS.has(value.spiderKind)))
    && typeof value.spiderHasIntegrityDigest === "boolean"
    && isCount(value.httpEndpointSiteCount)
    && isCount(value.spiderSiteCount)
    && isCount(value.unclassifiedSiteCount)
    && Array.isArray(value.opaqueTopLevelFieldNames)
    && value.opaqueTopLevelFieldNames.every(
      (name: unknown) => typeof name === "string" && name.length > 0,
    )
    && isCount(value.unrecognizedTopLevelFieldCount)
    && isCount(value.withheldTopLevelFieldCount)
  );
}

/**
 * 预览一个 TVBox / FongMi 配置地址。
 *
 * 只读预览：不保存、不注册来源、不写任何设置。地址只出现在请求方向上；响应里没有
 * 地址本身。形状守卫失败一律当作内部错误，不把半份摘要交给页面。
 */
export async function previewTvboxConfig(
  request: TvboxConfigPreviewRequest,
): Promise<TvboxConfigPreviewDto> {
  let value: unknown;
  try {
    value = await getHavenClient().tvboxConfigPreview(request);
  } catch (error) {
    throw toHavenError(error);
  }
  if (!guardTvboxConfigPreview(value)) {
    throw new Error("tvbox_config_preview 返回了非法数据");
  }
  return value;
}

// ---- Film/TV Provider 基础切片：TVBox / FongMi 配置保存 ----

/**
 * 保存响应的**精确**键集合。
 *
 * 与预览守卫同一约定：多一个键就判非法。响应的 `preview` 直接复用预览守卫，因此
 * 保存路径不会出现「预览挡住了、保存漏放行」的第二套脱敏规则。
 */
const TVBOX_SAVE_KEYS = new Set(["schemaVersion", "sourceId", "preview"]);

/**
 * `tvbox_config_save` 响应守卫。
 *
 * `sourceId` 必须是注册表的 TVBox 前缀 + 非空后缀；这是页面唯一能拿到的来源身份，
 * 一旦它不是这个形状，后续任何按 ID 的调用都会打到错误的来源上。
 */
export function guardTvboxConfigSaveResult(value: unknown): value is TvboxConfigSaveResult {
  if (!isRecord(value)) return false;
  const keys = Reflect.ownKeys(value);
  if (keys.length !== TVBOX_SAVE_KEYS.size) return false;
  if (!keys.every((key) => typeof key === "string" && TVBOX_SAVE_KEYS.has(key))) return false;
  const sourceId = value.sourceId;
  return (
    value.schemaVersion === 1
    && typeof sourceId === "string"
    && sourceId.length > TVBOX_SOURCE_PREFIX.length
    && sourceId.length <= 64
    && sourceId.startsWith(TVBOX_SOURCE_PREFIX)
    && guardTvboxConfigPreview(value.preview)
  );
}

/**
 * 导入一份 TVBox / FongMi 配置：取回 → 解析 → 登记来源（默认停用）→ 缓存原文。
 *
 * 只有取回与解析都成功时才会登记来源，因此失败不会留下半成品。地址只出现在请求
 * 方向上；响应只有稳定 `sourceId` 与形态摘要。新来源默认停用 —— 本版本还没有影视
 * 搜索与播放能力，页面不应把它显示成"可用"。
 */
export async function saveTvboxConfig(
  request: TvboxConfigSaveRequest,
): Promise<TvboxConfigSaveResult> {
  let value: unknown;
  try {
    value = await getHavenClient().tvboxConfigSave(request);
  } catch (error) {
    throw toHavenError(error);
  }
  if (!guardTvboxConfigSaveResult(value)) {
    throw new Error("tvbox_config_save 返回了非法数据");
  }
  return value;
}
