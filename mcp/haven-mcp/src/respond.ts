// 响应构造：脱敏 → 有界 → 文本与 structuredContent 同源。
//
// 规范：docs/architecture/AI_SYSTEM.md §9（G-脱敏 / G-有界 / G-一致）。
//
// 三条不变量（由 `test/tools.test.ts` 对全部 9 个工具逐一断言）：
// 1. **同源**：文本永远由 `structuredContent` 渲染而来，绝不第二次读数据。
//    `json` 格式下 `JSON.parse(text)` 必须与 `structuredContent` 深度相等；
//    `markdown` 格式下结构化值里的每个标量都必须出现在文本里。
// 2. **脱敏**：`structuredContent` 在渲染前先过 `redactValue`，因此文本也自动干净。
// 3. **有界**：超限时走工具自带的 shrink 规则收缩，并**重新渲染**文本；
//    收缩后仍超限则返回稳定错误，而不是把响应截断成两份不一致的东西。

import type { CallToolResult } from "@modelcontextprotocol/sdk/types.js";

import { CHARACTER_LIMIT } from "./constants.js";
import type { ResponseFormat } from "./constants.js";
import { responseTooLarge } from "./errors.js";
import { redactValue } from "./redact.js";

/** 收缩步骤上限：防止某个 shrink 实现收敛不了导致死循环。导出供测试断言这个硬上限。 */
export const MAX_SHRINK_STEPS = 8;

export interface SuccessInput<T extends Record<string, unknown>> {
  toolName: string;
  structured: T;
  format: ResponseFormat;
  /** 从**同一个**结构化对象渲染文本行（markdown 格式使用）。 */
  render: (value: T) => string[];
  /**
   * 收缩一步：返回更小的结构化值，或 `null` 表示"已经不能再收缩"。
   * 实现必须同时把 `truncated` 之类的标记写进返回值，让客户端看得见省略。
   */
  shrink?: (value: T) => T | null;
}

/** `json` 格式的渲染：`JSON.parse(text)` 与 `structuredContent` 深度相等。 */
export function renderJson(value: unknown): string {
  return JSON.stringify(value, null, 2);
}

/**
 * 通用 markdown 渲染：从**同一个**结构化对象派生。
 *
 * 刻意做成通用的：9 个工具各写一份手写模板，就有 9 处"改了结构忘了改文案"的机会，
 * 而这条不变量的价值恰恰在于"文本永远不可能描述一个对象里没有的事实"。
 * 通用渲染器让一致性成为结构性事实，而不是需要逐工具维护的纪律。
 */
export function renderRecord(title: string, value: Record<string, unknown>): string[] {
  const lines: string[] = [`# ${title}`, ""];
  appendEntries(lines, value, 0);
  return lines;
}

function appendEntries(lines: string[], value: unknown, depth: number): void {
  const indent = "  ".repeat(depth);
  if (value === null || value === undefined) {
    lines.push(`${indent}(none)`);
    return;
  }
  if (Array.isArray(value)) {
    if (value.length === 0) {
      lines.push(`${indent}(empty)`);
      return;
    }
    for (const item of value) {
      if (item !== null && typeof item === "object") {
        lines.push(`${indent}-`);
        appendEntries(lines, item, depth + 1);
      } else {
        lines.push(`${indent}- ${formatScalar(item)}`);
      }
    }
    return;
  }
  if (typeof value === "object") {
    for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
      if (child !== null && typeof child === "object") {
        lines.push(`${indent}${key}:`);
        appendEntries(lines, child, depth + 1);
      } else {
        lines.push(`${indent}${key}: ${formatScalar(child)}`);
      }
    }
    return;
  }
  lines.push(`${indent}${formatScalar(value)}`);
}

function formatScalar(value: unknown): string {
  if (value === null || value === undefined) return "(none)";
  if (typeof value === "boolean") return value ? "true" : "false";
  if (typeof value === "number") return String(value);
  if (typeof value === "string") return value;
  return String(value);
}

export function toolSuccess<T extends Record<string, unknown>>(input: SuccessInput<T>): CallToolResult {
  let current = redactValue(input.structured) as T;

  for (let step = 0; step <= MAX_SHRINK_STEPS; step += 1) {
    const text = input.format === "json" ? renderJson(current) : input.render(current).join("\n");
    if (text.length <= CHARACTER_LIMIT) {
      return {
        content: [{ type: "text", text }],
        structuredContent: current as Record<string, unknown>,
      };
    }
    const next = input.shrink?.(current) ?? null;
    if (next === null) break;
    current = redactValue(next) as T;
  }

  throw responseTooLarge(input.toolName);
}

/** 稳定失败结果：只有可解析的错误载荷，没有 structuredContent。 */
export function toolFailure(payload: { code: string; message: string; retryable: boolean }): CallToolResult {
  return {
    isError: true,
    content: [{ type: "text", text: JSON.stringify({ error: payload }, null, 2) }],
  };
}
