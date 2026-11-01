// @vitest-environment jsdom
//
// 来源订阅回归（复用 audit-20261028 的订阅探针，但反转预期）：
// 全新操作可以看到自己的完整事件流；加入一个已经运行、且不重放历史的后端
// 操作时，组件不得展示/导入任何局部候选，而是给出可重试的友好提示。
// 旧操作结束后显式重试必须发起全新查询，并用自己的 operationId + 本地索引导入。
//
// 这里不 mock source-search-gateway：走真实 Gateway 握手，只注入 typed client。

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { MemoryRouter } from "react-router"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type {
  SearchSourceEvent,
  SearchSourceStartRequest,
  SearchStartResultDto,
  WorkCardDto,
} from "@/lib/ipc/generated/wire"

const mocks = vi.hoisted(() => ({
  client: {
    searchSourceStart: vi.fn(),
    searchSourceCancel: vi.fn(),
    sourceWorkImport: vi.fn(),
  },
}))

// 组件图只需 isTauriRuntime（封面 URL）与本用例注入的 typed client；
// 不加载真实 runtime/tauri-client，避免 WebView 依赖进入 jsdom。
vi.mock("@/lib/ipc/runtime", () => ({
  isTauriRuntime: () => false,
  getHavenClient: () => mocks.client,
}))

import { SourceCandidates } from "./SourceCandidates"

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
      sourceId: kind === "source_result" ? "mangadex" : null,
      works,
      code: null,
      message: null,
    },
  }
}

function fresh(operationId: string): SearchStartResultDto {
  return { operationId, taskId: "task", alreadyRunning: false }
}

/** 条件驱动的挂起启动：只有用例显式 resolve 才会落定，不依赖计时器。 */
function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((res) => { resolve = res })
  return { promise, resolve }
}

const channels: Channel[] = []

beforeEach(() => {
  channels.length = 0
  mocks.client.searchSourceStart.mockReset()
  mocks.client.searchSourceCancel.mockReset().mockResolvedValue({
    operationId: "cancelled",
    alreadyTerminal: true,
  })
  mocks.client.sourceWorkImport.mockReset().mockResolvedValue({ workId: "import-result" })
})

afterEach(cleanup)

describe("来源订阅的索引安全与恢复", () => {
  it("拒绝已完成历史的局部接入，不显示也不导入局部候选，重试后使用全新索引", async () => {
    let call = 0
    mocks.client.searchSourceStart.mockImplementation(
      async (_request: SearchSourceStartRequest, onEvent?: Channel) => {
        call += 1
        const channel = onEvent!
        channels.push(channel)
        if (call === 1) {
          // 首次订阅：全新操作，启动返回前同步流出完整缓存切片。
          channel(event("old-operation", 1, "source_result", [
            card("cached-A"),
            card("cached-B"),
            card("cached-C"),
          ]))
          return fresh("old-operation")
        }
        if (call === 2) {
          // 分类变化：后端按不同合并键登记了另一个全新操作。
          return fresh("other-operation")
        }
        if (call === 3) {
          // 取消尚未生效，回到原分类时加入仍在运行的旧操作（无历史重放）。
          channel(event("old-operation", 2, "source_result", [card("later-D")]))
          return {
            operationId: "old-operation",
            taskId: "task",
            alreadyRunning: true,
          } as SearchStartResultDto
        }
        // 用户显式重试：旧操作已结束，后端登记全新操作。
        channel(event("new-operation", 1, "source_result", [card("fresh-E")]))
        channel(event("new-operation", 2, "completed"))
        return fresh("new-operation")
      },
    )

    const view = render(<MemoryRouter><SourceCandidates query="query" category="all" /></MemoryRouter>)
    await waitFor(() => expect(channels).toHaveLength(1))
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "导入媒体库：cached-C" })).toBeTruthy(),
    )

    view.rerender(<MemoryRouter><SourceCandidates query="query" category="book" /></MemoryRouter>)
    await waitFor(() => expect(channels).toHaveLength(2))

    view.rerender(<MemoryRouter><SourceCandidates query="query" category="all" /></MemoryRouter>)
    await waitFor(() => expect(channels).toHaveLength(3))
    await waitFor(() =>
      expect(screen.getByText(/相同关键词的来源搜索尚未结束/)).toBeTruthy(),
    )

    // 已解析后才到达的局部事件不得填充被拒绝的订阅。
    await act(async () => {
      channels[2](event("old-operation", 3, "source_result", [card("later-D")]))
      channels[2](event("old-operation", 4, "completed"))
    })

    expect(screen.queryByText("later-D")).toBeNull()
    expect(screen.queryByText("cached-A")).toBeNull()
    expect(screen.queryByRole("button", { name: "导入媒体库：later-D" })).toBeNull()
    expect(mocks.client.sourceWorkImport).not.toHaveBeenCalled()
    expect(document.body.innerHTML).not.toContain("alreadyRunning")
    // 只取消本订阅者拥有的两个全新操作，不取消被加入的旧操作。
    expect(mocks.client.searchSourceCancel.mock.calls.map((args) => args[0])).toEqual([
      { operationId: "old-operation" },
      { operationId: "other-operation" },
    ])

    // 旧操作结束后显式重试：必须发起全新查询并导入自己的索引。
    fireEvent.click(screen.getByRole("button", { name: "重试" }))
    await waitFor(() => expect(channels).toHaveLength(4))
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "导入媒体库：fresh-E" })).toBeTruthy(),
    )
    // 启动 Promise 的 then 链也需落定后再点击（operationId 由返回值确认）。
    await act(async () => { await Promise.resolve() })
    fireEvent.click(screen.getByRole("button", { name: "导入媒体库：fresh-E" }))
    await waitFor(() =>
      expect(mocks.client.sourceWorkImport).toHaveBeenCalledWith({
        operationId: "new-operation",
        index: 0,
      }),
    )
  })

  it("查询变更早于旧启动返回：迟到的旧结果只取消它自己的操作，不污染新查询也不取消新操作", async () => {
    const startA = deferred<SearchStartResultDto>()
    const startedQueries: string[] = []
    mocks.client.searchSourceStart.mockImplementation(
      (request: SearchSourceStartRequest, onEvent?: Channel) => {
        const channel = onEvent!
        startedQueries.push(request.query)
        if (request.query === "queryA") {
          // 启动返回值挂起：这些事件只能进入 Gateway 缓冲，组件尚未拿到 operationId。
          channel(event("op-A", 1, "started"))
          channel(event("op-A", 2, "source_result", [card("late-A")]))
          channel(event("op-A", 3, "completed"))
          return startA.promise
        }
        channel(event("op-B", 1, "source_result", [card("fresh-B")]))
        channel(event("op-B", 2, "completed"))
        return Promise.resolve(fresh("op-B"))
      },
    )

    const view = render(<MemoryRouter><SourceCandidates query="queryA" category="all" /></MemoryRouter>)
    await waitFor(() => expect(startedQueries).toEqual(["queryA"]))
    // 启动未返回：旧订阅的候选仍在 Gateway 缓冲中，不得可见。
    expect(screen.queryByText("late-A")).toBeNull()

    // 查询在旧启动返回之前变更：旧订阅被释放（此时还没有 operationId）。
    view.rerender(<MemoryRouter><SourceCandidates query="queryB" category="all" /></MemoryRouter>)
    await waitFor(() => expect(startedQueries).toEqual(["queryA", "queryB"]))
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "导入媒体库：fresh-B" })).toBeTruthy(),
    )

    // 旧启动迟到落定为全新操作：只取消它自己拥有的 op-A。
    await act(async () => {
      startA.resolve(fresh("op-A"))
      await Promise.resolve()
    })
    await waitFor(() =>
      expect(mocks.client.searchSourceCancel.mock.calls.map((args) => args[0])).toEqual([
        { operationId: "op-A" },
      ]),
    )
    // 旧事件经 Gateway 释放时已被释放的订阅丢弃：新查询的候选不受污染。
    expect(screen.queryByText("late-A")).toBeNull()
    expect(screen.getByRole("button", { name: "导入媒体库：fresh-B" })).toBeTruthy()

    // 新查询仍是自己的操作：导入使用 op-B 与其本地索引 0，且从未被取消。
    await act(async () => { await Promise.resolve() })
    fireEvent.click(screen.getByRole("button", { name: "导入媒体库：fresh-B" }))
    await waitFor(() =>
      expect(mocks.client.sourceWorkImport).toHaveBeenCalledWith({
        operationId: "op-B",
        index: 0,
      }),
    )
    expect(mocks.client.searchSourceCancel.mock.calls.map((args) => args[0])).toEqual([
      { operationId: "op-A" },
    ])
  })

  it("被释放的挂起订阅遇到 alreadyRunning：拒绝局部接入、不取消被加入的操作、不发布缓冲候选", async () => {
    const start = deferred<SearchStartResultDto>()
    mocks.client.searchSourceStart.mockImplementation(
      (_request: SearchSourceStartRequest, onEvent?: Channel) => {
        const channel = onEvent!
        channels.push(channel)
        // 后端不重放历史：这条缓存候选只是本订阅者的局部片段，仍停留在 Gateway 缓冲。
        channel(event("op-old", 1, "source_result", [card("partial-old")]))
        return start.promise
      },
    )

    const view = render(<MemoryRouter><SourceCandidates query="query" category="all" /></MemoryRouter>)
    await waitFor(() => expect(channels).toHaveLength(1))
    expect(screen.queryByText("partial-old")).toBeNull()

    // 订阅在启动返回之前被释放（卸载）。
    view.unmount()

    await act(async () => {
      start.resolve({ operationId: "op-old", taskId: "task", alreadyRunning: true })
      await Promise.resolve()
    })
    await act(async () => { await Promise.resolve() })

    // 拒绝发生在发布任何缓冲候选之前：无可见候选、无导入、未取消别人的操作。
    expect(screen.queryByText("partial-old")).toBeNull()
    expect(mocks.client.sourceWorkImport).not.toHaveBeenCalled()
    expect(mocks.client.searchSourceCancel).not.toHaveBeenCalled()

    // 之后属于被加入操作的事件也不再触达该订阅。
    await act(async () => {
      channels[0](event("op-old", 2, "source_result", [card("late-old")]))
      channels[0](event("op-old", 3, "completed"))
      await Promise.resolve()
    })
    expect(screen.queryByText("late-old")).toBeNull()
    expect(mocks.client.searchSourceCancel).not.toHaveBeenCalled()
  })
})
