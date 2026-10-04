// Reading Overview Gateway（阅读总览：设置页「总览」分组的唯一数据通道）。
//
// 页面只依赖这里的 typed 方法，不直接 invoke、也不感知 Tauri/Mock 之分——运行时选择
// 由 lib/ipc/runtime 决定（浏览器生产环境在没有显式开关时 fail closed）。
//
// 边界：请求只有窗口天数与显式 UTC 偏移（本地日历由后端按这个偏移换算，不读运行环境
// 时区），响应是纯只读聚合投影，没有任何可写字段。越界窗口在 Rust 侧是
// `READING_OVERVIEW_INVALID_RANGE`，所以这里在发出 IPC 之前就按同一份界限拦下，
// 不让一次注定被拒绝的往返发生。

import type { HavenClient } from "../../../lib/ipc/client.js";
import { HavenError } from "../../../lib/ipc/errors.js";
import { getHavenClient } from "../../../lib/ipc/runtime.js";
import { guardReadingOverviewGetRequest } from "../../../lib/ipc/settings-wire.js";
import type {
  ReadingOverviewGetRequestWire,
  ReadingOverviewWire,
} from "../../../lib/ipc/settings-wire.js";

export interface ReadingOverviewGateway {
  /** 读取阅读总览聚合；空库返回显式空态（`sessionCount=0` + 可选统计量为 `null`）。 */
  readingOverviewGet(request: ReadingOverviewGetRequestWire): Promise<ReadingOverviewWire>;
}

/** Runtime-selected gateway; unavailable browser production fails at the client boundary. */
export const readingOverviewGateway: ReadingOverviewGateway = { readingOverviewGet };

export async function readingOverviewGet(
  request: ReadingOverviewGetRequestWire,
  client: HavenClient = getHavenClient(),
): Promise<ReadingOverviewWire> {
  if (!guardReadingOverviewGetRequest(request)) {
    throw new HavenError({
      code: "READING_OVERVIEW_INVALID_RANGE",
      userMessage: "总览统计窗口超出允许范围",
      retryable: false,
    });
  }
  return client.readingOverviewGet(request);
}
