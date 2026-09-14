import { describe, expect, it } from "vitest"
import type { PrimaryActionDto } from "@/lib/ipc/generated/wire"
import {
  contentCategoryLabel,
  mediaPresentationLabel,
  primaryActionPresentation,
  resolveMediaItemCapability,
} from "./periodical-presentation"

const action = (kind: PrimaryActionDto["kind"], mediaItemId = "item-1"): PrimaryActionDto => ({
  kind,
  labelHint: "start",
  editionId: "edition-1",
  mediaItemId,
  locator: null,
})

describe("periodical presentation", () => {
  it("uses product labels for content categories and media variants", () => {
    expect(contentCategoryLabel("periodical")).toBe("报刊资料")
    expect(mediaPresentationLabel("tv")).toBe("剧集")
    expect(mediaPresentationLabel("document")).toBe("资料")
    expect(mediaPresentationLabel("article", "periodical")).toBe("报刊文章")
    expect(mediaPresentationLabel("document", "periodical")).toBe("报刊资料")
  })

  it("derives reader routes and action labels only from PrimaryActionDto", () => {
    expect(primaryActionPresentation(action("reader"))).toEqual({
      kind: "reader",
      label: "在线阅读",
      route: "/reader/item-1",
    })
    expect(primaryActionPresentation(action("article", "article-1"))).toEqual({
      kind: "article",
      label: "在线阅读",
      route: "/article/article-1",
    })
    expect(primaryActionPresentation(null)).toEqual({
      kind: "none",
      label: "当前内容暂不可用",
      route: null,
    })
  })

  it("does not turn a missing action into a reader route", () => {
    const downloadOnly = resolveMediaItemCapability({
      status: "idle",
      canDownload: true,
      hasOfflineResource: false,
      canOnlineRead: false,
    })

    expect(downloadOnly).toEqual({ state: "download_only", label: "需要下载后阅读" })
    expect(primaryActionPresentation(undefined).route).toBeNull()
  })

  it("prioritizes offline, queued, online, download-only, and unavailable states", () => {
    expect(resolveMediaItemCapability(null)).toEqual({ state: "loading", label: "正在读取能力" })
    expect(resolveMediaItemCapability(null, true)).toEqual({ state: "error", label: "能力读取失败" })
    expect(resolveMediaItemCapability({
      status: "downloaded",
      canDownload: true,
      hasOfflineResource: true,
      canOnlineRead: false,
    })).toEqual({ state: "offline", label: "已下载" })
    expect(resolveMediaItemCapability({
      status: "queued",
      canDownload: true,
      hasOfflineResource: false,
      canOnlineRead: false,
    })).toEqual({ state: "queued", label: "下载中" })
    expect(resolveMediaItemCapability({
      status: "idle",
      canDownload: false,
      hasOfflineResource: false,
      canOnlineRead: true,
    })).toEqual({ state: "online", label: "可在线阅读" })
    expect(resolveMediaItemCapability({
      status: "idle",
      canDownload: true,
      hasOfflineResource: false,
      canOnlineRead: false,
    })).toEqual({ state: "download_only", label: "需要下载后阅读" })
    expect(resolveMediaItemCapability({
      status: "idle",
      canDownload: false,
      hasOfflineResource: false,
      canOnlineRead: false,
    })).toEqual({ state: "unavailable", label: "当前内容暂不可用" })
  })
})
