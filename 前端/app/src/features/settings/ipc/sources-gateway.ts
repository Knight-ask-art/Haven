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

// ---- V2-H 收尾批次：自定义 OPDS 书源 ----

/** 自定义源 sourceId 前缀（与后端 `custom_` 前缀一致；mock 使用 `custom-`）。 */
export const CUSTOM_SOURCE_PREFIXES = ["custom_", "custom-"];

export function isCustomSourceId(sourceId: string): boolean {
  return CUSTOM_SOURCE_PREFIXES.some((prefix) => sourceId.startsWith(prefix));
}

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
