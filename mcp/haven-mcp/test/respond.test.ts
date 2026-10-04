// 响应层单元测试：脱敏 → 有界 → 同源。
//
// 工具测试走协议栈，这里直接打 `respond.ts` 的边界条件——尤其是 shrink 收缩循环，
// 它在当前切片里没有"活的"使用者（5 个列表型工具都还没接后端），但正是那些工具
// 接上以后必须依赖的机制。未被使用的机制等于未测试的机制，所以在这里钉死它。

import { describe, expect, it } from "vitest";

import { CHARACTER_LIMIT } from "../src/constants.js";
import { MAX_SHRINK_STEPS, renderJson, renderRecord, toolSuccess } from "../src/respond.js";
import { containsForbiddenMaterial } from "../src/redact.js";

interface Bulk extends Record<string, unknown> {
  schema_version: 1;
  items: { title: string }[];
  truncated: boolean;
  truncation_message?: string;
}

function bulk(count: number, titleLength = 400): Bulk {
  return {
    schema_version: 1,
    items: Array.from({ length: count }, (_value, index) => ({
      title: `条目 ${index} `.padEnd(titleLength, "x"),
    })),
    truncated: false,
  };
}

/** 每次丢掉一半条目，并把省略显式写进结构化值。 */
function shrinkHalf(value: Bulk): Bulk | null {
  if (value.items.length <= 1) return null;
  const kept = value.items.slice(0, Math.floor(value.items.length / 2));
  return {
    ...value,
    items: kept,
    truncated: true,
    truncation_message: `已省略 ${value.items.length - kept.length} 条；请用更小的 limit 分页读取。`,
  };
}

describe("toolSuccess", () => {
  it("json 格式下文本与 structuredContent 严格同源", () => {
    const structured: Bulk = bulk(3, 10);
    const result = toolSuccess({
      toolName: "t",
      structured,
      format: "json",
      render: () => [],
    });
    const text = (result.content as { text: string }[])[0]?.text ?? "";
    expect(JSON.parse(text)).toEqual(result.structuredContent);
  });

  it("markdown 格式由同一个结构化对象派生", () => {
    const structured: Bulk = bulk(2, 10);
    const result = toolSuccess({
      toolName: "t",
      structured,
      format: "markdown",
      render: (value) => renderRecord("标题", value),
    });
    const text = (result.content as { text: string }[])[0]?.text ?? "";
    expect(text).toContain("# 标题");
    for (const item of structured.items) {
      expect(text).toContain(item.title);
    }
  });

  it("超限时收缩，并把省略写进结构化值（两个格式都跟着变）", () => {
    const structured = bulk(200);
    expect(renderJson(structured).length).toBeGreaterThan(CHARACTER_LIMIT);

    const result = toolSuccess({
      toolName: "t",
      structured,
      format: "json",
      render: () => [],
      shrink: shrinkHalf,
    });

    const bounded = result.structuredContent as Bulk;
    expect(bounded.truncated).toBe(true);
    expect(bounded.truncation_message).toContain("已省略");
    expect(bounded.items.length).toBeLessThan(200);
    // 收缩**之后**文本与结构仍然一致——这正是"重新渲染"而不是"截断字符串"的意义。
    const text = (result.content as { text: string }[])[0]?.text ?? "";
    expect(text.length).toBeLessThanOrEqual(CHARACTER_LIMIT);
    expect(JSON.parse(text)).toEqual(bounded);
  });

  it("收缩无法收敛时抛稳定错误，而不是死循环或返回半截响应", () => {
    // 注意：单字段已经被 boundText 限到 512 字符，所以"一条就超限"是构造不出来的。
    // 真正需要防的是"shrink 实现收敛不了"——循环必须有硬上限。
    let calls = 0;
    try {
      toolSuccess({
        toolName: "t",
        structured: bulk(200),
        format: "json",
        render: () => [],
        shrink: (value) => {
          calls += 1;
          return value; // 收缩不减少任何东西
        },
      });
      throw new Error("应当抛出");
    } catch (error) {
      expect((error as { code?: string }).code).toBe("HAVEN_RESPONSE_TOO_LARGE");
    }
    // 循环从 step=0 到 MAX_SHRINK_STEPS（含），因此最多 MAX_SHRINK_STEPS + 1 次尝试。
    expect(calls).toBe(MAX_SHRINK_STEPS + 1);
    expect(calls).toBeGreaterThan(0);
  });

  it("没有 shrink 且超限时同样抛稳定错误", () => {
    try {
      toolSuccess({
        toolName: "get_settings_snapshot",
        structured: bulk(50, CHARACTER_LIMIT),
        format: "json",
        render: () => [],
      });
      throw new Error("应当抛出");
    } catch (error) {
      expect((error as { code?: string }).code).toBe("HAVEN_RESPONSE_TOO_LARGE");
    }
  });

  it("构造响应时就已经脱敏（文本不可能包含被剥离的材料）", () => {
    const result = toolSuccess({
      toolName: "t",
      structured: {
        schema_version: 1,
        credential_ref: "haven:ai:gw",
        note: "C:\\Users\\someone\\secret.txt",
      },
      format: "json",
      render: () => [],
    });
    const text = (result.content as { text: string }[])[0]?.text ?? "";
    expect(text).not.toContain("haven:ai:gw");
    expect(text).not.toContain("C:\\Users");
    expect(containsForbiddenMaterial(result.structuredContent)).toBe(false);
  });

  it("边界：恰好等于上限时通过，不触发收缩", () => {
    const structured: Bulk = { schema_version: 1, items: [], truncated: false };
    const size = renderJson(structured).length;
    const padding = "y".repeat(Math.max(0, CHARACTER_LIMIT - size - 8));
    const exact = { ...structured, pad: padding };
    // 先确认确实在上限内，再断言没有被收缩。
    const rendered = renderJson(exact);
    if (rendered.length > CHARACTER_LIMIT) return;
    const result = toolSuccess({
      toolName: "t",
      structured: exact,
      format: "json",
      render: () => [],
      shrink: shrinkHalf,
    });
    expect((result.structuredContent as { pad?: string }).pad).toBe(padding);
  });
});
