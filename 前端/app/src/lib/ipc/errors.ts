// IPC 错误层（C-02：ErrorDto 已进入单一生成源；前端依据 code 决定回滚/刷新/重试/导航）。

import type { ErrorDto } from "./generated/wire";

/** 运行时守卫：任意 JSON 是否为契约 ErrorDto 形状（C-07 双端消费基线）。 */
export function isErrorDto(v: unknown): v is ErrorDto {
  if (typeof v !== "object" || v === null) return false;
  const e = v as Record<string, unknown>;
  return (
    typeof e.code === "string" &&
    e.code.length > 0 &&
    typeof e.userMessage === "string" &&
    typeof e.retryable === "boolean"
  );
}

/** 前端统一错误类型：携带稳定 code 与 retryable 语义。 */
export class HavenError extends Error {
  readonly dto: ErrorDto;

  constructor(dto: ErrorDto, options?: ErrorOptions) {
    super(dto.userMessage, options);
    this.name = "HavenError";
    this.dto = dto;
  }

  get code(): string {
    return this.dto.code;
  }

  get retryable(): boolean {
    return this.dto.retryable;
  }
}

/** 把任意后端响应归一为 ErrorDto（非契约形状 → INTERNAL_ERROR 兜底）。 */
export function toHavenError(v: unknown): HavenError {
  // Preserve the canonical error instance as it crosses client/gateway/hooks.
  // HavenError intentionally exposes code/retryable via accessors, so it is not
  // itself a top-level ErrorDto object and must be handled before isErrorDto.
  if (v instanceof HavenError) return v;
  if (isErrorDto(v)) return new HavenError(v);
  return new HavenError({
    code: "INTERNAL_ERROR",
    userMessage: "操作失败，请稍后重试",
    retryable: false,
  });
}
