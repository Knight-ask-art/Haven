// 稳定错误语义。
//
// 规范：docs/architecture/AI_SYSTEM.md §9（验收门禁 G-错误）。
//
// 规则：
// - **错误码是形状，错误消息是内容。** 桥接层可以回传自己的稳定码（Haven 的
//   ErrorDto.code 风格），但必须匹配 `STABLE_CODE_PATTERN`；不匹配一律降级为
//   `HAVEN_BRIDGE_PROTOCOL_ERROR`，绝不把任意字符串当码透传。
// - **消息必须过脱敏**：桥接层本该给用户可见文案，但"本该"不是保证。
// - **绝不回显参数**：Zod 的校验错误里会带上被拒绝的输入值，而输入可能含 secret
//   （例如用户误把 API key 填进 patch 字段）。因此本地校验错误只报告**字段路径**，
//   不报告值。

import { ERROR_CODES, STABLE_CODE_PATTERN } from "./constants.js";
import { boundText, redactText } from "./redact.js";

/** 桥接层与本地共用的稳定错误载体。 */
export class HavenMcpError extends Error {
  readonly code: string;
  readonly retryable: boolean;

  constructor(code: string, message: string, retryable: boolean) {
    super(message);
    this.name = "HavenMcpError";
    this.code = STABLE_CODE_PATTERN.test(code) ? code : ERROR_CODES.BRIDGE_PROTOCOL_ERROR;
    this.retryable = retryable;
  }
}

export function bridgeUnavailable(detail: string): HavenMcpError {
  return new HavenMcpError(ERROR_CODES.BRIDGE_UNAVAILABLE, detail, false);
}

export function endpointInvalid(): HavenMcpError {
  return new HavenMcpError(
    ERROR_CODES.ENDPOINT_INVALID,
    "HAVEN_MCP_ENDPOINT 不是当前平台允许的 Haven 本地端点；未尝试连接。",
    false,
  );
}

export function bridgeNotImplemented(detail: string): HavenMcpError {
  return new HavenMcpError(ERROR_CODES.BRIDGE_NOT_IMPLEMENTED, detail, false);
}

/**
 * 桥接超时。
 *
 * `retryable` 必须由调用方按**操作的副作用**给出，不能一律写死：
 * - 只读读取超时 → 没有落任何东西，重试是安全的；
 * - 提案创建超时 → 请求可能已经被 Broker 处理并落库，"重试"会再创建一条提案，
 *   而调用方以为上一次失败了。这种结果必须是非重试的，并要求先回栖阅确认。
 */
export function bridgeTimeout(operation: string, retryable = false): HavenMcpError {
  return new HavenMcpError(
    ERROR_CODES.BRIDGE_TIMEOUT,
    retryable
      ? `${operation}超时，本次没有产生任何写入，可以重试。`
      : `Haven 桥接在 ${operation} 上超时，操作结果可能尚未确认。请先检查栖阅中是否已有待审批提案，再决定是否重试。`,
    retryable,
  );
}

/**
 * 调用方取消。
 *
 * `mayHavePersisted` 来自工具自身：提案类工具的请求一旦发出，就可能已经在 Haven 里
 * 创建了一条 pending 提案，因此取消后的结果同样"未确认"，必须非重试并让调用方去核对。
 * 只读工具没有这个风险：取消就是取消。
 */
export function bridgeCancelled(operation: string, mayHavePersisted: boolean): HavenMcpError {
  return new HavenMcpError(
    ERROR_CODES.BRIDGE_CANCELLED,
    mayHavePersisted
      ? `调用方取消了${operation}。该请求可能已经在栖阅中创建了待审批提案，结果未确认：请先在栖阅界面检查是否已有这条提案，不要直接重试。`
      : `调用方取消了${operation}，本次没有产生任何写入。`,
    !mayHavePersisted,
  );
}

export function bridgeProtocolError(detail: string): HavenMcpError {
  return new HavenMcpError(ERROR_CODES.BRIDGE_PROTOCOL_ERROR, detail, false);
}

export function capabilityUnavailable(capability: string): HavenMcpError {
  return new HavenMcpError(
    ERROR_CODES.CAPABILITY_UNAVAILABLE,
    `当前 Haven 版本未实现该能力：${capability}。可用能力见 get_system_capabilities。`,
    false,
  );
}

export function invalidArgument(detail: string): HavenMcpError {
  return new HavenMcpError(ERROR_CODES.INVALID_ARGUMENT, detail, false);
}

export function responseTooLarge(tool: string): HavenMcpError {
  return new HavenMcpError(
    ERROR_CODES.RESPONSE_TOO_LARGE,
    `${tool} 的响应超过当前上限；请用更小的 limit 或更窄的过滤条件重试。`,
    false,
  );
}

/** 错误载荷：唯一允许出 MCP 的错误形状。 */
export interface HavenErrorPayload {
  code: string;
  message: string;
  retryable: boolean;
}

/**
 * 把任意抛出物归一为稳定错误载荷。
 *
 * - `HavenMcpError` → 原样（码已校验过形状）；
 * - 其它 `Error` → `INTERNAL_ERROR`，**消息被替换为固定文案**（第三方/桥接异常消息
 *   可能带路径、URL 或参数）；
 * - 非 Error 抛出物 → 同上。
 */
export function toErrorPayload(error: unknown): HavenErrorPayload {
  if (error instanceof HavenMcpError) {
    return {
      code: error.code,
      message: redactText(boundText(error.message, 400)),
      retryable: error.retryable,
    };
  }
  return {
    code: ERROR_CODES.INTERNAL,
    message: "MCP server 内部错误；该失败不携带任何可展示的细节。",
    retryable: false,
  };
}

/** 把错误载荷渲染成 MCP 文本内容（JSON，便于客户端解析）。 */
export function errorPayloadToText(payload: HavenErrorPayload): string {
  return JSON.stringify({ error: payload }, null, 2);
}
