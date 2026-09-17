// @vitest-environment jsdom

import { useLayoutEffect } from "react"
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { MemoryRouter, Route, Routes, useLocation, useNavigate, useParams } from "react-router"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { HavenError } from "@/lib/ipc/errors"
import type {
  DownloadEvent,
  DownloadStateDto,
  DownloadTaskDto,
  PeriodicalArticleDto,
  PeriodicalTreeDto,
  ResourceListDto,
  StorageLocationDto,
} from "@/lib/ipc/generated/wire"
import { PeriodicalTreePage } from "./PeriodicalTreePage"

// 页面必须经由真实 Gateway（含严格 Wire 守卫）与真实 download Gateway 读取事实，
// 因此这里只替换最外层的运行时：注入一个假 Client，其余各层保持生产实现。
const runtime = vi.hoisted(() => ({
  mode: "tauri" as "tauri" | "mock" | "unavailable",
  client: null as unknown,
}))

// vi.mock 会被提升到 import 之前，因此工厂内部只能引用 vi.hoisted 的结果。
// `.js` 与无后缀两种写法都登记，是因为 Feature Gateway 用的是带后缀的说明符。
vi.mock("@/lib/ipc/runtime", () => ({
  getHavenClientMode: () => runtime.mode,
  getHavenClient: () => runtime.client,
}))
vi.mock("@/lib/ipc/runtime.js", () => ({
  getHavenClientMode: () => runtime.mode,
  getHavenClient: () => runtime.client,
}))

const WORK_ID = "11111111-1111-4111-8111-111111111111"
const PERIODICAL_ID = "22222222-2222-4222-8222-222222222201"
const VOLUME_A = "33333333-3333-4333-8333-333333333301"
const VOLUME_B = "33333333-3333-4333-8333-333333333302"
const ISSUE_A = "44444444-4444-4444-8444-444444444401"
const ISSUE_B = "44444444-4444-4444-8444-444444444402"
const ISSUE_C = "44444444-4444-4444-8444-444444444403"
const ARTICLE_A = "55555555-5555-4555-8555-555555555501"
const ARTICLE_B = "55555555-5555-4555-8555-555555555502"
const ARTICLE_C = "55555555-5555-4555-8555-555555555503"
const MEDIA_A = "66666666-6666-4666-8666-666666666601"
const MEDIA_B = "66666666-6666-4666-8666-666666666602"
const MEDIA_C = "66666666-6666-4666-8666-666666666603"

// 另一个作品：它复用了 MEDIA_A，因此「旧树的能力被留给新树」会直接落到同一个
// mediaItemId 的正文入口上。
const WORK_B = "11111111-1111-4111-8111-111111111112"
const PERIODICAL_B = "22222222-2222-4222-8222-222222222202"
const VOLUME_D = "33333333-3333-4333-8333-333333333303"
const ISSUE_D = "44444444-4444-4444-8444-444444444404"
const ARTICLE_D = "55555555-5555-4555-8555-555555555504"

const TITLE_A = "面向本地优先阅读器的期刊层级建模"
const TITLE_B = "来源未给出序号的第二篇"
const TITLE_C = "第二卷的第一篇文章"
const TITLE_D = "另一个作品的同一篇正文"

/**
 * 后端的返回顺序就是事实顺序：卷按年份倒序、期号不连续、文章序号不递增。
 * 页面若做任何排序，下面的顺序断言就会失败。
 */
function tree(): PeriodicalTreeDto {
  return {
    schemaVersion: 1,
    workId: WORK_ID,
    periodical: {
      id: PERIODICAL_ID,
      workId: WORK_ID,
      title: "示范期刊 · 信息科学前沿",
      issnPrint: "1234-5679",
      issnElectronic: "2041-1723",
      publisher: "示范出版方",
    },
    volumes: [
      {
        id: VOLUME_B,
        periodicalId: PERIODICAL_ID,
        label: null,
        number: 2,
        year: 2025,
        ordinal: 1,
        issues: [
          {
            id: ISSUE_C,
            volumeId: VOLUME_B,
            label: "Spring",
            number: null,
            publicationDate: "2025-01",
            ordinal: 0,
            articles: [
              {
                id: ARTICLE_C,
                issueId: ISSUE_C,
                mediaItemId: MEDIA_C,
                ordinal: 9,
                title: TITLE_C,
                doi: "10.1038/s41586-025-00003-3",
                pageRange: { start: "S1", end: "S5" },
                sourceKey: "europepmc",
                remoteArticleId: "demo-0003",
                availability: "unknown",
              },
            ],
          },
        ],
      },
      {
        id: VOLUME_A,
        periodicalId: PERIODICAL_ID,
        label: null,
        number: 12,
        year: 2024,
        ordinal: 0,
        issues: [
          {
            id: ISSUE_B,
            volumeId: VOLUME_A,
            label: null,
            number: 4,
            publicationDate: "2024-04-18",
            ordinal: 1,
            articles: [
              {
                id: ARTICLE_B,
                issueId: ISSUE_B,
                mediaItemId: MEDIA_B,
                ordinal: null,
                title: TITLE_B,
                doi: null,
                pageRange: null,
                sourceKey: "europepmc",
                remoteArticleId: "demo-0002",
                availability: "unknown",
              },
            ],
          },
          {
            id: ISSUE_A,
            volumeId: VOLUME_A,
            label: "3-4",
            number: null,
            publicationDate: "2024-03",
            ordinal: 0,
            articles: [
              {
                id: ARTICLE_A,
                issueId: ISSUE_A,
                mediaItemId: MEDIA_A,
                ordinal: 1,
                title: TITLE_A,
                doi: "10.0000/zhiyue.2024.0001",
                pageRange: { start: "e12345", end: null },
                sourceKey: "europepmc",
                remoteArticleId: "demo-0001",
                availability: "full_text",
              },
            ],
          },
        ],
      },
    ],
  }
}

/**
 * 第二个作品的期刊层级。它只有一个卷期，文章刻意复用 MEDIA_A：新树与旧树指向
 * 同一个 mediaItemId 时，旧树的能力事实最容易看起来像新树的能力事实。
 *
 * `mediaItemId` 可以由调用方替换：需要证明「上一个作品的事件根本不进入这一棵树」
 * 时，用一篇不同的正文更干净。
 */
function treeForOtherWork(mediaItemId: string = MEDIA_A): PeriodicalTreeDto {
  return {
    schemaVersion: 1,
    workId: WORK_B,
    periodical: {
      id: PERIODICAL_B,
      workId: WORK_B,
      title: "另一个作品 · 期刊",
      issnPrint: "1234-5679",
      issnElectronic: "2041-1723",
      publisher: null,
    },
    volumes: [
      {
        id: VOLUME_D,
        periodicalId: PERIODICAL_B,
        label: null,
        number: 1,
        year: 2026,
        ordinal: 0,
        issues: [
          {
            id: ISSUE_D,
            volumeId: VOLUME_D,
            label: null,
            number: 1,
            publicationDate: "2026-01",
            ordinal: 0,
            articles: [
              {
                id: ARTICLE_D,
                issueId: ISSUE_D,
                mediaItemId,
                ordinal: 1,
                title: TITLE_D,
                doi: null,
                pageRange: null,
                sourceKey: "europepmc",
                remoteArticleId: "demo-0004",
                availability: "full_text",
              },
            ],
          },
        ],
      },
    ],
  }
}

/**
 * 把指定文章的 Provider 正文观察替换掉，其余事实一律不动。
 *
 * 页面必须把这条观察渲染成可辨识的文案，因此这些用例只需改动这一处事实。
 */
function withArticleAvailability(
  articleId: string,
  availability: PeriodicalArticleDto["availability"],
): PeriodicalTreeDto {
  const base = tree()
  return {
    ...base,
    volumes: base.volumes.map((volume) => ({
      ...volume,
      issues: volume.issues.map((issue) => ({
        ...issue,
        articles: issue.articles.map((article) => (
          article.id === articleId ? { ...article, availability } : article
        )),
      })),
    })),
  }
}

const resource = (overrides: Partial<ResourceListDto["items"][number]> = {}): ResourceListDto["items"][number] => ({
  resourceId: "resource-1",
  resourceType: "article_snapshot",
  availability: "available",
  mimeType: "text/html",
  size: null,
  storageDisplayName: null,
  sourceDisplayName: null,
  isOffline: false,
  isLocal: false,
  requiresReauthorization: false,
  canDownload: false,
  canOnlineRead: false,
  streamKind: null,
  ...overrides,
})

const onlineReadable = (): ResourceListDto => ({ schemaVersion: 1, items: [resource({ canOnlineRead: true })] })
const offlineAvailable = (): ResourceListDto => ({
  schemaVersion: 1,
  items: [resource({ isOffline: true, availability: "offline_available" })],
})
/** 同一条目既有在线读取能力、又有已落地的离线资源。 */
const onlineAndOffline = (): ResourceListDto => ({
  schemaVersion: 1,
  items: [
    resource({ canOnlineRead: true }),
    resource({ resourceId: "resource-2", isOffline: true, availability: "offline_available" }),
  ],
})
/**
 * 只有下载来源、还没有本地资源的文章。
 *
 * 资源 ID 带上 mediaItemId，是为了让假 Client 也能像真实后端一样，从
 * `download_create` 收到的来源反查出这一篇是谁——页面与 Gateway 都不传 mediaItemId，
 * 这正是「页面没有自己编造下载参数」的证据。
 */
const downloadOnly = (mediaItemId?: string): ResourceListDto => ({
  schemaVersion: 1,
  items: [resource({
    resourceId: mediaItemId === undefined ? "resource-1" : `resource-${mediaItemId}`,
    canDownload: true,
  })],
})
const notReadable = (): ResourceListDto => ({ schemaVersion: 1, items: [resource()] })

const resourcesByMediaItem = new Map<string, ResourceListDto>()

const DOWNLOAD_TARGET = "storage-local-1"
const storageLocations: StorageLocationDto[] = []
const downloadTasks: DownloadTaskDto[] = []
const downloadListeners = new Set<(event: DownloadEvent) => void>()
let downloadEventSequence = 0
/**
 * 每次创建都发一个**新的** taskId：真实后端不会把一次重试复用成同一个任务身份，
 * 复用 taskId 会让「上一个任务的迟到事件」与「这一个任务的真实事件」在测试里变成
 * 同一件事，从而掩盖归属没有随任务终态一起注销的问题。
 */
let downloadTaskSequence = 0
/** 创建请求的人为闸门：测试用它观察「请求在途」那一帧。 */
let downloadCreateGate: Promise<void> | null = null

function defer(): { promise: Promise<void>; resolve: () => void } {
  let resolve: () => void = () => undefined
  const promise = new Promise<void>((settle) => { resolve = settle })
  return { promise, resolve }
}

/** 连通的本地存储位置：没有它时创建下载应当被 Gateway 明确拒绝。 */
function connectStorageLocation() {
  storageLocations.push({
    locationId: DOWNLOAD_TARGET,
    displayName: "本地下载目录",
    providerType: "local",
    status: "connected",
  })
}

function mediaItemIdForResource(resourceId: string): string | null {
  for (const [mediaItemId, list] of resourcesByMediaItem) {
    if (list.items.some((item) => item.resourceId === resourceId)) return mediaItemId
  }
  return null
}

/** 任务落地：离线资源真的出现，任务才进入 completed。 */
function completeDownload(taskId: string) {
  const task = downloadTasks.find((candidate) => candidate.taskId === taskId)
  if (!task || task.mediaItemId === null) throw new Error(`没有登记过的任务：${taskId}`)
  const offlineResourceId = `offline-${task.mediaItemId}`
  task.state = "completed"
  task.offlineResourceId = offlineResourceId
  const current = resourcesByMediaItem.get(task.mediaItemId)
  resourcesByMediaItem.set(task.mediaItemId, {
    schemaVersion: 1,
    items: [
      ...(current?.items ?? []),
      resource({ resourceId: offlineResourceId, isOffline: true, availability: "offline_available" }),
    ],
  })
}

/** 推送一次真实形状的下载事件（页面之外的下载生命周期只经由这条通道到达）。 */
function emitDownloadEvent(data: {
  taskId: string
  state: DownloadStateDto
  offlineResourceId?: string | null
  errorCode?: string | null
}) {
  downloadEventSequence += 1
  const event: DownloadEvent = {
    operationId: "op-download-1",
    sequence: downloadEventSequence,
    at: "2026-09-15T00:00:10.000Z",
    kind: "updated",
    data: {
      taskId: data.taskId,
      state: data.state,
      offlineResourceId: data.offlineResourceId ?? null,
      bytesTotal: null,
      bytesDownloaded: 0,
      speedBps: null,
      etaSeconds: null,
      errorCode: data.errorCode ?? null,
    },
  }
  for (const listener of [...downloadListeners]) listener(event)
}

const client = {
  periodicalTreeGet: vi.fn(),
  resourceListByMediaItem: vi.fn(async ({ mediaItemId }: { mediaItemId: string }) => {
    const found = resourcesByMediaItem.get(mediaItemId)
    if (!found) {
      throw new HavenError({ code: "MEDIA_ITEM_NOT_FOUND", userMessage: "媒体条目不存在", retryable: false })
    }
    return found
  }),
  storageLocationList: vi.fn(async () => storageLocations),
  downloadList: vi.fn(async () => downloadTasks),
  downloadCreate: vi.fn(async ({ sourceResourceId, targetStorageId }: {
    sourceResourceId: string
    targetStorageId: string
  }) => {
    if (downloadCreateGate) await downloadCreateGate
    const mediaItemId = mediaItemIdForResource(sourceResourceId)
    if (mediaItemId === null) {
      throw new HavenError({ code: "MEDIA_ITEM_NOT_FOUND", userMessage: "媒体条目不存在", retryable: false })
    }
    downloadEventSequence += 1
    const stamp = `2026-09-15T00:00:0${downloadEventSequence}.000Z`
    downloadTaskSequence += 1
    const task: DownloadTaskDto = {
      schemaVersion: 1,
      taskId: `task-${mediaItemId}-${downloadTaskSequence}`,
      workId: WORK_ID,
      editionId: null,
      mediaItemId,
      sourceResourceId,
      targetStorageId,
      offlineResourceId: null,
      title: "示范文章",
      mediaType: "article",
      category: "periodical",
      posterUri: null,
      state: "queued",
      bytesTotal: null,
      bytesDownloaded: 0,
      speedBps: null,
      etaSeconds: null,
      progressRatio: null,
      createdAt: stamp,
      updatedAt: stamp,
    }
    downloadTasks.push(task)
    return task
  }),
  downloadSubscribe: vi.fn(async (_subscriptionId: string, onEvent: (event: DownloadEvent) => void) => {
    downloadListeners.add(onEvent)
    return async () => { downloadListeners.delete(onEvent) }
  }),
}

function ArticleReaderMarker() {
  const { mediaItemId } = useParams<{ mediaItemId?: string }>()
  return <div data-testid="article-reader">{mediaItemId}</div>
}

/**
 * 提交级探针：在每一次提交真正落到 DOM 之后记录此刻屏幕上的画面。
 *
 * 陈旧渲染只有一次提交宽——act 结束时被动副作用已经把 loading 刷上去，等测试代码
 * 拿回控制权再查询已经太晚，什么都看不到。要证明「换作品的那一次提交里，上一个
 * 作品的层级还留在屏幕上」，只能在提交阶段读 DOM：useLayoutEffect 在本次提交的
 * DOM 变更之后、浏览器重绘之前运行，因此它读到的就是那一刻用户看到、也可以点的画面。
 * 用 useLocation 是为了在导航时与页面同时重渲染，从而进入同一次提交。
 */
function CommitProbe({ onCommit }: { onCommit: (html: string) => void }) {
  useLocation()
  useLayoutEffect(() => {
    onCommit(document.body.innerHTML)
  })
  return null
}

/** 在同一棵路由树里换 workId：页面实例不会重建，竞态正发生在这一层。 */
function NavigateTo({ to }: { to: string }) {
  const navigate = useNavigate()
  return (
    <button type="button" data-testid="navigate-away" onClick={() => navigate(to)}>
      换个作品
    </button>
  )
}

function renderPage(
  initial = `/periodical/${WORK_ID}`,
  options: { onCommit?: (html: string) => void; navigateTo?: string } = {},
) {
  return render(
    <MemoryRouter initialEntries={[initial]}>
      {options.onCommit ? <CommitProbe onCommit={options.onCommit} /> : null}
      {options.navigateTo ? <NavigateTo to={options.navigateTo} /> : null}
      <Routes>
        <Route path="/periodical/:workId" element={<PeriodicalTreePage />} />
        <Route path="/article/:mediaItemId" element={<ArticleReaderMarker />} />
      </Routes>
    </MemoryRouter>
  )
}

function articleRow(title: string): HTMLElement {
  return screen.getByText(title).closest("[data-testid^='periodical-article-']") as HTMLElement
}

function openButton(title: string): HTMLButtonElement {
  return within(articleRow(title)).getByRole("button") as HTMLButtonElement
}

async function articleRowFor(title: string): Promise<HTMLElement> {
  await screen.findByText(title)
  return articleRow(title)
}

/** 这一篇被读了几次能力：用来证明旧下载事件没有替新树做多余的重读。 */
function capabilityReadsFor(mediaItemId: string): number {
  return client.resourceListByMediaItem.mock.calls
    .filter((call) => (call[0] as { mediaItemId: string }).mediaItemId === mediaItemId)
    .length
}

/**
 * 从文章行上真的发起一次下载，返回这一次创建出来的任务身份。
 *
 * 每次创建都拿到一个新的 taskId，因此「上一个任务的迟到事件」在测试里就是一个与当前
 * 任务身份不同的旧 taskId——这正是归属必须随终态一起注销的场景。
 */
async function startDownload(title: string): Promise<string> {
  await waitFor(() => expect(within(articleRow(title)).getByText("需要下载后阅读")).toBeTruthy())
  const tasksBefore = downloadTasks.length
  fireEvent.click(openButton(title))
  await waitFor(() => expect(downloadTasks.length).toBe(tasksBefore + 1))
  return downloadTasks[downloadTasks.length - 1].taskId
}

/** 断言这些文案在 DOM 中按给定先后顺序出现，即它们在列表里的渲染顺序。 */
function expectRenderedOrder(texts: string[]) {
  const nodes = texts.map((text) => screen.getByText(text))
  for (let index = 1; index < nodes.length; index += 1) {
    expect(nodes[index - 1].compareDocumentPosition(nodes[index]) & Node.DOCUMENT_POSITION_FOLLOWING)
      .toBe(Node.DOCUMENT_POSITION_FOLLOWING)
  }
}

beforeEach(() => {
  runtime.mode = "tauri"
  runtime.client = client
  resourcesByMediaItem.clear()
  storageLocations.length = 0
  connectStorageLocation()
  downloadTasks.length = 0
  downloadListeners.clear()
  downloadEventSequence = 0
  downloadTaskSequence = 0
  downloadCreateGate = null
  client.periodicalTreeGet.mockReset()
  client.resourceListByMediaItem.mockClear()
  client.downloadList.mockClear()
  client.downloadCreate.mockClear()
  client.downloadSubscribe.mockClear()
  client.storageLocationList.mockClear()
  client.periodicalTreeGet.mockResolvedValue(tree())
})
afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe("PeriodicalTreePage", () => {
  it("renders the periodical identity and every level in backend order", async () => {
    renderPage()

    expect(await screen.findByText("示范期刊 · 信息科学前沿")).toBeTruthy()
    expect(screen.getByText("ISSN（印刷版）")).toBeTruthy()
    expect(screen.getByText("1234-5679")).toBeTruthy()
    expect(screen.getByText("ISSN（电子版）")).toBeTruthy()
    expect(screen.getByText("2041-1723")).toBeTruthy()
    expect(screen.getByText("示范出版方")).toBeTruthy()

    // 卷、期、文章都保持后端返回顺序：前端不排序、不重排。
    expectRenderedOrder([
      "第 2 卷",
      "Spring",
      TITLE_C,
      "第 12 卷",
      "第 4 期",
      TITLE_B,
      "3-4",
      TITLE_A,
    ])
    expect(screen.getByText("2025 年")).toBeTruthy()
    expect(screen.getByText("2025 年 1 月")).toBeTruthy()
    expect(screen.getByText("2024 年 4 月 18 日")).toBeTruthy()
    expect(screen.getByText("2024 年 3 月")).toBeTruthy()
  })

  it("shows ordinal, page range and DOI only when the source supplied them", async () => {
    renderPage()

    expect(await screen.findByText("第 1 篇 · 页码 e12345 · DOI 10.0000/zhiyue.2024.0001")).toBeTruthy()
    expect(screen.getByText("第 9 篇 · 页码 S1–S5 · DOI 10.1038/s41586-025-00003-3")).toBeTruthy()
    // 未给序号、页码与 DOI 的文章只显示标题，不补默认值。
    const row = await articleRowFor(TITLE_B)
    expect(within(row).queryByText(/页码/)).toBeNull()
    expect(within(row).queryByText(/DOI/)).toBeNull()
    expect(within(row).queryByText(/第 \d+ 篇/)).toBeNull()
  })

  it("sends only the local work identity to the gateway", async () => {
    renderPage()

    await screen.findByText("示范期刊 · 信息科学前沿")
    expect(client.periodicalTreeGet).toHaveBeenCalledTimes(1)
    expect(client.periodicalTreeGet).toHaveBeenCalledWith({ workId: WORK_ID })
  })

  it("reads article capabilities in one batch covering every article", async () => {
    renderPage()
    await screen.findByText("示范期刊 · 信息科学前沿")

    await waitFor(() => expect(client.downloadList).toHaveBeenCalledTimes(1))
    expect(client.resourceListByMediaItem.mock.calls.map((call) => call[0].mediaItemId).sort())
      .toEqual([MEDIA_A, MEDIA_B, MEDIA_C].sort())
  })

  it("reports an empty tree as an empty state instead of an error or a fabrication", async () => {
    client.periodicalTreeGet.mockResolvedValue({ ...tree(), volumes: [] })
    renderPage()

    expect(await screen.findByText("该期刊暂无可浏览的卷期")).toBeTruthy()
    expect(screen.queryByRole("button", { name: "重试" })).toBeNull()
  })

  it("shows an honest empty state for a volume without issues and an issue without articles", async () => {
    const base = tree()
    client.periodicalTreeGet.mockResolvedValue({
      ...base,
      volumes: [
        { ...base.volumes[0], issues: [] },
        { ...base.volumes[1], issues: [{ ...base.volumes[1].issues[0], articles: [] }] },
      ],
    })
    renderPage()

    expect(await screen.findByText("本卷暂无可浏览的期号")).toBeTruthy()
    expect(screen.getByText("本期暂无可展示的文章")).toBeTruthy()
    // 空层级是后端确认过的合法结果：不是错误，也不需要读任何阅读能力。
    expect(screen.queryByRole("button", { name: "重试" })).toBeNull()
    expect(client.resourceListByMediaItem).not.toHaveBeenCalled()
  })

  it("marks a volume or issue without a source label or number as a position, not a number", async () => {
    const base = tree()
    const volume = base.volumes[1]
    client.periodicalTreeGet.mockResolvedValue({
      ...base,
      volumes: [{
        ...volume,
        label: null,
        number: null,
        ordinal: 0,
        issues: [{ ...volume.issues[0], label: null, number: null, ordinal: 0 }],
      }],
    })
    renderPage()

    expect(await screen.findByText("卷（第 1 项）")).toBeTruthy()
    expect(screen.getByText("期（第 1 项）")).toBeTruthy()
    // 来源没有给出编号时，不能输出看起来像来源编号的「第 1 卷 / 第 1 期」。
    expect(screen.queryByText("第 1 卷")).toBeNull()
    expect(screen.queryByText("第 1 期")).toBeNull()
    // 这一篇没有登记任何资源 → 批量能力查询失败，落定为 unknown 而不是「不可用」。
    await waitFor(() => expect(screen.getByText("阅读能力读取失败")).toBeTruthy())
  })

  it("surfaces PERIODICAL_NOT_FOUND as a terminal error without falling back to a fake tree", async () => {
    client.periodicalTreeGet.mockRejectedValue(new HavenError({
      code: "PERIODICAL_NOT_FOUND",
      userMessage: "该作品没有期刊层级",
      retryable: false,
    }))
    renderPage()

    expect(await screen.findByRole("heading", { name: "该作品没有期刊层级" })).toBeTruthy()
    expect(screen.queryByText("示范期刊 · 信息科学前沿")).toBeNull()
    expect(screen.queryByRole("button", { name: "重试" })).toBeNull()
  })

  it("offers a retry for a retryable failure and re-requests the same work", async () => {
    client.periodicalTreeGet.mockRejectedValueOnce(new HavenError({
      code: "IPC_UNAVAILABLE",
      userMessage: "期刊层级暂时读取失败",
      retryable: true,
    }))
    renderPage()

    fireEvent.click(await screen.findByRole("button", { name: "重试" }))

    expect(await screen.findByText("示范期刊 · 信息科学前沿")).toBeTruthy()
    expect(client.periodicalTreeGet).toHaveBeenCalledTimes(2)
    expect(client.periodicalTreeGet).toHaveBeenLastCalledWith({ workId: WORK_ID })
  })

  it("never navigates for an article whose capability is explicitly unavailable", async () => {
    resourcesByMediaItem.set(MEDIA_A, notReadable())
    resourcesByMediaItem.set(MEDIA_B, notReadable())
    resourcesByMediaItem.set(MEDIA_C, notReadable())
    renderPage()

    const row = await articleRowFor(TITLE_A)
    await waitFor(() => expect(within(row).getByText("当前文章暂不可阅读")).toBeTruthy())
    const button = openButton(TITLE_A)
    expect(button.disabled).toBe(true)
    fireEvent.click(button)

    expect(screen.queryByTestId("article-reader")).toBeNull()
  })

  it("starts the existing download for a download-only article and still never navigates", async () => {
    resourcesByMediaItem.set(MEDIA_A, downloadOnly(MEDIA_A))
    resourcesByMediaItem.set(MEDIA_B, notReadable())
    resourcesByMediaItem.set(MEDIA_C, notReadable())
    renderPage()

    const row = await articleRowFor(TITLE_A)
    await waitFor(() => expect(within(row).getByText("需要下载后阅读")).toBeTruthy())
    const button = openButton(TITLE_A)
    // 这不是死按钮：后端给了可下载来源，页面就必须给出真实可执行的动作。
    expect(button.disabled).toBe(false)
    expect(button.textContent).toBe("下载后阅读")
    expect(button.getAttribute("aria-label")).toBe(`下载《${TITLE_A}》后阅读`)

    fireEvent.click(button)

    // 只经由既有 download Gateway：来源与目标都由能力投影和存储位置决定，
    // 页面自己既不拼参数，也不碰 IPC、URL 或 SQLite。
    await waitFor(() => expect(client.downloadCreate).toHaveBeenCalledTimes(1))
    expect(client.downloadCreate).toHaveBeenCalledWith({
      sourceResourceId: `resource-${MEDIA_A}`,
      targetStorageId: DOWNLOAD_TARGET,
    })
    // 创建下载不是阅读：页面绝不因此打开正文。
    expect(screen.queryByTestId("article-reader")).toBeNull()

    // 任务排队之后，屏幕上说的是真实进度，按钮也不能再被点第二次。
    const downloading = await waitFor(() => within(articleRow(TITLE_A))
      .getByRole("button", { name: `《${TITLE_A}》正在下载` }) as HTMLButtonElement)
    expect(downloading.disabled).toBe(true)
    expect(downloading.textContent).toBe("下载中")
    expect(screen.queryByTestId("article-reader")).toBeNull()
  })

  it("does not submit a second download while one is being created or queued", async () => {
    const gate = defer()
    downloadCreateGate = gate.promise
    resourcesByMediaItem.set(MEDIA_A, downloadOnly(MEDIA_A))
    resourcesByMediaItem.set(MEDIA_B, notReadable())
    resourcesByMediaItem.set(MEDIA_C, notReadable())
    renderPage()

    await waitFor(() => expect(within(articleRow(TITLE_A)).getByText("需要下载后阅读")).toBeTruthy())
    fireEvent.click(openButton(TITLE_A))

    // 创建请求在途：按钮停在这个状态并拒绝重复提交。
    const preparing = await waitFor(() => within(articleRow(TITLE_A))
      .getByRole("button", { name: `正在创建《${TITLE_A}》的下载任务` }) as HTMLButtonElement)
    expect(preparing.disabled).toBe(true)
    expect(preparing.textContent).toBe("准备下载")
    fireEvent.click(preparing)
    expect(client.downloadCreate).toHaveBeenCalledTimes(1)

    // 任务排队之后同样不能重复创建。
    await act(async () => { gate.resolve() })
    const downloading = await waitFor(() => within(articleRow(TITLE_A))
      .getByRole("button", { name: `《${TITLE_A}》正在下载` }) as HTMLButtonElement)
    expect(downloading.disabled).toBe(true)
    fireEvent.click(downloading)

    await waitFor(() => expect(client.downloadCreate).toHaveBeenCalledTimes(1))

    // 再等一轮微任务：按钮若允许重复提交，第二次创建请求此刻就已经打出去了。
    await act(async () => { await Promise.resolve() })
    expect(client.downloadCreate).toHaveBeenCalledTimes(1)
    expect(screen.queryByTestId("article-reader")).toBeNull()
  })

  it("reports a failed download creation as a retryable failure, not as a verdict", async () => {
    // 没有连通的本地存储位置：真实 Gateway 会拒绝创建，页面必须把原因说出来。
    storageLocations.length = 0
    resourcesByMediaItem.set(MEDIA_A, downloadOnly(MEDIA_A))
    resourcesByMediaItem.set(MEDIA_B, notReadable())
    resourcesByMediaItem.set(MEDIA_C, notReadable())
    renderPage()

    await waitFor(() => expect(within(articleRow(TITLE_A)).getByText("需要下载后阅读")).toBeTruthy())
    fireEvent.click(openButton(TITLE_A))

    const retry = await waitFor(() => within(articleRow(TITLE_A))
      .getByRole("button", { name: `重试下载《${TITLE_A}》` }) as HTMLButtonElement)
    expect(retry.disabled).toBe(false)
    expect(retry.textContent).toBe("重试下载")
    // 失败原因必须可理解，而且不能被说成「不可用」这个结论或「可阅读」。
    await waitFor(() => expect(within(articleRow(TITLE_A)).getByText("请先在设置中添加可用的本地存储位置")).toBeTruthy())
    expect(within(articleRow(TITLE_A)).queryByText("当前文章暂不可阅读")).toBeNull()
    expect(within(articleRow(TITLE_A)).queryByText("可离线阅读")).toBeNull()
    expect(within(articleRow(TITLE_A)).queryByText("可在线阅读")).toBeNull()
    expect(client.downloadCreate).not.toHaveBeenCalled()
    expect(screen.queryByTestId("article-reader")).toBeNull()

    // 补回存储位置后重试：这次真的创建了任务。
    connectStorageLocation()
    fireEvent.click(retry)

    await waitFor(() => expect(client.downloadCreate).toHaveBeenCalledTimes(1))
    await waitFor(() => expect(within(articleRow(TITLE_A))
      .getByRole("button", { name: `《${TITLE_A}》正在下载` })).toBeTruthy())
    expect(within(articleRow(TITLE_A)).queryByText("请先在设置中添加可用的本地存储位置")).toBeNull()
  })

  it("shows a terminal download failure from the download lifecycle and allows another try", async () => {
    resourcesByMediaItem.set(MEDIA_A, downloadOnly(MEDIA_A))
    resourcesByMediaItem.set(MEDIA_B, notReadable())
    resourcesByMediaItem.set(MEDIA_C, notReadable())
    renderPage()

    await waitFor(() => expect(within(articleRow(TITLE_A)).getByText("需要下载后阅读")).toBeTruthy())
    fireEvent.click(openButton(TITLE_A))
    await waitFor(() => expect(client.downloadCreate).toHaveBeenCalledTimes(1))

    // 任务在下载过程中失败：事件只带稳定错误码，页面必须把它翻译成可读原因。
    const task = downloadTasks[0]
    task.state = "failed"
    await act(async () => {
      emitDownloadEvent({ taskId: task.taskId, state: "failed", errorCode: "DOWNLOAD_IO_FAILED" })
    })

    const retry = await waitFor(() => within(articleRow(TITLE_A))
      .getByRole("button", { name: `重试下载《${TITLE_A}》` }) as HTMLButtonElement)
    expect(retry.disabled).toBe(false)
    expect(within(articleRow(TITLE_A)).getByText("下载过程中发生本地读写错误，请稍后重试。")).toBeTruthy()
    // 失败不是结论，也不是可读：只有真实能力能改变这一点。
    expect(within(articleRow(TITLE_A)).queryByText("当前文章暂不可阅读")).toBeNull()
    expect(within(articleRow(TITLE_A)).queryByText("可离线阅读")).toBeNull()

    fireEvent.click(retry)
    await waitFor(() => expect(client.downloadCreate).toHaveBeenCalledTimes(2))
    await waitFor(() => expect(within(articleRow(TITLE_A))
      .getByRole("button", { name: `《${TITLE_A}》正在下载` })).toBeTruthy())
  })

  it("turns a completed download into a readable article only after the real capability comes back", async () => {
    resourcesByMediaItem.set(MEDIA_A, downloadOnly(MEDIA_A))
    resourcesByMediaItem.set(MEDIA_B, notReadable())
    resourcesByMediaItem.set(MEDIA_C, notReadable())
    renderPage()

    await waitFor(() => expect(within(articleRow(TITLE_A)).getByText("需要下载后阅读")).toBeTruthy())
    fireEvent.click(openButton(TITLE_A))

    // 创建成功只说明任务建好了：此刻还没落地，正文仍然打不开。
    const downloading = await waitFor(() => within(articleRow(TITLE_A))
      .getByRole("button", { name: `《${TITLE_A}》正在下载` }) as HTMLButtonElement)
    expect(downloading.disabled).toBe(true)
    expect(screen.queryByTestId("article-reader")).toBeNull()

    // 离线资源真的落地，终态事件到达：页面据此重新读取真实能力。
    const taskId = downloadTasks[0].taskId
    completeDownload(taskId)
    await act(async () => { emitDownloadEvent({ taskId, state: "completed", offlineResourceId: `offline-${MEDIA_A}` }) })

    const row = await waitFor(() => {
      const current = articleRow(TITLE_A)
      expect(within(current).getByText("可离线阅读")).toBeTruthy()
      return current
    })
    // 只有离线资源：不得把这条路径说成在线阅读。
    expect(within(row).queryByText(/在线阅读/)).toBeNull()

    const open = within(row).getByRole("button", { name: `打开《${TITLE_A}》` }) as HTMLButtonElement
    expect(open.disabled).toBe(false)
    expect(open.textContent).toBe("打开正文")
    fireEvent.click(open)
    expect((await screen.findByTestId("article-reader")).textContent).toBe(MEDIA_A)
  })

  it("retires a task at its terminal state so its late events can no longer rewrite the row", async () => {
    resourcesByMediaItem.set(MEDIA_A, downloadOnly(MEDIA_A))
    resourcesByMediaItem.set(MEDIA_B, notReadable())
    resourcesByMediaItem.set(MEDIA_C, notReadable())
    renderPage()

    const taskId = await startDownload(TITLE_A)
    await waitFor(() => expect(within(articleRow(TITLE_A))
      .getByRole("button", { name: `《${TITLE_A}》正在下载` })).toBeTruthy())

    // 任务真的落地，终态事件到达，页面据此重新读了一次能力：这一篇现在可离线阅读。
    completeDownload(taskId)
    await act(async () => {
      emitDownloadEvent({ taskId, state: "completed", offlineResourceId: `offline-${MEDIA_A}` })
    })
    await waitFor(() => expect(within(articleRow(TITLE_A)).getByText("可离线阅读")).toBeTruthy())
    const readsAfterTerminal = capabilityReadsFor(MEDIA_A)

    // 同一个 taskId 的迟到事件：重复的完成事件，外加一个失败的残影。这个任务已经结束，
    // 两件事都不再属于当前这一行。
    await act(async () => {
      emitDownloadEvent({ taskId, state: "failed", errorCode: "DOWNLOAD_IO_FAILED" })
      emitDownloadEvent({ taskId, state: "completed", offlineResourceId: `offline-${MEDIA_A}` })
    })

    // 既没有替这一篇多读一次能力，也没有把失败写到现在这一行上。
    expect(capabilityReadsFor(MEDIA_A)).toBe(readsAfterTerminal)
    const row = articleRow(TITLE_A)
    expect(within(row).getByText("可离线阅读")).toBeTruthy()
    expect(within(row).queryByText("下载过程中发生本地读写错误，请稍后重试。")).toBeNull()
    expect(within(row).queryByText("下载中")).toBeNull()
    const open = within(row).getByRole("button", { name: `打开《${TITLE_A}》` }) as HTMLButtonElement
    expect(open.disabled).toBe(false)
    fireEvent.click(open)
    expect((await screen.findByTestId("article-reader")).textContent).toBe(MEDIA_A)
  })

  it("does not let a cancelled task's late failure become the current state of the row", async () => {
    resourcesByMediaItem.set(MEDIA_A, downloadOnly(MEDIA_A))
    resourcesByMediaItem.set(MEDIA_B, notReadable())
    resourcesByMediaItem.set(MEDIA_C, notReadable())
    renderPage()

    const taskId = await startDownload(TITLE_A)
    await waitFor(() => expect(within(articleRow(TITLE_A))
      .getByRole("button", { name: `《${TITLE_A}》正在下载` })).toBeTruthy())

    // 取消也是终态：能力重新投影回「可下载」，按钮回到真实可执行的动作。
    downloadTasks[0].state = "cancelled"
    await act(async () => { emitDownloadEvent({ taskId, state: "cancelled" }) })
    await waitFor(() => expect(within(articleRow(TITLE_A)).getByText("需要下载后阅读")).toBeTruthy())
    const readsAfterCancel = capabilityReadsFor(MEDIA_A)

    // 同一个 taskId 的迟到失败事件：任务已经结束，不能替当前这一行凭空写出一次失败。
    await act(async () => {
      emitDownloadEvent({ taskId, state: "failed", errorCode: "DOWNLOAD_IO_FAILED" })
    })

    // 这一行仍是「可下载」，不是一次属于已经结束的旧任务的失败。
    const row = articleRow(TITLE_A)
    expect(within(row).getByText("需要下载后阅读")).toBeTruthy()
    expect(within(row).queryByText("下载过程中发生本地读写错误，请稍后重试。")).toBeNull()
    expect(within(row).queryByText("重试下载")).toBeNull()
    const button = within(row).getByRole("button", { name: `下载《${TITLE_A}》后阅读` }) as HTMLButtonElement
    expect(button.disabled).toBe(false)
    // 也没有替这一篇多读一次能力。
    expect(capabilityReadsFor(MEDIA_A)).toBe(readsAfterCancel)
  })

  it("still accepts events for a task the newly read tree projects as downloading", async () => {
    resourcesByMediaItem.set(MEDIA_A, downloadOnly(MEDIA_A))
    resourcesByMediaItem.set(MEDIA_B, notReadable())
    resourcesByMediaItem.set(MEDIA_C, notReadable())

    renderPage(`/periodical/${WORK_ID}`, { navigateTo: `/periodical/${WORK_B}` })

    const taskId = await startDownload(TITLE_A)
    await waitFor(() => expect(within(articleRow(TITLE_A))
      .getByRole("button", { name: `《${TITLE_A}》正在下载` })).toBeTruthy())

    // 换到另一个作品：新树复用了同一个 mediaItemId，重新读到的能力仍然说这一篇在下载，
    // 因此这个 taskId 在新的一代里依然是当前任务，事件必须照常接收。
    client.periodicalTreeGet.mockResolvedValue(treeForOtherWork(MEDIA_A))
    fireEvent.click(screen.getByTestId("navigate-away"))
    const nextRow = await articleRowFor(TITLE_D)
    // 新树读到的能力仍然说这一篇在下载：按钮说的是真实进度，而不是一个已经结束的任务。
    await waitFor(() => expect(within(nextRow)
      .getByRole("button", { name: `《${TITLE_D}》正在下载` })).toBeTruthy())

    completeDownload(taskId)
    await act(async () => {
      emitDownloadEvent({ taskId, state: "completed", offlineResourceId: `offline-${MEDIA_A}` })
    })

    await waitFor(() => expect(within(articleRow(TITLE_D)).getByText("可离线阅读")).toBeTruthy())
    fireEvent.click(within(articleRow(TITLE_D)).getByRole("button", { name: `打开《${TITLE_D}》` }))
    expect((await screen.findByTestId("article-reader")).textContent).toBe(MEDIA_A)
  })

  it("stops reacting to download events once the page is unmounted", async () => {
    resourcesByMediaItem.set(MEDIA_A, downloadOnly(MEDIA_A))
    resourcesByMediaItem.set(MEDIA_B, notReadable())
    resourcesByMediaItem.set(MEDIA_C, notReadable())
    const page = renderPage()

    await waitFor(() => expect(within(articleRow(TITLE_A)).getByText("需要下载后阅读")).toBeTruthy())
    fireEvent.click(openButton(TITLE_A))
    await waitFor(() => expect(client.downloadCreate).toHaveBeenCalledTimes(1))
    const taskId = downloadTasks[0].taskId
    const readsBeforeUnmount = capabilityReadsFor(MEDIA_A)

    page.unmount()
    // 订阅必须随卸载一起解除，否则离开页面之后仍会有事件打进来读能力、写状态。
    expect(downloadListeners.size).toBe(0)

    completeDownload(taskId)
    await act(async () => {
      emitDownloadEvent({ taskId, state: "completed", offlineResourceId: `offline-${MEDIA_A}` })
    })

    expect(capabilityReadsFor(MEDIA_A)).toBe(readsBeforeUnmount)
    expect(screen.queryByTestId("article-reader")).toBeNull()
  })

  it("ignores a download event that the current tree does not track", async () => {
    resourcesByMediaItem.set(MEDIA_A, downloadOnly(MEDIA_A))
    resourcesByMediaItem.set(MEDIA_B, notReadable())
    resourcesByMediaItem.set(MEDIA_C, notReadable())

    renderPage(`/periodical/${WORK_ID}`, { navigateTo: `/periodical/${WORK_B}` })

    await waitFor(() => expect(within(articleRow(TITLE_A)).getByText("需要下载后阅读")).toBeTruthy())
    fireEvent.click(openButton(TITLE_A))
    await waitFor(() => expect(within(articleRow(TITLE_A))
      .getByRole("button", { name: `《${TITLE_A}》正在下载` })).toBeTruthy())
    const staleTaskId = downloadTasks[0].taskId

    // 换到另一个作品：这一篇是另一条正文，新树不认识上一个作品的下载任务。
    client.periodicalTreeGet.mockResolvedValue(treeForOtherWork(MEDIA_C))
    fireEvent.click(screen.getByTestId("navigate-away"))
    await waitFor(() => expect(within(articleRow(TITLE_D)).getByText("当前文章暂不可阅读")).toBeTruthy())
    expect(screen.queryByText(TITLE_A)).toBeNull()

    const readsBeforeStaleEvents = capabilityReadsFor(MEDIA_A)
    // 上一个作品那一篇的下载此刻完成，随后又失败一次：两件事都属于上一代树。
    completeDownload(staleTaskId)
    await act(async () => {
      emitDownloadEvent({ taskId: staleTaskId, state: "completed", offlineResourceId: `offline-${MEDIA_A}` })
    })
    await act(async () => {
      emitDownloadEvent({ taskId: staleTaskId, state: "failed", errorCode: "DOWNLOAD_IO_FAILED" })
    })

    const nextRow = articleRow(TITLE_D)
    // 认不出的任务事件不得改动当前这一棵树：既没有重读这一篇的能力，也没有把上一个
    // 作品的结论或失败状态贴上来。
    expect(within(nextRow).getByText("当前文章暂不可阅读")).toBeTruthy()
    expect(within(nextRow).queryByText("下载中")).toBeNull()
    expect(within(nextRow).queryByText("可离线阅读")).toBeNull()
    expect(within(nextRow).queryByText("重试下载")).toBeNull()
    expect(within(nextRow).queryByText("下载过程中发生本地读写错误，请稍后重试。")).toBeNull()
    expect(capabilityReadsFor(MEDIA_A)).toBe(readsBeforeStaleEvents)

    const nextButton = within(nextRow).getByRole("button", { name: `《${TITLE_D}》当前不可阅读` }) as HTMLButtonElement
    expect(nextButton.disabled).toBe(true)
    fireEvent.click(nextButton)
    expect(screen.queryByTestId("article-reader")).toBeNull()
  })

  it("shows a metadata-only article as metadata only instead of a generic verdict", async () => {
    client.periodicalTreeGet.mockResolvedValue(withArticleAvailability(ARTICLE_A, "metadata_only"))
    // 后端确实给了可下载来源：来源侧的「正文未开放」仍然压过它，绝不承诺也没有下载动作。
    resourcesByMediaItem.set(MEDIA_A, downloadOnly(MEDIA_A))
    resourcesByMediaItem.set(MEDIA_B, notReadable())
    resourcesByMediaItem.set(MEDIA_C, notReadable())
    renderPage()

    const row = await articleRowFor(TITLE_A)
    await waitFor(() => expect(within(row).getByText("仅有元数据 · 正文未开放")).toBeTruthy())
    // 明确结论：按钮打不开，也不值得重试，更不会去下载一个并不存在的正文。
    const button = openButton(TITLE_A)
    expect(button.disabled).toBe(true)
    expect(button.textContent).toBe("仅元数据")
    expect(within(row).queryByText("需要下载后阅读")).toBeNull()
    fireEvent.click(button)
    expect(client.downloadCreate).not.toHaveBeenCalled()
    expect(screen.queryByTestId("article-reader")).toBeNull()
    // 同一页面上，观察为 unknown 的文章仍然只说「当前不可阅读」，两者不合并。
    expect(within(articleRow(TITLE_B)).queryByText(/仅元数据/)).toBeNull()
  })

  it("keeps a probe failure unknown even when the provider observed metadata only", async () => {
    client.periodicalTreeGet.mockResolvedValue(withArticleAvailability(ARTICLE_A, "metadata_only"))
    // MEDIA_A 没有登记任何资源 → 能力查询失败：这是「这次没拿到事实」，不是结论。
    renderPage()

    const row = await articleRowFor(TITLE_A)
    await waitFor(() => expect(within(row).getByText("阅读能力读取失败")).toBeTruthy())
    expect(within(row).queryByText(/仅元数据/)).toBeNull()
    expect(openButton(TITLE_A).disabled).toBe(false)
  })

  it("never claims full text is readable when no local reading path exists", async () => {
    client.periodicalTreeGet.mockResolvedValue(withArticleAvailability(ARTICLE_A, "full_text"))
    resourcesByMediaItem.set(MEDIA_A, notReadable())
    resourcesByMediaItem.set(MEDIA_B, notReadable())
    resourcesByMediaItem.set(MEDIA_C, notReadable())
    renderPage()

    const row = await articleRowFor(TITLE_A)
    await waitFor(() => expect(within(row).getByText("当前文章暂不可阅读")).toBeTruthy())
    expect(openButton(TITLE_A).disabled).toBe(true)
  })

  it("opens a metadata-only article that the user already has locally", async () => {
    // Provider 观察到来源只有元数据，但本地已经有离线正文：真实资源路径优先，
    // 不能因为来源的观察把用户自己已有的内容锁住。
    client.periodicalTreeGet.mockResolvedValue(withArticleAvailability(ARTICLE_A, "metadata_only"))
    resourcesByMediaItem.set(MEDIA_A, offlineAvailable())
    resourcesByMediaItem.set(MEDIA_B, notReadable())
    resourcesByMediaItem.set(MEDIA_C, notReadable())
    renderPage()

    const row = await articleRowFor(TITLE_A)
    await waitFor(() => expect(within(row).getByText("可离线阅读")).toBeTruthy())
    expect(within(row).queryByText(/仅元数据/)).toBeNull()
    fireEvent.click(openButton(TITLE_A))
    expect((await screen.findByTestId("article-reader")).textContent).toBe(MEDIA_A)
  })

  it("calls an offline-only article readable offline instead of readable online", async () => {
    resourcesByMediaItem.set(MEDIA_A, offlineAvailable())
    resourcesByMediaItem.set(MEDIA_B, notReadable())
    resourcesByMediaItem.set(MEDIA_C, notReadable())
    renderPage()

    const row = await articleRowFor(TITLE_A)
    await waitFor(() => expect(within(row).getByText("可离线阅读")).toBeTruthy())
    // 只有离线资源时不得出现「可在线阅读」这条并不存在的路径。
    expect(within(row).queryByText(/在线阅读/)).toBeNull()

    fireEvent.click(openButton(TITLE_A))
    expect((await screen.findByTestId("article-reader")).textContent).toBe(MEDIA_A)
  })

  it("names both reading paths when an article has online access and an offline resource", async () => {
    resourcesByMediaItem.set(MEDIA_A, onlineAndOffline())
    resourcesByMediaItem.set(MEDIA_B, notReadable())
    resourcesByMediaItem.set(MEDIA_C, notReadable())
    renderPage()

    const row = await articleRowFor(TITLE_A)
    await waitFor(() => expect(within(row).getByText("可在线阅读 · 可离线阅读")).toBeTruthy())

    fireEvent.click(openButton(TITLE_A))
    expect((await screen.findByTestId("article-reader")).textContent).toBe(MEDIA_A)
  })

  it("keeps a failed capability query unknown and does not navigate", async () => {
    // 没有登记任何资源 → 资源查询整体失败，能力状态必须是 unknown 而不是可用。
    renderPage()

    const row = await articleRowFor(TITLE_A)
    await waitFor(() => expect(within(row).getByText("阅读能力读取失败")).toBeTruthy())
    fireEvent.click(openButton(TITLE_A))

    // 点击触发的重查也失败：状态回到 unknown，仍然不导航。
    await waitFor(() => expect(client.resourceListByMediaItem).toHaveBeenLastCalledWith({ mediaItemId: MEDIA_A }))
    await waitFor(() => expect(within(articleRow(TITLE_A)).getByText("阅读能力读取失败")).toBeTruthy())
    expect(screen.queryByTestId("article-reader")).toBeNull()

    // 重查失败仍然是 unknown：这次没有拿到事实，不能被写成「明确不可用」的结论，
    // 也不能让按钮改口叫「打开」——它此刻能做的只有再试一次。
    expect(within(articleRow(TITLE_A)).queryByText("当前文章暂不可阅读")).toBeNull()
    const button = within(articleRow(TITLE_A)).getByRole("button", { name: `重试读取《${TITLE_A}》的阅读能力` })
    expect(button.hasAttribute("disabled")).toBe(false)
    expect(button.getAttribute("title")).toBe("阅读能力读取失败")
  })

  it("keeps the other articles readable when one article's capability query fails", async () => {
    // MEDIA_A 没有登记资源 → 这一篇的资源查询 reject。一篇失败不能把整批写成失败：
    // 其余两篇必须仍然拿到真实能力，失败的那一篇仍是可重试的 unknown。
    resourcesByMediaItem.set(MEDIA_B, onlineReadable())
    resourcesByMediaItem.set(MEDIA_C, offlineAvailable())
    renderPage()

    const broken = await articleRowFor(TITLE_A)
    await waitFor(() => expect(within(broken).getByText("阅读能力读取失败")).toBeTruthy())
    // 失败篇是「这次没拿到事实」，不是「当前文章暂不可阅读」这个结论。
    expect(within(broken).queryByText("当前文章暂不可阅读")).toBeNull()
    const retry = within(broken).getByRole("button", { name: `重试读取《${TITLE_A}》的阅读能力` })
    expect(retry.hasAttribute("disabled")).toBe(false)

    // 同一个批次里成功的两篇保留各自的真实阅读路径。
    await waitFor(() => expect(within(articleRow(TITLE_B)).getByText("可在线阅读")).toBeTruthy())
    expect(within(articleRow(TITLE_C)).getByText("可离线阅读")).toBeTruthy()

    // 成功项仍然按真实资源路径打开正文；批量里的那一篇失败不改变这一点。
    fireEvent.click(openButton(TITLE_B))
    expect((await screen.findByTestId("article-reader")).textContent).toBe(MEDIA_B)
  })

  it("names an unknown article's button after the retry it performs instead of a verdict", async () => {
    // 没有登记任何资源 → 批量能力查询失败：unknown 是可重试的探测失败，不是不可用。
    renderPage()

    const row = await articleRowFor(TITLE_A)
    await waitFor(() => expect(within(row).getByText("阅读能力读取失败")).toBeTruthy())

    const retryButton = within(articleRow(TITLE_A)).getByRole("button", { name: `重试读取《${TITLE_A}》的阅读能力` })
    expect(retryButton.hasAttribute("disabled")).toBe(false)
    expect(retryButton.textContent).toBe("重试")
    // unknown 与 unavailable 不能共用同一句话，也不能让辅助技术以为它打不开。
    expect(within(articleRow(TITLE_A)).queryByRole("button", { name: `《${TITLE_A}》当前不可阅读` })).toBeNull()
    expect(within(articleRow(TITLE_A)).queryByText("当前文章暂不可阅读")).toBeNull()
  })

  it("names each article button after the action it really performs, not always 打开", async () => {
    resourcesByMediaItem.set(MEDIA_A, onlineReadable())
    resourcesByMediaItem.set(MEDIA_B, downloadOnly(MEDIA_B))
    resourcesByMediaItem.set(MEDIA_C, notReadable())
    renderPage()

    // readable：按钮真的会打开正文。
    await waitFor(() => expect(within(articleRow(TITLE_A)).getByText("可在线阅读")).toBeTruthy())
    const openable = within(articleRow(TITLE_A)).getByRole("button", { name: `打开《${TITLE_A}》` })
    expect(openable.hasAttribute("disabled")).toBe(false)

    // download_only：正文此刻打不开，但按钮真的会创建下载，可访问名称必须说出这个动作。
    await waitFor(() => expect(within(articleRow(TITLE_B)).getByText("需要下载后阅读")).toBeTruthy())
    const needsDownload = within(articleRow(TITLE_B)).getByRole("button", { name: `下载《${TITLE_B}》后阅读` })
    expect(needsDownload.hasAttribute("disabled")).toBe(false)

    // unavailable：明确结论，可访问名称说它此刻不可阅读。
    await waitFor(() => expect(within(articleRow(TITLE_C)).getByText("当前文章暂不可阅读")).toBeTruthy())
    const unreadable = within(articleRow(TITLE_C)).getByRole("button", { name: `《${TITLE_C}》当前不可阅读` })
    expect(unreadable.hasAttribute("disabled")).toBe(true)

    // 可见文案各自说出按钮真正做的事。
    expect(openable.textContent).toBe("打开正文")
    expect(needsDownload.textContent).toBe("下载后阅读")
    expect(unreadable.textContent).toBe("暂不可阅读")
    for (const title of [TITLE_B, TITLE_C]) {
      expect(within(articleRow(title)).queryByRole("button", { name: `打开《${title}》` })).toBeNull()
    }
  })

  it("opens the article reader only for a readable article", async () => {
    resourcesByMediaItem.set(MEDIA_A, onlineReadable())
    resourcesByMediaItem.set(MEDIA_B, offlineAvailable())
    resourcesByMediaItem.set(MEDIA_C, notReadable())
    renderPage()

    const row = await articleRowFor(TITLE_C)
    await waitFor(() => expect(within(row).getByText("当前文章暂不可阅读")).toBeTruthy())
    expect(openButton(TITLE_C).disabled).toBe(true)

    const readableRow = await articleRowFor(TITLE_A)
    await waitFor(() => expect(within(readableRow).getByText("可在线阅读")).toBeTruthy())
    fireEvent.click(openButton(TITLE_A))

    expect((await screen.findByTestId("article-reader")).textContent).toBe(MEDIA_A)
  })

  it("routes the article reader by media item id, never by source or remote identity", async () => {
    resourcesByMediaItem.set(MEDIA_C, onlineReadable())
    resourcesByMediaItem.set(MEDIA_A, notReadable())
    resourcesByMediaItem.set(MEDIA_B, notReadable())
    renderPage()

    const row = await articleRowFor(TITLE_C)
    await waitFor(() => expect(within(row).getByText("可在线阅读")).toBeTruthy())
    fireEvent.click(openButton(TITLE_C))

    const reader = await screen.findByTestId("article-reader")
    expect(reader.textContent).toBe(MEDIA_C)
    expect(reader.textContent).not.toContain("demo-0003")
    expect(reader.textContent).not.toContain("europepmc")
    expect(reader.textContent).not.toContain("10.1038")
  })

  it("re-checks the cached capability on click and re-reads it without duplicate requests", async () => {
    renderPage()

    // 首轮批量能力查询失败：三篇都是 unknown，点击只触发重查，不导航。
    const row = await articleRowFor(TITLE_A)
    await waitFor(() => expect(within(row).getByText("阅读能力读取失败")).toBeTruthy())
    const button = openButton(TITLE_A)
    expect(button.disabled).toBe(false)

    resourcesByMediaItem.set(MEDIA_A, onlineReadable())
    resourcesByMediaItem.set(MEDIA_B, notReadable())
    resourcesByMediaItem.set(MEDIA_C, notReadable())
    const callsBeforeRetry = client.resourceListByMediaItem.mock.calls.length

    fireEvent.click(button)
    expect(screen.queryByTestId("article-reader")).toBeNull()

    await waitFor(() => expect(within(articleRow(TITLE_A)).getByText("可在线阅读")).toBeTruthy())
    // 同一篇的重查只发一次请求，且不影响其它文章。
    expect(client.resourceListByMediaItem.mock.calls.length).toBe(callsBeforeRetry + 1)
    expect(client.resourceListByMediaItem).toHaveBeenLastCalledWith({ mediaItemId: MEDIA_A })

    fireEvent.click(openButton(TITLE_A))
    expect((await screen.findByTestId("article-reader")).textContent).toBe(MEDIA_A)
  })

  it("fails closed with an explicit panel outside the desktop runtime", async () => {
    runtime.mode = "unavailable"
    renderPage()

    expect(await screen.findByRole("heading", { name: "当前环境无法读取本地期刊数据" })).toBeTruthy()
    expect(client.periodicalTreeGet).not.toHaveBeenCalled()
  })

  it("reports an invalid work identity without calling IPC", async () => {
    renderPage("/periodical/not-a-work-id")

    expect(await screen.findByRole("heading", { name: "期刊作品标识无效" })).toBeTruthy()
    expect(client.periodicalTreeGet).not.toHaveBeenCalled()
  })

  it("shows loading before the tree resolves", async () => {
    client.periodicalTreeGet.mockReturnValue(new Promise<PeriodicalTreeDto>(() => undefined))
    renderPage()

    expect(await screen.findByRole("heading", { name: "正在加载期刊层级" })).toBeTruthy()
    // 未落地前不渲染任何层级事实，也不提前报成功。
    expect(screen.queryByText("示范期刊 · 信息科学前沿")).toBeNull()
    expect(screen.queryByText("该期刊暂无可浏览的卷期")).toBeNull()
  })

  it("drops the previous work's tree and its readable capability as soon as the next request starts", async () => {
    // 第一个作品：MEDIA_A 明确可在线阅读，页面上真的存在一个可打开的正文入口。
    resourcesByMediaItem.set(MEDIA_A, onlineReadable())
    resourcesByMediaItem.set(MEDIA_B, notReadable())
    resourcesByMediaItem.set(MEDIA_C, notReadable())

    const commits: string[] = []
    renderPage(`/periodical/${WORK_ID}`, {
      onCommit: (html) => commits.push(html),
      navigateTo: `/periodical/${WORK_B}`,
    })

    const firstRow = await articleRowFor(TITLE_A)
    await waitFor(() => expect(within(firstRow).getByText("可在线阅读")).toBeTruthy())

    // 第二个作品的层级请求挂起不返回；它复用了同一个 mediaItemId。
    resourcesByMediaItem.set(MEDIA_A, notReadable())
    let releaseOtherTree: (value: PeriodicalTreeDto) => void = () => undefined
    client.periodicalTreeGet.mockImplementation(() => new Promise<PeriodicalTreeDto>((resolve) => {
      releaseOtherTree = resolve
    }))

    // 换作品：同一个页面实例换 workId，旧树与新请求之间没有任何卸载。
    commits.length = 0
    fireEvent.click(screen.getByTestId("navigate-away"))
    await waitFor(() => expect(commits.length).toBeGreaterThan(0))

    // 探针只在导航提交时重渲染，因此下面这些画面正是「新请求在途、新树还没回来」的帧：
    // 旧实现在这一帧里继续渲染上一个作品的层级，连同它的 readable 结论——而两棵树共用
    // mediaItemId，旧能力一旦留下就会直接落到新作品的正文入口上。
    for (const html of commits) {
      expect(html).not.toContain(TITLE_A)
      expect(html).not.toContain("可在线阅读")
    }
    // 作废旧事实之后页面是「正在读取」，不是空树、也不是加载成功。
    expect(screen.getByRole("heading", { name: "正在加载期刊层级" })).toBeTruthy()
    expect(screen.queryByText("该期刊暂无可浏览的卷期")).toBeNull()

    // 新树落地：同一个 mediaItemId 的能力是重新读出来的，不是从上一个作品继承的。
    await act(async () => { releaseOtherTree(treeForOtherWork()) })

    const nextRow = await articleRowFor(TITLE_D)
    await waitFor(() => expect(within(nextRow).getByText("当前文章暂不可阅读")).toBeTruthy())
    expect(client.resourceListByMediaItem).toHaveBeenLastCalledWith({ mediaItemId: MEDIA_A })
  })
})
