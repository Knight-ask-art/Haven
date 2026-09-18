import { describe, expect, it } from "vitest"
import {
  NO_DOWNLOAD_ATTEMPT,
  activeDownloadTaskId,
  canStartArticleDownload,
  failedArticleCapabilities,
  pendingArticleCapabilities,
  periodicalArticleAccessibleName,
  projectArticleCapabilities,
  resolvePeriodicalArticleReadState,
  type PeriodicalArticleReadState,
} from "./periodical-article-read-state"

type Info = {
  status: "idle" | "queued" | "downloaded"
  canDownload: boolean
  hasOfflineResource: boolean
  canOnlineRead: boolean
  taskId: string | null
}

const info = (overrides: Partial<Info> = {}): Info => ({
  status: "idle",
  canDownload: false,
  hasOfflineResource: false,
  canOnlineRead: false,
  taskId: null,
  ...overrides,
})

describe("resolvePeriodicalArticleReadState", () => {
  it("stays in loading until a capability projection arrives", () => {
    expect(resolvePeriodicalArticleReadState(null)).toEqual({
      state: "loading",
      label: "正在读取阅读能力",
      canOpen: false,
    })
    expect(resolvePeriodicalArticleReadState(undefined)).toEqual({
      state: "loading",
      label: "正在读取阅读能力",
      canOpen: false,
    })
  })

  it("reports unknown when the capability query itself failed", () => {
    // 查询失败不等于不可用：不能把一次失败的探测说成能力结论。
    expect(resolvePeriodicalArticleReadState(null, true)).toEqual({
      state: "unknown",
      label: "阅读能力读取失败",
      canOpen: false,
    })
    expect(resolvePeriodicalArticleReadState(info({ canOnlineRead: true }), true).state).toBe("unknown")
  })

  it("opens when the backend grants an online read, and says exactly that", () => {
    expect(resolvePeriodicalArticleReadState(info({ canOnlineRead: true }))).toEqual({
      state: "readable",
      label: "可在线阅读",
      canOpen: true,
    })
  })

  it("opens an offline-only article but never calls it an online read", () => {
    // 有离线资源、没有在线读取能力：这是离线路径，不是在线阅读路径。
    const presentation = resolvePeriodicalArticleReadState(info({ hasOfflineResource: true }))
    expect(presentation).toEqual({
      state: "readable",
      label: "可离线阅读",
      canOpen: true,
    })
    expect(presentation.label).not.toContain("在线阅读")
  })

  it("names both reading paths only when both really exist", () => {
    const presentation = resolvePeriodicalArticleReadState(info({
      canOnlineRead: true,
      hasOfflineResource: true,
    }))
    expect(presentation.state).toBe("readable")
    expect(presentation.canOpen).toBe(true)
    // 组合文案必须同时点明两条路径，不能只写在线读取。
    expect(presentation.label).toContain("可在线阅读")
    expect(presentation.label).toContain("可离线阅读")
  })

  it("keeps a downloadable-only article closed and says so", () => {
    expect(resolvePeriodicalArticleReadState(info({ canDownload: true }))).toEqual({
      state: "download_only",
      label: "需要下载后阅读",
      canOpen: false,
    })
  })

  it("reports unavailable without claiming a MetadataOnly verdict", () => {
    const presentation = resolvePeriodicalArticleReadState(info())
    expect(presentation).toEqual({
      state: "unavailable",
      label: "当前文章暂不可阅读",
      canOpen: false,
    })
    // 没有 Provider 观察（默认 unknown）时，文案不能说成已确认的元数据结论。
    expect(presentation.label).not.toContain("MetadataOnly")
    expect(presentation.label).not.toContain("仅元数据")
  })

  it("prefers readable over download_only when both capabilities exist", () => {
    expect(resolvePeriodicalArticleReadState(info({ canDownload: true, canOnlineRead: true })).state).toBe("readable")
    expect(resolvePeriodicalArticleReadState(info({ canDownload: true, hasOfflineResource: true })).state).toBe("readable")
  })

  it("never reports an openable state for anything but readable", () => {
    const states: PeriodicalArticleReadState[] = [
      "loading",
      "preparing",
      "downloading",
      "download_failed",
      "download_only",
      "metadata_only",
      "unavailable",
      "unknown",
    ]
    for (const state of states) {
      const input = state === "download_only" || state === "downloading"
        ? info({ canDownload: true, status: state === "downloading" ? "queued" : "idle", taskId: "task-1" })
        : info()
      const failed = state === "unknown"
      const download = state === "preparing"
        ? { creating: true, error: null }
        : state === "download_failed"
          ? { creating: false, error: "下载失败，请重试" }
          : NO_DOWNLOAD_ATTEMPT
      expect(resolvePeriodicalArticleReadState(state === "loading" ? null : input, failed, "unknown", download).canOpen)
        .toBe(false)
    }
  })
})

describe("download task ownership", () => {
  it("keeps ownership only for a task the projection still reports as downloading", () => {
    expect(activeDownloadTaskId(info({ status: "queued", taskId: "task-1" }))).toBe("task-1")
  })

  it("drops a task that already reached a terminal state, however its taskId reads", () => {
    // 离线资源落地之后任务已经结束：即便投影还带着这个 taskId，也没有任何可监听的东西。
    expect(activeDownloadTaskId(info({ status: "downloaded", hasOfflineResource: true, taskId: "task-1" }))).toBeNull()
    // 失败 / 取消之后重新投影回 idle：同样不再需要监听。
    expect(activeDownloadTaskId(info({ status: "idle", canDownload: true, taskId: null }))).toBeNull()
    // 排队但没有身份（没有 taskId）时也没有可归属的事件来源。
    expect(activeDownloadTaskId(info({ status: "queued", taskId: null }))).toBeNull()
  })

  it("says nothing at all before a projection exists", () => {
    // 缺失的事实不能注销一个仍可能有效的归属：调用方据此跳过对账。
    expect(activeDownloadTaskId(null)).toBeNull()
    expect(activeDownloadTaskId(undefined)).toBeNull()
  })
})

describe("article download lifecycle", () => {
  it("makes a download-only article an actionable download instead of a dead end", () => {
    // canDownload 是后端能力投影：这一篇此刻真的能下载，页面就必须给出可执行的动作，
    // 但下载本身仍不是阅读路径，因此此刻不能打开。
    expect(resolvePeriodicalArticleReadState(info({ canDownload: true }))).toEqual({
      state: "download_only",
      label: "需要下载后阅读",
      canOpen: false,
    })
  })

  it("reports an in-flight creation request as its own state", () => {
    const presentation = resolvePeriodicalArticleReadState(
      info({ canDownload: true }),
      false,
      "unknown",
      { creating: true, error: null },
    )
    expect(presentation.state).toBe("preparing")
    expect(presentation.canOpen).toBe(false)
    expect(presentation.label).not.toBe(resolvePeriodicalArticleReadState(info({ canDownload: true })).label)
  })

  it("keeps a queued task as an explicit downloading state", () => {
    const presentation = resolvePeriodicalArticleReadState(info({
      canDownload: true,
      status: "queued",
      taskId: "task-1",
    }))
    expect(presentation).toEqual({ state: "downloading", label: "下载中", canOpen: false })
  })

  it("lets the queued fact win over the page's own in-flight marker", () => {
    // 页面自己记的「正在创建」只是本地动作状态；后端已经报出排队中的任务时，
    // 屏幕上应当说真实进度，而不是停留在更早的那一步。
    const presentation = resolvePeriodicalArticleReadState(
      info({ canDownload: true, status: "queued", taskId: "task-1" }),
      false,
      "unknown",
      { creating: true, error: null },
    )
    expect(presentation.state).toBe("downloading")
  })

  it("shows a failed download as a retryable failure, not as unavailable or readable", () => {
    const presentation = resolvePeriodicalArticleReadState(
      info({ canDownload: true }),
      false,
      "unknown",
      { creating: false, error: "请先在设置中添加可用的本地存储位置" },
    )
    expect(presentation.state).toBe("download_failed")
    expect(presentation.label).toBe("请先在设置中添加可用的本地存储位置")
    expect(presentation.canOpen).toBe(false)
    // 失败不是结论：这一篇仍然可下载，所以既不能说「当前不可阅读」，也不能说可读。
    expect(presentation.label).not.toContain("不可阅读")
    expect(presentation.label).not.toContain("可离线阅读")
  })

  it("stops offering a retry when the article is no longer downloadable at all", () => {
    // 下载失败之后来源消失：此刻的明确结论是「暂不可阅读」，而不是一个点不动的重试。
    const presentation = resolvePeriodicalArticleReadState(
      info(),
      false,
      "unknown",
      { creating: false, error: "下载来源当前不可用，请检查原始媒体位置。" },
    )
    expect(presentation.state).toBe("unavailable")
  })

  it("never lets a download lifecycle state become readable on its own", () => {
    // 创建成功、排队、失败都不构成可读证据：只有重新读到的真实能力才能打开正文。
    const attempts = [
      { creating: true, error: null },
      { creating: false, error: "下载失败，请重试" },
    ]
    for (const attempt of attempts) {
      for (const input of [info({ canDownload: true }), info({ canDownload: true, status: "queued" as const })]) {
        expect(resolvePeriodicalArticleReadState(input, false, "unknown", attempt).canOpen).toBe(false)
      }
    }
  })

  it("still refuses to offer a download for a metadata-only article", () => {
    // metadata_only 是来源侧的明确结论：既不能承诺下载后阅读，也不能显示下载状态。
    const presentation = resolvePeriodicalArticleReadState(
      info({ canDownload: true, status: "queued", taskId: "task-1" }),
      false,
      "metadata_only",
      { creating: true, error: null },
    )
    expect(presentation.state).toBe("metadata_only")
    expect(presentation.canOpen).toBe(false)
  })

  it("lets a real offline resource end the download lifecycle as readable", () => {
    const presentation = resolvePeriodicalArticleReadState(
      info({ hasOfflineResource: true, status: "downloaded" }),
      false,
      "unknown",
      { creating: false, error: "上一条失败消息不该再显示" },
    )
    expect(presentation).toEqual({ state: "readable", label: "可离线阅读", canOpen: true })
    expect(presentation.label).not.toContain("在线阅读")
  })
})

describe("article download action boundary", () => {
  it("refuses to start a download for a metadata-only article the projection still calls downloadable", () => {
    // 阅读状态说得出「仅有元数据」还不够：动作一旦被调用就不再经过那个状态机，
    // 只要能力投影说 canDownload，这一篇就会真的产生一个下载任务。
    // 来源已经明确说正文不存在，创建任务等于把一条并不存在的路径做成事实。
    expect(canStartArticleDownload(info({ canDownload: true }), false, "metadata_only")).toBe(false)
    // 已经在下载的任务同样不能成为例外：metadata_only 是关于来源的结论，不因任务状态而改变。
    expect(canStartArticleDownload(
      info({ canDownload: true, status: "queued", taskId: "task-1" }),
      false,
      "metadata_only",
    )).toBe(false)
  })

  it("still starts the download for a download-only article and for a failed one being retried", () => {
    // 正常下载路径必须原样保留：来源侧没有观察到「仅有元数据」时，
    // 能力投影说能下载就是真的能下载。
    expect(canStartArticleDownload(info({ canDownload: true }), false, "unknown")).toBe(true)
    expect(canStartArticleDownload(info({ canDownload: true }), false, "full_text")).toBe(true)
    // 上一次失败不影响再次发起：重试是这条路径上唯一的出路。
    expect(canStartArticleDownload(info({ canDownload: true }), false, "unknown")).toBe(true)
  })

  it("starts nothing without a real capability fact of its own", () => {
    // 探测失败是「这次没拿到事实」，不是「可以下载」；能力缺失同理。
    expect(canStartArticleDownload(info({ canDownload: true }), true, "unknown")).toBe(false)
    expect(canStartArticleDownload(null, false, "unknown")).toBe(false)
    expect(canStartArticleDownload(undefined, false, "unknown")).toBe(false)
    // 投影明确说不能下载时也不发起。
    expect(canStartArticleDownload(info(), false, "unknown")).toBe(false)
  })
})

describe("provider content availability", () => {
  it("shows a metadata-only article as metadata only instead of a generic verdict", () => {
    const presentation = resolvePeriodicalArticleReadState(info(), false, "metadata_only")
    expect(presentation.state).toBe("metadata_only")
    expect(presentation.label).toContain("仅有元数据")
    expect(presentation.label).toContain("正文未开放")
    expect(presentation.canOpen).toBe(false)
    // 这是一条关于来源的明确结论，不能与「当前不可阅读」这句笼统措辞合并。
    expect(presentation.label).not.toBe(resolvePeriodicalArticleReadState(info()).label)
  })

  it("keeps every state distinguishable from the others", () => {
    // 每个状态取一个代表：所有状态的文案必须两两不同，否则用户无法从屏幕上
    // 分辨「来源只有元数据」和「这次没探测到事实」这类完全不同的处境。
    const downloadable = info({ canDownload: true })
    const byState: Record<PeriodicalArticleReadState, { label: string; canOpen: boolean }> = {
      readable: resolvePeriodicalArticleReadState(info({ canOnlineRead: true }), false, "metadata_only"),
      loading: resolvePeriodicalArticleReadState(null, false, "metadata_only"),
      preparing: resolvePeriodicalArticleReadState(downloadable, false, "unknown", { creating: true, error: null }),
      downloading: resolvePeriodicalArticleReadState(info({ canDownload: true, status: "queued" }), false, "unknown"),
      download_failed: resolvePeriodicalArticleReadState(downloadable, false, "unknown", {
        creating: false,
        error: "下载失败，请重试",
      }),
      download_only: resolvePeriodicalArticleReadState(downloadable, false, "unknown"),
      metadata_only: resolvePeriodicalArticleReadState(info(), false, "metadata_only"),
      unavailable: resolvePeriodicalArticleReadState(info(), false, "unknown"),
      unknown: resolvePeriodicalArticleReadState(info(), true, "metadata_only"),
    }

    const labels = Object.values(byState).map((presentation) => presentation.label)
    expect(new Set(labels).size).toBe(labels.length)
    expect(Object.entries(byState).filter(([, presentation]) => presentation.canOpen).map(([state]) => state))
      .toEqual(["readable"])
  })

  it("never claims metadata-only for an unknown or full-text observation", () => {
    for (const availability of ["unknown", "full_text"] as const) {
      const presentation = resolvePeriodicalArticleReadState(info(), false, availability)
      expect(presentation.state).toBe("unavailable")
      expect(presentation.label).not.toContain("仅元数据")
    }
  })

  it("treats a missing observation as unknown rather than as metadata only", () => {
    expect(resolvePeriodicalArticleReadState(info()).state).toBe("unavailable")
    expect(resolvePeriodicalArticleReadState(info()).label).not.toContain("仅元数据")
  })

  it("lets a real online or offline resource win over a metadata-only observation", () => {
    // 来源只有元数据，但用户本地已经有可读内容：必须按真实资源路径打开，
    // 不能因为 Provider 的观察把用户自己已有的内容锁住。
    expect(resolvePeriodicalArticleReadState(info({ hasOfflineResource: true }), false, "metadata_only")).toEqual({
      state: "readable",
      label: "可离线阅读",
      canOpen: true,
    })
    expect(resolvePeriodicalArticleReadState(info({ canOnlineRead: true }), false, "metadata_only")).toEqual({
      state: "readable",
      label: "可在线阅读",
      canOpen: true,
    })
    expect(
      resolvePeriodicalArticleReadState(info({ canOnlineRead: true, hasOfflineResource: true }), false, "metadata_only").canOpen,
    ).toBe(true)
  })

  it("does not promise a download-then-read path for a metadata-only article", () => {
    // canDownload 不是可读路径：来源已经观察到正文不存在，承诺「下载后阅读」
    // 会指向一个并不存在的内容。
    const presentation = resolvePeriodicalArticleReadState(info({ canDownload: true }), false, "metadata_only")
    expect(presentation.state).toBe("metadata_only")
    expect(presentation.canOpen).toBe(false)
  })

  it("still reports a probe failure as unknown even when the provider observed metadata only", () => {
    const presentation = resolvePeriodicalArticleReadState(info(), true, "metadata_only")
    expect(presentation.state).toBe("unknown")
    expect(presentation.label).not.toContain("仅元数据")
  })
})

describe("batch capability projection", () => {
  it("keeps every requested article loading before the batch lands", () => {
    expect(pendingArticleCapabilities([])).toEqual({})
    expect(pendingArticleCapabilities(["m1", "m2"])).toEqual({
      m1: { info: null, failed: false },
      m2: { info: null, failed: false },
    })
  })

  it("marks every requested article unknown when the batch query itself failed", () => {
    expect(failedArticleCapabilities(["m1", "m2"])).toEqual({
      m1: { info: null, failed: true },
      m2: { info: null, failed: true },
    })
  })

  it("lands the facts the projection actually carries", () => {
    const projected = new Map([
      ["m1", info({ canOnlineRead: true })],
      ["m2", info({ hasOfflineResource: true })],
      ["m3", info()],
    ])
    const merged = projectArticleCapabilities(["m1", "m2", "m3"], projected)

    expect(Object.keys(merged)).toEqual(["m1", "m2", "m3"])
    expect(resolvePeriodicalArticleReadState(merged.m1.info, merged.m1.failed).label).toBe("可在线阅读")
    expect(resolvePeriodicalArticleReadState(merged.m2.info, merged.m2.failed).label).toBe("可离线阅读")
    expect(resolvePeriodicalArticleReadState(merged.m3.info, merged.m3.failed).state).toBe("unavailable")
  })

  it("keeps a missing projection entry unknown instead of inventing an unavailable verdict", () => {
    // 批量投影少了某个请求 ID：这次没有拿到那一篇的事实。把它写成「无能力」的明确
    // 结论就是把缺失当事实，因此必须保持 unknown（failed: true）。
    const merged = projectArticleCapabilities(["m1", "m2"], new Map([["m1", info()]]))
    expect(merged.m2).toEqual({ info: null, failed: true })
    expect(resolvePeriodicalArticleReadState(merged.m2.info, merged.m2.failed)).toEqual({
      state: "unknown",
      label: "阅读能力读取失败",
      canOpen: false,
    })
  })

  it("treats an explicit null entry as missing rather than as a capability", () => {
    const merged = projectArticleCapabilities(
      ["m1"],
      new Map<string, Info | null>([["m1", null]]),
    )
    expect(merged.m1.failed).toBe(true)
    expect(resolvePeriodicalArticleReadState(merged.m1.info, merged.m1.failed).state).toBe("unknown")
  })

  it("projects only the requested ids and keeps the requested order", () => {
    const merged = projectArticleCapabilities(
      ["m2", "m1"],
      new Map([["m1", info()], ["m2", info()], ["m9", info()]]),
    )
    expect(Object.keys(merged)).toEqual(["m2", "m1"])
  })

  it("keeps a single-article re-check unknown when the projection has no entry for it", () => {
    // 单篇重查与批量查询共用同一条投影规则：投影里没有这一篇时保持 unknown。
    // 这里同时钉住「不能被空投影兜底成不可用」——canDownload / hasOfflineResource /
    // canOnlineRead 全 false 的空投影只能来自真实条目，不能用来补缺失的那一条。
    const projected = new Map<string, Info>()
    const landed = projectArticleCapabilities(["m1"], projected).m1
    expect(landed).toEqual({ info: null, failed: true })
    expect(resolvePeriodicalArticleReadState(landed.info, landed.failed).state).toBe("unknown")

    // 反过来：只有真正读到的「三无」条目才是 unavailable 这个明确结论。
    const real = projectArticleCapabilities(["m1"], new Map([["m1", info()]])).m1
    expect(resolvePeriodicalArticleReadState(real.info, real.failed).state).toBe("unavailable")
  })
})

describe("periodicalArticleAccessibleName", () => {
  const title = "面向本地优先阅读器的期刊层级建模"
  const downloadable = info({ canDownload: true })

  const presentations = () => [
    resolvePeriodicalArticleReadState(null),
    resolvePeriodicalArticleReadState(info({ canOnlineRead: true })),
    resolvePeriodicalArticleReadState(downloadable, false, "unknown", { creating: true, error: null }),
    resolvePeriodicalArticleReadState(info({ canDownload: true, status: "queued" })),
    resolvePeriodicalArticleReadState(downloadable, false, "unknown", { creating: false, error: "下载失败，请重试" }),
    resolvePeriodicalArticleReadState(downloadable),
    resolvePeriodicalArticleReadState(info()),
    resolvePeriodicalArticleReadState(null, true),
    resolvePeriodicalArticleReadState(info(), false, "metadata_only"),
  ]

  it("says 打开 only for the article that can really be opened", () => {
    const [loading, readable, ...closed] = presentations()

    expect(periodicalArticleAccessibleName(readable, title)).toBe(`打开《${title}》`)
    for (const presentation of [loading, ...closed]) {
      expect(periodicalArticleAccessibleName(presentation, title)).not.toContain("打开")
    }
  })

  it("names the download action for a downloadable article instead of a verdict", () => {
    const downloadOnly = resolvePeriodicalArticleReadState(downloadable)

    // 这个按钮真的会创建下载任务，可访问名称就必须说出这个动作，
    // 而不是说一句「它打不开」让读屏用户以为没有出路。
    expect(periodicalArticleAccessibleName(downloadOnly, title)).toContain("下载")
    expect(periodicalArticleAccessibleName(downloadOnly, title)).toContain("阅读")
    expect(periodicalArticleAccessibleName(downloadOnly, title))
      .not.toBe(periodicalArticleAccessibleName(resolvePeriodicalArticleReadState(info()), title))
  })

  it("names the retry for a failed download", () => {
    const failed = resolvePeriodicalArticleReadState(downloadable, false, "unknown", {
      creating: false,
      error: "下载失败，请重试",
    })
    const downloadOnly = resolvePeriodicalArticleReadState(downloadable)

    expect(periodicalArticleAccessibleName(failed, title)).toContain("重试")
    expect(periodicalArticleAccessibleName(failed, title)).toContain("下载")
    // 失败与「还没开始下载」是两件事，名称不能共用一句话。
    expect(periodicalArticleAccessibleName(failed, title))
      .not.toBe(periodicalArticleAccessibleName(downloadOnly, title))
  })

  it("names an in-flight download as a state rather than as an action", () => {
    const preparing = resolvePeriodicalArticleReadState(downloadable, false, "unknown", { creating: true, error: null })
    const downloading = resolvePeriodicalArticleReadState(info({ canDownload: true, status: "queued" }))

    expect(periodicalArticleAccessibleName(preparing, title)).toContain("正在创建")
    expect(periodicalArticleAccessibleName(downloading, title)).toContain("正在下载")
    expect(periodicalArticleAccessibleName(preparing, title))
      .not.toBe(periodicalArticleAccessibleName(downloading, title))
  })

  it("names the retry action for an unknown article instead of a verdict", () => {
    const unknown = resolvePeriodicalArticleReadState(null, true)
    const unavailable = resolvePeriodicalArticleReadState(info())

    expect(periodicalArticleAccessibleName(unknown, title)).toContain("重试")
    // unknown 与 unavailable 是两件事，可访问名称也不能共用一句话。
    expect(periodicalArticleAccessibleName(unknown, title))
      .not.toBe(periodicalArticleAccessibleName(unavailable, title))
    expect(periodicalArticleAccessibleName(unavailable, title)).toContain("不可阅读")
  })

  it("names the metadata-only verdict instead of a generic unavailable state", () => {
    const metadataOnly = resolvePeriodicalArticleReadState(info(), false, "metadata_only")
    const unavailable = resolvePeriodicalArticleReadState(info())

    expect(periodicalArticleAccessibleName(metadataOnly, title)).toContain("仅有元数据")
    expect(periodicalArticleAccessibleName(metadataOnly, title))
      .not.toBe(periodicalArticleAccessibleName(unavailable, title))
  })

  it("keeps every state distinguishable and still names the article", () => {
    const names = presentations().map((presentation) => periodicalArticleAccessibleName(presentation, title))

    expect(names).toHaveLength(9)
    expect(new Set(names).size).toBe(names.length)
    for (const name of names) expect(name).toContain(title)
  })
})
