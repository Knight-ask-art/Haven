// 报刊层级 Gateway（React UI → Feature Hook/Action → 本模块 → HavenClient → Tauri 命令）。
//
// 只转交本地 Work 身份：期刊层级由后端按 Repository 顺序组装，前端既不重排也不
// 推断归属。响应必须通过与 Rust `deny_unknown_fields` DTO 对应的严格守卫，并按
// 请求身份回显校验，避免把别的作品的期刊树当成当前事实。
//
// 这里不做 fallback、不重试、不降级成空树：拿不到可信层级时失败退出，由调用方
// 决定如何展示错误；空 `volumes` 是后端确认过的合法结果，不是本地兜底。

import type { HavenClient } from "@/lib/ipc/client";
import { HavenError, toHavenError } from "@/lib/ipc/errors.js";
import type { PeriodicalTreeDto, PeriodicalTreeGetRequest } from "@/lib/ipc/generated/wire";
import { getHavenClient } from "@/lib/ipc/runtime.js";
import { isPeriodicalTreeDto } from "@/lib/ipc/periodical-tree.js";

const CANONICAL_UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

function invalidArgument(): HavenError {
  return new HavenError({
    code: "INVALID_ARGUMENT",
    userMessage: "作品标识格式非法",
    retryable: false,
  });
}

function invalidTreeResponse(): HavenError {
  return new HavenError({
    code: "PERIODICAL_INVALID_RESPONSE",
    userMessage: "期刊层级不可用",
    retryable: false,
  });
}

/**
 * 读取一个 Work 的期刊层级。
 *
 * 非法 `workId`（非规范小写 UUID）在调用 IPC 之前就失败；响应不闭合、不属回显
 * Work、或携带传输事实（URL/路径/凭据）时抛 `PERIODICAL_INVALID_RESPONSE`。
 * 客户端已经抛出的 `HavenError`（例如 `PERIODICAL_NOT_FOUND`）原样透传。
 */
export async function getPeriodicalTree(
  workId: string,
  client: Pick<HavenClient, "periodicalTreeGet"> = getHavenClient(),
): Promise<PeriodicalTreeDto> {
  if (typeof workId !== "string" || !CANONICAL_UUID_PATTERN.test(workId)) throw invalidArgument();

  const request: PeriodicalTreeGetRequest = { workId };
  let result: unknown;
  try {
    result = await client.periodicalTreeGet(request);
  } catch (error) {
    throw toHavenError(error);
  }

  if (!isPeriodicalTreeDto(result, { workId })) throw invalidTreeResponse();
  return result;
}
