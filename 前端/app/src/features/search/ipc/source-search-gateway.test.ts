// 来源搜索启动/事件握手测试（来源索引安全边界）。
//
// 这里验证的是 Gateway 对 search_source_start 的 Channel 语义假设：
// - 事件可能早于启动返回值到达 → 返回值确认前不得触达调用方；
// - 只有 alreadyRunning=false 且 operationId 匹配的完整流才可释放；
// - 加入既有操作（alreadyRunning=true）必须可重试地拒绝，且不发布任何候选；
// - 启动失败保留原始错误，不泄漏半截事件流。

import { beforeEach, describe, expect, it, vi } from "vitest"
import { HavenError } from "@/lib/ipc/errors"
import type {
  SearchSourceEvent,
  SearchStartResultDto,
  SearchSourceStartRequest,
  WorkCardDto,
} from "@/lib/ipc/generated/wire"

const mocks = vi.hoisted(() => ({ getHavenClient: vi.fn() }))

vi.mock("@/lib/ipc/runtime", () => ({ getHavenClient: mocks.getHavenClient }))

import { SOURCE_SEARCH_BUSY_CODE, startSourceSearch } from "./source-search-gateway"

type Channel = (event: SearchSourceEvent) => void

function card(name: string): WorkCardDto {
  return {
    workId: `content-candidate-${name}`,
    title: name,
    originalTitle: null,
    description: null,
    categories: ["comic"],
    availableMediaTypes: ["comic"],
    posterUri: null,
    backdropUri: null,
    releaseYear: null,
    ratingValue: null,
    ratingScale: null,
    favorite: false,
    progress: null,
    primaryAction: null,
    externalIds: [],
  }
}

function event(
  operationId: string,
  sequence: number,
  kind: SearchSourceEvent["kind"],
  works: WorkCardDto[] = [],
): SearchSourceEvent {
  return {
    operationId,
    sequence,
    at: "2026-10-31T00:00:00Z",
    kind,
    data: {
      sourceId: kind === "source_result" || kind === "warning" ? "mangadex" : null,
      works,
      code: null,
      message: null,
    },
  }
}

function fresh(operationId: string): SearchStartResultDto {
  return { operationId, taskId: "task", alreadyRunning: false }
}

beforeEach(() => {
  mocks.getHavenClient.mockReset()
})

describe("来源搜索启动握手", () => {
  it("启动返回前的同步事件先缓冲，确认全新操作后才按序触达调用方", async () => {
    let resolveStart!: (result: SearchStartResultDto) => void
    const start = new Promise<SearchStartResultDto>((resolve) => {
      resolveStart = resolve
    })
    mocks.getHavenClient.mockReturnValue({
      searchSourceStart: vi.fn(
        (_request: SearchSourceStartRequest, onEvent?: Channel) => {
          onEvent?.(event("op-fresh", 1, "started"))
          onEvent?.(event("op-fresh", 2, "source_result", [card("early")]))
          onEvent?.(event("op-other", 1, "completed"))
          onEvent?.(event("op-fresh", 3, "completed"))
          return start
        },
      ),
    })
    const seen: SearchSourceEvent[] = []

    const pending = startSourceSearch("漫画", (e) => seen.push(e))

    // 启动返回值未到：即使事件已同步流入 Channel，也不得发布候选。
    expect(seen).toEqual([])

    resolveStart(fresh("op-fresh"))
    await expect(pending).resolves.toEqual(fresh("op-fresh"))
    expect(seen.map((e) => [e.operationId, e.sequence])).toEqual([
      ["op-fresh", 1],
      ["op-fresh", 2],
      ["op-fresh", 3],
    ])
  })

  it("alreadyRunning 在发布任何缓冲候选前以可重试特性错误拒绝，并忽略后续事件", async () => {
    let channel!: Channel
    const cancel = vi.fn()
    mocks.getHavenClient.mockReturnValue({
      searchSourceStart: vi.fn((_request: SearchSourceStartRequest, onEvent?: Channel) => {
        channel = onEvent!
        // 后端不重放历史；这条缓存候选只代表本订阅者看到的局部片段。
        onEvent?.(event("op-old", 4, "source_result", [card("cached-partial")]))
        return Promise.resolve({
          operationId: "op-old",
          taskId: "task",
          alreadyRunning: true,
        } satisfies SearchStartResultDto)
      }),
      searchSourceCancel: cancel,
    })
    const onEvent = vi.fn()

    const error = await startSourceSearch("漫画", onEvent).catch((e: unknown) => e)

    expect(error).toBeInstanceOf(HavenError)
    expect((error as HavenError).code).toBe(SOURCE_SEARCH_BUSY_CODE)
    expect((error as HavenError).retryable).toBe(true)
    expect((error as HavenError).dto.userMessage).not.toContain("alreadyRunning")
    expect(onEvent).not.toHaveBeenCalled()

    channel(event("op-old", 5, "source_result", [card("later")]))
    channel(event("op-old", 6, "completed"))
    expect(onEvent).not.toHaveBeenCalled()
    // 加入别人的操作不能顺手取消它。
    expect(cancel).not.toHaveBeenCalled()
  })

  it("全新操作只释放权威 operationId 的事件，按 sequence 排序并继续转发后续事件", async () => {
    let channel!: Channel
    mocks.getHavenClient.mockReturnValue({
      searchSourceStart: vi.fn((_request: SearchSourceStartRequest, onEvent?: Channel) => {
        channel = onEvent!
        onEvent?.(event("op-fresh", 3, "completed"))
        onEvent?.(event("op-other", 9, "source_result", [card("foreign")]))
        onEvent?.(event("op-fresh", 2, "source_result", [card("second")]))
        onEvent?.(event("op-fresh", 1, "source_result", [card("first")]))
        return Promise.resolve(fresh("op-fresh"))
      }),
    })
    const seen: SearchSourceEvent[] = []

    await startSourceSearch("漫画", (e) => seen.push(e))

    expect(seen.map((e) => e.sequence)).toEqual([1, 2, 3])
    expect(seen.every((e) => e.operationId === "op-fresh")).toBe(true)
    expect(seen[1].data.works[0].title).toBe("second")

    channel(event("op-fresh", 4, "warning"))
    channel(event("op-other", 10, "completed"))
    expect(seen.map((e) => e.sequence)).toEqual([1, 2, 3, 4])
    expect(seen.every((e) => e.operationId === "op-fresh")).toBe(true)
  })

  it("启动失败保留原始错误，并丢弃已同步流入的局部事件流", async () => {
    const original = new HavenError({
      code: "SOURCE_UNAVAILABLE",
      userMessage: "来源暂不可用",
      retryable: true,
    })
    let channel!: Channel
    mocks.getHavenClient.mockReturnValue({
      searchSourceStart: vi.fn((_request: SearchSourceStartRequest, onEvent?: Channel) => {
        channel = onEvent!
        onEvent?.(event("op-fresh", 1, "source_result", [card("partial")]))
        return Promise.reject(original)
      }),
    })
    const onEvent = vi.fn()

    await expect(startSourceSearch("漫画", onEvent)).rejects.toBe(original)
    expect(onEvent).not.toHaveBeenCalled()

    channel(event("op-fresh", 2, "completed"))
    expect(onEvent).not.toHaveBeenCalled()
  })
})
