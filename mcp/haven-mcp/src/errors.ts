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

export function bridgeTimeout(operation: string): HavenMcpError {
  return new HavenMcpError(
    ERROR_CODES.BRIDGE_TIMEOUT,
    `Haven 桥接在 ${operation} 上超时；应用可能正忙，可稍后重试。`,
    true,
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
    `${tool} 的响应在收缩后仍超出上限；请用更小的 limit 或更窄的过滤条件重试。`,
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
