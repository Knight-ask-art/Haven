// 云盘（Google Drive 只读切片）Gateway：设置页「云盘」分组的唯一数据通道。
//
// 职责边界：
// - 只调用 Typed HavenClient（`getHavenClient`）；组件不得直接 invoke，也不得 fetch。
// - 每条响应在本层再过一次守卫：即使某个客户端实现漏掉了守卫，畸形载荷也不会变成
//   页面上的"已连接账户"或"可导入文件"——那正是用户会据此按下按钮的地方。
// - 只读切片：这里没有 upload / sync / refresh 方法。某个账户"部分可用"只意味着
//   能浏览与导入 PDF，不意味着可以写回远端、也不意味着存在双向同步。
// - 页面拿不到 Drive 文件 / 目录 ID、邮箱、token 或凭据：类型里就没有这些位置，
//   守卫还会把多出来的这类字段整体判为非法。

import {
  guardCloudAccount,
  guardCloudBrowsePage,
  guardCloudConnectAttempt,
  guardCloudConnectPoll,
  guardCloudConnectStatus,
  guardCloudFolderBinding,
  guardCloudFolderRemoved,
  guardCloudLocationId,
  guardCloudObject,
  guardCloudStorageList,
} from "@/lib/ipc/cloud-storage-client"
import type {
  CloudAccountDisconnectRequestWire,
  CloudAccountWire,
  CloudBrowsePageWire,
  CloudConnectAttemptWire,
  CloudConnectBeginRequestWire,
  CloudConnectPollWire,
  CloudConnectStatusWire,
  CloudFolderWire,
  CloudImportPdfRequestWire,
  CloudObjectWire,
  CloudRegisterFolderRequestWire,
  CloudStorageListWire,
} from "@/lib/ipc/cloud-storage-client"
import { toHavenError } from "@/lib/ipc/errors"
import { getHavenClient } from "@/lib/ipc/runtime"

const INVALID_RESPONSE = "云盘接口返回了非法数据"

function invalidResponse(): never {
  throw toHavenError({ code: "INTERNAL_ERROR", userMessage: INVALID_RESPONSE, retryable: false })
}

/** 取数 → 守卫 → 错误归一：调用点只会看到 HavenError 或一份被验证过的投影。 */
async function call<T>(
  invoke: () => Promise<unknown>,
  guard: (value: unknown) => value is T,
): Promise<T> {
  try {
    const value: unknown = await invoke()
    if (!guard(value)) invalidResponse()
    return value
  } catch (error) {
    throw toHavenError(error)
  }
}

/**
 * 本切片的能力面：只有只读浏览与 PDF 导入。
 *
 * 把 upload / bidirectionalSync / backgroundRefresh 显式写成 false，是为了让"云盘可用"
 * 在代码里也无法被读成"可以上传或同步"——后端当前没有这些能力，前端也不预留入口。
 */
export const CLOUD_STORAGE_SLICE_CAPABILITIES = Object.freeze({
  browse: true,
  importPdf: true,
  upload: false,
  bidirectionalSync: false,
  backgroundRefresh: false,
} as const)

/** 本机授权可用性 + 全部账户（含断开墓碑）。 */
export function fetchCloudStorageList(): Promise<CloudStorageListWire> {
  return call(() => getHavenClient().cloudStorageList(), guardCloudStorageList)
}

/** 发起一次授权（`accountId` 省略即新连接）。返回的只是本机尝试句柄。 */
export function beginCloudAccountConnect(
  request: CloudConnectBeginRequestWire,
): Promise<CloudConnectAttemptWire> {
  return call(() => getHavenClient().cloudAccountConnectBegin(request), guardCloudConnectAttempt)
}

/** 只读轮询一次授权尝试。 */
export function pollCloudAccountConnect(attemptId: string): Promise<CloudConnectPollWire> {
  return call(() => getHavenClient().cloudAccountConnectPoll(attemptId), guardCloudConnectPoll)
}

/** 完成授权（恰好一次）；返回值是账户投影，不含 Provider 身份。 */
export function completeCloudAccountConnect(attemptId: string): Promise<CloudAccountWire> {
  return call(() => getHavenClient().cloudAccountConnectComplete(attemptId), guardCloudAccount)
}

/** 取消授权尝试；返回值如实反映终态，不折叠成同一个"已取消"。 */
export function cancelCloudAccountConnect(attemptId: string): Promise<CloudConnectStatusWire> {
  return call(() => getHavenClient().cloudAccountConnectCancel(attemptId), guardCloudConnectStatus)
}

/** 断开账户（CAS：代际不符即失败，不静默覆盖）。 */
export function disconnectCloudAccount(
  request: CloudAccountDisconnectRequestWire,
): Promise<CloudAccountWire> {
  return call(() => getHavenClient().cloudAccountDisconnect(request), guardCloudAccount)
}

/** 读取一个已登记目录的绑定投影；未登记 → null。 */
export function fetchCloudFolderBinding(locationId: string): Promise<CloudFolderWire | null> {
  return call(() => getHavenClient().cloudFolderBindingGet(locationId), guardCloudFolderBinding)
}

/** 移除目录绑定；只删应用内索引与统一内容，不触碰远端文件与账户凭据。 */
export function removeCloudFolder(locationId: string): Promise<boolean> {
  return call(() => getHavenClient().cloudFolderRemove(locationId), guardCloudFolderRemoved)
}

/** 列出账户根目录（不透明句柄 + 有界分页）。 */
export function browseCloudRoot(accountId: string): Promise<CloudBrowsePageWire> {
  return call(() => getHavenClient().cloudBrowseRoot(accountId), guardCloudBrowsePage)
}

/** 列出句柄指向的目录。 */
export function browseCloudFolder(folderHandle: string): Promise<CloudBrowsePageWire> {
  return call(() => getHavenClient().cloudBrowseFolder(folderHandle), guardCloudBrowsePage)
}

/** 翻页：光标句柄一次性消费。 */
export function browseCloudNextPage(cursor: string): Promise<CloudBrowsePageWire> {
  return call(() => getHavenClient().cloudBrowseNextPage(cursor), guardCloudBrowsePage)
}

/** 列出已登记目录（内部位置 UUID）。 */
export function browseCloudLocation(locationId: string): Promise<CloudBrowsePageWire> {
  return call(() => getHavenClient().cloudBrowseLocation(locationId), guardCloudBrowsePage)
}

/** 登记目录：返回内部位置 UUID（再用 `fetchCloudFolderBinding` 读回投影）。 */
export function registerCloudFolder(request: CloudRegisterFolderRequestWire): Promise<string> {
  return call(() => getHavenClient().cloudRegisterFolder(request), guardCloudLocationId)
}

/** 导入目录下的一份 PDF：返回对象 / 媒体条目 / 资源三个内部 UUID。 */
export function importCloudPdf(request: CloudImportPdfRequestWire): Promise<CloudObjectWire> {
  return call(() => getHavenClient().cloudImportPdf(request), guardCloudObject)
}

/** 设置 feature 内部用例门面；全部委托上面的 typed 绑定。 */
export const cloudStorageGateway = {
  list: fetchCloudStorageList,
  begin: beginCloudAccountConnect,
  poll: pollCloudAccountConnect,
  complete: completeCloudAccountConnect,
  cancel: cancelCloudAccountConnect,
  disconnect: disconnectCloudAccount,
  folderBinding: fetchCloudFolderBinding,
  removeFolder: removeCloudFolder,
  root: browseCloudRoot,
  folder: browseCloudFolder,
  next: browseCloudNextPage,
  location: browseCloudLocation,
  register: registerCloudFolder,
  importPdf: importCloudPdf,
}
