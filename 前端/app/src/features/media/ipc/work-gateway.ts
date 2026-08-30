// Work Gateway：详情页首屏 Header 的唯一数据通道。
// 页面决定何时使用权威 IPC；此 gateway 始终调用 work_get，不回退 library_list。

import { getHavenClient } from "@/lib/ipc/runtime"
import type { WorkDetailHeaderDto } from "@/lib/ipc/generated/wire"

/** 获取 Work Detail Header；调用方按运行环境决定是否使用该权威数据。 */
export function getWorkDetail(workId: string): Promise<WorkDetailHeaderDto> {
  return getHavenClient().workGet({ workId })
}
