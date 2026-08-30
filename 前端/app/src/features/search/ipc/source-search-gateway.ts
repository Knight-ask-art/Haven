// 来源搜索 Gateway（V2-B 实战批次）：渐进式来源搜索候选 + 导入。
// - 搜索：search_source_start（Channel）+ search_source_cancel（幂等）。
// - 导入：source_work_import（幂等；返回真实 Work/MediaItem 身份）。

import { getHavenClient } from "@/lib/ipc/runtime";
import { toHavenError } from "@/lib/ipc/errors";
import type {
  QueryCategory,
  SearchSourceEvent,
  SearchStartResultDto,
  SourceWorkImportRequest,
  SourceWorkImportResult,
} from "@/lib/ipc/generated/wire";

/** 发起来源搜索；事件经 Channel 回调逐条推送。category 为后端参与者分类门控。 */
export async function startSourceSearch(
  query: string,
  onEvent: (event: SearchSourceEvent) => void,
  category?: QueryCategory,
): Promise<SearchStartResultDto> {
  return getHavenClient().searchSourceStart(
    { query, category: category ?? null, limitPerSource: null },
    onEvent,
  );
}

/** 取消来源搜索（幂等）。 */
export async function cancelSourceSearch(operationId: string): Promise<void> {
  try {
    await getHavenClient().searchSourceCancel({ operationId });
  } catch (error) {
    throw toHavenError(error);
  }
}

/** 导入候选到媒体库（幂等）；返回真实身份供路由跳转。 */
export async function importSourceWork(
  request: SourceWorkImportRequest,
): Promise<SourceWorkImportResult> {
  try {
    return await getHavenClient().sourceWorkImport(request);
  } catch (error) {
    throw toHavenError(error);
  }
}
