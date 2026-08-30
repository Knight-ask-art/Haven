// Storage/Scan Gateway（SLICE-SCAN-001 + INTEGRATION-SLICE-001：设置页存储分组
// 的唯一数据通道，禁止散落 invoke）。
// - Tauri WebView：真实 storage_location_* 命令 + library_scan_start（Channel）。
// - 浏览器 dev：演示环境——列表面板为空、选择目录/扫描抛出可提示错误（UI 转 notice）。

import { invoke } from "@tauri-apps/api/core";

import { isTauriRuntime, getHavenClient } from "@/lib/ipc/runtime";
import { toHavenError } from "@/lib/ipc/errors";
import type {
  LibraryScanEvent,
  ScanCancelResultDto,
  ScanStartResult,
  StorageLocationDto,
} from "@/lib/ipc/generated/wire";

export type StorageLocationWire = StorageLocationDto;
export type ScanCancelResultWire = ScanCancelResultDto;

const PROVIDER_TYPES = new Set(["local", "web_dav", "one_drive", "google_drive"]);
const STORAGE_STATUSES = new Set([
  "connected",
  "disconnected",
  "auth_expired",
  "unavailable",
  "read_only",
  "error",
  "disabled",
  "missing",
]);
const CANCEL_TERMINAL_PHASES = new Set(["completed", "cancelled", "failed"]);
const STORAGE_LOCATION_FIELDS = new Set(["locationId", "displayName", "providerType", "status"]);
const FORBIDDEN_STORAGE_LOCATION_FIELDS = [
  "rootPath",
  "rootRef",
  "root_ref",
  "credentialRef",
  "credential_ref",
];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function guardStorageLocation(value: unknown): value is StorageLocationDto {
  if (!isRecord(value)) return false;
  if (FORBIDDEN_STORAGE_LOCATION_FIELDS.some((field) => Object.prototype.hasOwnProperty.call(value, field))) {
    return false;
  }
  if (Object.keys(value).some((field) => !STORAGE_LOCATION_FIELDS.has(field))) return false;
  return typeof value.locationId === "string"
    && value.locationId.length > 0
    && typeof value.displayName === "string"
    && typeof value.providerType === "string"
    && PROVIDER_TYPES.has(value.providerType)
    && typeof value.status === "string"
    && STORAGE_STATUSES.has(value.status);
}

export function guardStorageLocationList(value: unknown): value is StorageLocationDto[] {
  return Array.isArray(value) && value.every(guardStorageLocation);
}

export function guardScanCancelResult(value: unknown): value is ScanCancelResultDto {
  if (!isRecord(value)
    || typeof value.taskId !== "string"
    || value.taskId.length === 0
    || typeof value.alreadyTerminal !== "boolean"
    || typeof value.phase !== "string"
    || !CANCEL_TERMINAL_PHASES.has(value.phase)) {
    return false;
  }
  return value.alreadyTerminal || value.phase === "cancelled";
}

function demoUnsupported(action: string): never {
  throw new Error(`浏览器演示环境不支持${action}，请在栖阅应用内操作`);
}

/** 已注册的本地媒体库位置列表。 */
export async function listStorageLocations(): Promise<StorageLocationWire[]> {
  if (!isTauriRuntime()) return [];
  try {
    const value: unknown = await invoke("storage_location_list");
    if (!guardStorageLocationList(value)) throw new Error("storage_location_list 返回了非法数据");
    return value;
  } catch (error) {
    throw toHavenError(error);
  }
}

/** 唤起系统目录选择对话框并注册本地媒体库（路径不经 WebView，P0-1）。
 * displayName 传空串：后端以所选目录名兜底；返回新位置的 opaque ID
 * （UUID 字符串），完整行数据由 listStorageLocations 拉取。 */
export async function pickLocalDirectory(): Promise<string> {
  if (!isTauriRuntime()) demoUnsupported("选择本地目录");
  try {
    return await invoke<string>("storage_location_pick_local_directory", {
      displayName: "",
    });
  } catch (error) {
    throw toHavenError(error);
  }
}

/** 重新绑定：唤起原生对话框为既有位置选择新目录（路径不经 WebView，P0-1）。 */
export async function rebindLocalDirectory(locationId: string): Promise<void> {
  if (!isTauriRuntime()) demoUnsupported("重新绑定目录");
  try {
    await invoke<void>("storage_location_rebind_local_directory", {
      storageLocationId: locationId,
    });
  } catch (error) {
    throw toHavenError(error);
  }
}

/** 移除位置：同一事务删除该位置的资源索引、**仅由其派生的内容链**与关联
 * 用户状态（选错目录可整体撤销）；共享内容保留；原始文件不受影响。 */
export async function removeStorageLocation(locationId: string): Promise<void> {
  if (!isTauriRuntime()) demoUnsupported("移除媒体库位置");
  try {
    await invoke<void>("storage_location_remove", { storageLocationId: locationId });
  } catch (error) {
    throw toHavenError(error);
  }
}

/** 触发扫描：Channel 逐事件回调（限频由后端负责），立即返回 ScanStartResult。 */
export async function startLibraryScan(
  locationId: string,
  onEvent: (event: LibraryScanEvent) => void,
): Promise<ScanStartResult> {
  if (!isTauriRuntime()) demoUnsupported("扫描媒体库");
  return getHavenClient().libraryScanStart({ storageLocationId: locationId }, onEvent);
}

/** 协作式取消（幂等；已结束任务返回真实终态）。 */
export async function cancelScan(taskId: string): Promise<ScanCancelResultWire> {
  if (!isTauriRuntime()) demoUnsupported("取消扫描");
  try {
    const value: unknown = await invoke("scan_cancel", { taskId });
    if (!guardScanCancelResult(value)) throw new Error("scan_cancel 返回了非法数据");
    return value;
  } catch (error) {
    throw toHavenError(error);
  }
}

/** ScanPhase → 中文标签（进度行展示用）。 */
export const SCAN_PHASE_LABELS: Record<string, string> = {
  started: "已启动",
  enumerating: "枚举目录",
  detecting: "识别格式",
  fingerprinting: "计算指纹",
  indexing: "写入索引",
  item_indexed: "索引进度",
  warning: "警告",
  completed: "扫描完成",
  cancelled: "已取消",
  failed: "扫描失败",
}
