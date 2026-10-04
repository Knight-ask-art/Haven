// 云盘（Google Drive 只读切片）的 Typed IPC 消费层。
//
// 线上类型直接导入 Rust 生成的 Wire；运行时守卫拒绝形状漂移。
// 边界（与 Rust wire 注释逐条对应）：
// - 线上类型只承载内部 UUID 与不透明句柄，字段集合精确闭合（Rust 侧 deny_unknown_fields）。
//   driveId / fileId / folderId / pageToken / email / accessToken / refreshToken /
//   credentialRef 这类键在任何云盘响应里都没有合法位置，出现即整体拒绝。
// - 所有句柄（目录 / 文件 / 分页光标 / 授权尝试）都是不透明 UUID（对应 Rust 的
//   `is_opaque_handle`）：前端可以持有、回传、过期，但无法解释、拼接或推断远端身份；
//   页面永远拿不到可展示的 Drive ID。
// - 本切片只读：列表 / 浏览 / 登记目录 / 导入 PDF。没有上传、没有双向同步、没有后台刷新，
//   也没有远端删除——"云盘部分可用"不等于"可以写回远端"。
// - 本层是唯一持有传输函数的地方；形状漂移一律 INTERNAL_ERROR，不透传半份数据。

import { HavenError, toHavenError } from "./errors.js"

import type {
  CloudConnectStatusDto as CloudConnectStatusWire,
  CloudAccountDto as CloudAccountWire,
  CloudConnectAttemptDto as CloudConnectAttemptWire,
  CloudConnectPollDto as CloudConnectPollWire,
  CloudFolderDto as CloudFolderWire,
  CloudBrowseEntryDto as CloudBrowseEntryWire,
  CloudBrowsePageDto as CloudBrowsePageWire,
  CloudObjectDto as CloudObjectWire,
  CloudStorageListDto as CloudStorageListWire,
  CloudConnectBeginRequest as CloudConnectBeginRequestWire,
  CloudAccountDisconnectRequest as CloudAccountDisconnectRequestWire,
  CloudRegisterFolderRequest as CloudRegisterFolderRequestWire,
  CloudImportPdfRequest as CloudImportPdfRequestWire,
} from "./generated/wire"
export type {
  CloudConnectStatusDto as CloudConnectStatusWire,
  CloudAccountDto as CloudAccountWire,
  CloudConnectAttemptDto as CloudConnectAttemptWire,
  CloudConnectPollDto as CloudConnectPollWire,
  CloudFolderDto as CloudFolderWire,
  CloudBrowseEntryDto as CloudBrowseEntryWire,
  CloudBrowsePageDto as CloudBrowsePageWire,
  CloudObjectDto as CloudObjectWire,
  CloudStorageListDto as CloudStorageListWire,
  CloudConnectBeginRequest as CloudConnectBeginRequestWire,
  CloudAccountDisconnectRequest as CloudAccountDisconnectRequestWire,
  CloudRegisterFolderRequest as CloudRegisterFolderRequestWire,
  CloudImportPdfRequest as CloudImportPdfRequestWire,
} from "./generated/wire"

const CANONICAL_UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
const CONTROL_CHARACTER_PATTERN = /[\u0000-\u001f\u007f]/

/** 与 Rust `CLOUD_BROWSE_MAX_PAGE_ENTRIES` 对齐的单页条目上限。 */
const MAX_BROWSE_ENTRIES = 1_000
/** 与 Application MAX_CLOUD_ACCOUNTS 对齐。 */
const MAX_ACCOUNTS = 8
/** 展示文本上限；Rust 命令层对用户输入另有 ≤120 字符的更严规则。 */
const MAX_DISPLAY_NAME_CHARS = 512

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

/**
 * 精确字段守卫：键集合必须与 Rust DTO 逐字相同。
 * `Reflect.ownKeys` 而不是 `Object.keys`——Symbol 键与不可枚举的自有键同样是越界字段。
 */
function isClosedRecord(value: unknown, fields: readonly string[]): value is Record<string, unknown> {
  if (!isRecord(value)) return false
  const keys = Reflect.ownKeys(value)
  const permitted = new Set<string>(fields)
  if (keys.length !== permitted.size) return false
  for (const key of keys) {
    if (typeof key !== "string") return false
    if (!permitted.has(key)) return false
  }
  return true
}

function isCanonicalUuid(value: unknown): value is string {
  return typeof value === "string" && CANONICAL_UUID_PATTERN.test(value)
}

function isNullableCanonicalUuid(value: unknown): value is string | null {
  return value === null || isCanonicalUuid(value)
}

/** 展示文本：非空、有长度上限、无控制字符。 */
function isDisplayName(value: unknown): value is string {
  return typeof value === "string"
    && value.trim().length > 0
    && Array.from(value).length <= MAX_DISPLAY_NAME_CHARS
    && !CONTROL_CHARACTER_PATTERN.test(value)
}

/** wire 上的 i64 / u64 一律是 number：拒绝 NaN / Infinity / 小数 / 越界。 */
function isSafeInteger(value: unknown, min: number, max: number): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= min && value <= max
}

/** u64 字节数：非负安全整数。 */
function isSizeCount(value: unknown): value is number {
  return isSafeInteger(value, 0, Number.MAX_SAFE_INTEGER)
}

function isNullableSizeCount(value: unknown): value is number | null {
  return value === null || isSizeCount(value)
}

/** 毫秒时刻：必须是正的安全整数（0 或负数不是真实时刻）。 */
function isTimestampMs(value: unknown): value is number {
  return isSafeInteger(value, 1, Number.MAX_SAFE_INTEGER)
}

/** 账户代际：正的安全整数，用作 CAS 凭据。 */
function isGeneration(value: unknown): value is number {
  return isSafeInteger(value, 1, Number.MAX_SAFE_INTEGER)
}

/** 授权尝试状态的闭合集合：近似词、大小写变体、未知值都不接受。 */
export function guardCloudConnectStatus(value: unknown): value is CloudConnectStatusWire {
  return typeof value === "string"
    && (["pending", "completing", "authorized", "cancelled", "expired", "failed", "completed"] as readonly string[]).includes(value)
}

export function guardCloudAccount(value: unknown): value is CloudAccountWire {
  if (!isClosedRecord(value, ["id", "displayName", "connected", "generation"])) return false
  return isCanonicalUuid(value.id)
    && isDisplayName(value.displayName)
    && typeof value.connected === "boolean"
    && isGeneration(value.generation)
}

export function guardCloudConnectAttempt(value: unknown): value is CloudConnectAttemptWire {
  if (!isClosedRecord(value, ["attemptId", "expiresAtMs"])) return false
  return isCanonicalUuid(value.attemptId) && isTimestampMs(value.expiresAtMs)
}

export function guardCloudConnectPoll(value: unknown): value is CloudConnectPollWire {
  if (!isClosedRecord(value, ["status", "expiresAtMs"])) return false
  return guardCloudConnectStatus(value.status) && isTimestampMs(value.expiresAtMs)
}

export function guardCloudFolder(value: unknown): value is CloudFolderWire {
  if (!isClosedRecord(value, ["locationId", "accountId", "createdAtMs"])) return false
  return isCanonicalUuid(value.locationId)
    && isCanonicalUuid(value.accountId)
    && isTimestampMs(value.createdAtMs)
}

/** 未登记目录的合法答案是 `null`，不是"空对象"或"未知"。 */
export function guardCloudFolderBinding(value: unknown): value is CloudFolderWire | null {
  return value === null || guardCloudFolder(value)
}

/** 移除绑定返回布尔事实；true 表示确实删掉了一条绑定。 */
export function guardCloudFolderRemoved(value: unknown): value is boolean {
  return typeof value === "boolean"
}

export function guardCloudBrowseEntry(value: unknown): value is CloudBrowseEntryWire {
  if (!isClosedRecord(value, ["handle", "displayName", "isFolder", "pdfSupported", "sizeBytes"])) {
    return false
  }
  if (!isNullableCanonicalUuid(value.handle)) return false
  if (!isDisplayName(value.displayName)) return false
  if (typeof value.isFolder !== "boolean" || typeof value.pdfSupported !== "boolean") return false
  if (!isNullableSizeCount(value.sizeBytes)) return false
  // 跨字段事实（对应 Rust `project_entry` 的三条投影分支）：
  // 文件夹永远不支持 PDF；有句柄的条目一定可浏览或可导入，二者都不是就是自相矛盾。
  if (value.pdfSupported && (value.isFolder || value.handle === null || !isSafeInteger(value.sizeBytes, 1, 128 * 1024 * 1024))) return false
  if (value.handle !== null && !value.isFolder && !value.pdfSupported) return false
  return true
}

export function guardCloudBrowsePage(value: unknown): value is CloudBrowsePageWire {
  if (!isClosedRecord(value, ["folderHandle", "entries", "nextCursor"])) return false
  if (!isNullableCanonicalUuid(value.folderHandle)) return false
  if (!isNullableCanonicalUuid(value.nextCursor)) return false
  const entries = value.entries
  if (!Array.isArray(entries) || entries.length > MAX_BROWSE_ENTRIES) return false
  const handles = new Set<string>()
  return entries.every((entry: unknown) => {
    if (!guardCloudBrowseEntry(entry)) return false
    if (entry.handle === null) return true
    // 同页重复句柄说明这一页被拼接或重复展开：渲染键与后续导入都会指错对象。
    if (handles.has(entry.handle)) return false
    handles.add(entry.handle)
    return true
  })
}

export function guardCloudObject(value: unknown): value is CloudObjectWire {
  if (!isClosedRecord(value, ["objectId", "mediaItemId", "resourceId", "displayName", "sizeBytes"])) {
    return false
  }
  return isCanonicalUuid(value.objectId)
    && isCanonicalUuid(value.mediaItemId)
    && isCanonicalUuid(value.resourceId)
    && isDisplayName(value.displayName)
    && isSafeInteger(value.sizeBytes, 1, 128 * 1024 * 1024)
}

export function guardCloudStorageList(value: unknown): value is CloudStorageListWire {
  if (!isClosedRecord(value, ["oauthAvailable", "accounts"])) return false
  if (typeof value.oauthAvailable !== "boolean") return false
  const accounts = value.accounts
  if (!Array.isArray(accounts) || accounts.length > MAX_ACCOUNTS) return false
  const ids = new Set<string>()
  return accounts.every((account: unknown) => {
    if (!guardCloudAccount(account)) return false
    if (ids.has(account.id)) return false
    ids.add(account.id)
    return true
  })
}

/** 登记目录的返回值：裸的内部位置 UUID，不是 URL、句柄或路径。 */
export function guardCloudLocationId(value: unknown): value is string {
  return isCanonicalUuid(value)
}

/**
 * 云盘 Typed Client：14 条命令的闭合接口。
 * 由 `HavenClient` 继承（Tauri 环境走真实 invoke，Mock 环境显式失败）。
 */
export interface CloudStorageClient {
  /** 本机授权可用性 + 全部账户（含断开墓碑）。 */
  cloudStorageList(): Promise<CloudStorageListWire>
  cloudAccountConnectBegin(request: CloudConnectBeginRequestWire): Promise<CloudConnectAttemptWire>
  cloudAccountConnectPoll(attemptId: string): Promise<CloudConnectPollWire>
  cloudAccountConnectComplete(attemptId: string): Promise<CloudAccountWire>
  cloudAccountConnectCancel(attemptId: string): Promise<CloudConnectStatusWire>
  cloudAccountDisconnect(request: CloudAccountDisconnectRequestWire): Promise<CloudAccountWire>
  cloudFolderBindingGet(locationId: string): Promise<CloudFolderWire | null>
  cloudFolderRemove(locationId: string): Promise<boolean>
  cloudBrowseRoot(accountId: string): Promise<CloudBrowsePageWire>
  cloudBrowseFolder(folderHandle: string): Promise<CloudBrowsePageWire>
  cloudBrowseNextPage(cursor: string): Promise<CloudBrowsePageWire>
  cloudBrowseLocation(locationId: string): Promise<CloudBrowsePageWire>
  /** 返回内部位置 UUID（裸 UUID，不是 URL / 句柄 / 路径）。 */
  cloudRegisterFolder(request: CloudRegisterFolderRequestWire): Promise<string>
  cloudImportPdf(request: CloudImportPdfRequestWire): Promise<CloudObjectWire>
}

/** 传输函数：只接收命令名与 camelCase 参数，返回未知形状，由本层守卫收敛。 */
export type CloudStorageTransport = (
  command: string,
  args?: Record<string, unknown>,
) => Promise<unknown>

function invalidResponse(): never {
  throw new HavenError({
    code: "INTERNAL_ERROR",
    userMessage: "云盘接口返回了非法数据",
    retryable: false,
  })
}

/**
 * 调用 transport → 运行时守卫 → 归一化错误。
 * 守卫失败与 transport 拒绝都以 HavenError 抛出（前者 INTERNAL_ERROR），因此调用点
 * 永远只看到 HavenError，不会拿到半份载荷或裸 Error。
 */
async function readCloudResponse<T>(
  transport: CloudStorageTransport,
  command: string,
  args: Record<string, unknown> | undefined,
  guard: (value: unknown) => value is T,
): Promise<T> {
  try {
    const value: unknown = await transport(command, args)
    if (!guard(value)) invalidResponse()
    return value
  } catch (error) {
    throw toHavenError(error)
  }
}

/**
 * 共享实现：唯一持有 invoke/transport 的地方，`TauriHavenClient` 只做委托。
 * 每条命令都是闭合的 typed 调用，没有 `invoke(name, arbitraryArgs)` 这类自由分发入口，
 * 也没有任何 token、Provider 身份或路径字段参与请求。
 */
export function createCloudStorageIpc(transport: CloudStorageTransport): CloudStorageClient {
  return {
    cloudStorageList: () =>
      readCloudResponse(transport, "cloud_storage_list", undefined, guardCloudStorageList),

    // accountId 省略或 null 都是"新连接"：显式传 null，让 Rust 侧的 Option 收到确定的空值。
    cloudAccountConnectBegin: (request) =>
      readCloudResponse(
        transport,
        "cloud_account_connect_begin",
        { request: { accountId: request.accountId ?? null } },
        guardCloudConnectAttempt,
      ),

    cloudAccountConnectPoll: (attemptId) =>
      readCloudResponse(transport, "cloud_account_connect_poll", { attemptId }, guardCloudConnectPoll),

    cloudAccountConnectComplete: (attemptId) =>
      readCloudResponse(transport, "cloud_account_connect_complete", { attemptId }, guardCloudAccount),

    cloudAccountConnectCancel: (attemptId) =>
      readCloudResponse(transport, "cloud_account_connect_cancel", { attemptId }, guardCloudConnectStatus),

    cloudAccountDisconnect: (request) =>
      readCloudResponse(
        transport,
        "cloud_account_disconnect",
        { request: { accountId: request.accountId, expectedGeneration: request.expectedGeneration } },
        guardCloudAccount,
      ),

    cloudFolderBindingGet: (locationId) =>
      readCloudResponse(transport, "cloud_folder_binding_get", { locationId }, guardCloudFolderBinding),

    cloudFolderRemove: (locationId) =>
      readCloudResponse(transport, "cloud_folder_remove", { locationId }, guardCloudFolderRemoved),

    cloudBrowseRoot: (accountId) =>
      readCloudResponse(transport, "cloud_browse_root", { accountId }, guardCloudBrowsePage),

    cloudBrowseFolder: (folderHandle) =>
      readCloudResponse(transport, "cloud_browse_folder", { folderHandle }, guardCloudBrowsePage),

    cloudBrowseNextPage: (cursor) =>
      readCloudResponse(transport, "cloud_browse_next_page", { cursor }, guardCloudBrowsePage),

    cloudBrowseLocation: (locationId) =>
      readCloudResponse(transport, "cloud_browse_location", { locationId }, guardCloudBrowsePage),

    cloudRegisterFolder: (request) =>
      readCloudResponse(
        transport,
        "cloud_register_folder",
        { request: { folderHandle: request.folderHandle, displayName: request.displayName } },
        guardCloudLocationId,
      ),

    cloudImportPdf: (request) =>
      readCloudResponse(
        transport,
        "cloud_import_pdf",
        { request: { locationId: request.locationId, fileHandle: request.fileHandle } },
        guardCloudObject,
      ),
  }
}
