// 来源搜索 Gateway（V2-B 实战批次）：渐进式来源搜索候选 + 导入。
// - 搜索：search_source_start（Channel）+ search_source_cancel（幂等）。
// - 导入：source_work_import（幂等；返回真实 Work/MediaItem 身份）。
//
// 启动/事件握手（来源索引安全边界）：search_source_start 的 Channel 事件可能
// 早于启动返回值到达；启动返回 false 时，缓冲事件在拿到权威 operationId 后
// 按 sequence 释放，之后只转发同一 operationId 的事件。启动返回 true
// （alreadyRunning：新 Channel 接入一个已有历史的操作，后端不重放历史）时，
// 本地观察到的事件流必然缺少此前的候选，据此推导入索引会与后端操作级缓存
// 错位。该订阅在发布任何缓冲候选之前以可重试特性错误拒绝，之后不再转发事件，
// 也不取消这个不属于本订阅者的操作。启动本身失败时保留原始错误。

import { getHavenClient } from "@/lib/ipc/runtime";
import { HavenError, toHavenError } from "@/lib/ipc/errors";
import type {
  QueryCategory,
  SearchSourceEvent,
  SearchStartResultDto,
  SourceWorkImportRequest,
  SourceWorkImportResult,
} from "@/lib/ipc/generated/wire";

/** 已有一个同条件操作尚未结束、本次订阅无法获得完整历史时的特性错误码。 */
export const SOURCE_SEARCH_BUSY_CODE = "SOURCE_SEARCH_BUSY";

function sourceSearchBusyError(): HavenError {
  return new HavenError({
    code: SOURCE_SEARCH_BUSY_CODE,
    userMessage: "相同关键词的来源搜索尚未结束，暂时不能重新查询",
    retryable: true,
  });
}

/**
 * 发起来源搜索；事件经 Channel 回调逐条推送。category 为后端参与者分类门控。
 *
 * 只有确认是本次全新登记（alreadyRunning === false）且事件归属权威
 * operationId 时才会调用 onEvent；此前到达的事件按 sequence 顺序补发。
 */
export async function startSourceSearch(
  query: string,
  onEvent: (event: SearchSourceEvent) => void,
  category?: QueryCategory,
): Promise<SearchStartResultDto> {
  // 握手状态：settled=false 时只缓冲；settled=true 后按权威 operationId 过滤。
  let settled = false;
  let authoritativeOperationId: string | null = null;
  const buffered: SearchSourceEvent[] = [];

  const deliver = (event: SearchSourceEvent) => {
    if (!settled) {
      buffered.push(event);
      return;
    }
    // 被拒绝的订阅（alreadyRunning / 启动失败）永不再触达调用方。
    if (authoritativeOperationId !== null && event.operationId === authoritativeOperationId) {
      onEvent(event);
    }
  };

  let result: SearchStartResultDto;
  try {
    result = await getHavenClient().searchSourceStart(
      { query, category: category ?? null, limitPerSource: null },
      deliver,
    );
  } catch (error) {
    // 启动失败：丢弃半截事件流，保留原始错误（含 retryable 语义）。
    settled = true;
    buffered.length = 0;
    throw error;
  }

  settled = true;
  if (result.alreadyRunning) {
    // 加入既有操作：无法观测完整历史，先在发布任何候选前拒绝。
    // 不调用 search_source_cancel——该操作不属于本订阅者。
    buffered.length = 0;
    throw sourceSearchBusyError();
  }

  authoritativeOperationId = result.operationId;
  const matching = buffered.filter((event) => event.operationId === result.operationId);
  buffered.length = 0;
  matching.sort((a, b) => a.sequence - b.sequence);
  for (const event of matching) onEvent(event);
  return result;
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
