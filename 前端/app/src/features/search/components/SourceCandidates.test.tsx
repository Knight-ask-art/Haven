// SourceCandidates 来源健康度明细（V2-H 收尾批次）纯函数测试：
// - warning 事件累计为可折叠明细列表数据（源名 · 安全文案 / 稳定码）。
// - 非 warning 事件不改变明细；message 缺失回退稳定 code。
import { describe, expect, it } from "vitest"
import type { SearchSourceEvent, SearchSourceEventData, SearchSourceEventKind } from "@/lib/ipc/generated/wire"
import {
  accumulateWarning,
  sourceDisplayName,
  warningLineText,
  type SourceWarning,
} from "../lib/source-warnings"

function event(
  kind: SearchSourceEventKind,
  data: SearchSourceEventData,
): SearchSourceEvent {
  return { operationId: "op-1", sequence: 1, at: "2026-08-26T00:00:00Z", kind, data }
}

const emptyData: SearchSourceEventData = { sourceId: null, works: [], code: null, message: null }

describe("SourceCandidates 源健康度明细", () => {
  it("warning 事件累积明细；非 warning 不变", () => {
    let warnings: SourceWarning[] = []
    warnings = accumulateWarning(warnings, event("started", emptyData))
    expect(warnings).toHaveLength(0)
    warnings = accumulateWarning(
      warnings,
      event("warning", {
        sourceId: "opds_gutenberg",
        works: [],
        code: "SOURCE_UNAVAILABLE",
        message: "目录服务暂时不可用，请稍后重试",
      }),
    )
    expect(warnings).toHaveLength(1)
    // 六态语义：partial 不破坏既有明细。
    warnings = accumulateWarning(warnings, event("source_result", emptyData))
    expect(warnings).toHaveLength(1)
    warnings = accumulateWarning(
      warnings,
      event("warning", {
        sourceId: "custom_abc123456789",
        works: [],
        code: "CREDENTIAL_ACCESS_FAILED",
        message: null,
      }),
    )
    expect(warnings).toHaveLength(2)
  })

  it("内置源显示中文名映射，自定义源保留原样", () => {
    expect(sourceDisplayName("tvmaze")).toBe("TVMaze")
    expect(sourceDisplayName("opds_gutenberg")).toBe("古腾堡计划（OPDS）")
    expect(sourceDisplayName("custom_xyz")).toBe("custom_xyz")
  })

  it("明细行文案优先安全 userMessage，缺失回退稳定 code", () => {
    expect(
      warningLineText({
        sourceId: "archive",
        code: "SOURCE_UNAVAILABLE",
        message: "来源暂时不可用，请稍后重试",
      }),
    ).toBe("来源暂时不可用，请稍后重试")
    expect(
      warningLineText({ sourceId: "custom_x", code: "CREDENTIAL_ACCESS_FAILED", message: null }),
      ).toBe("CREDENTIAL_ACCESS_FAILED")
  })
})
