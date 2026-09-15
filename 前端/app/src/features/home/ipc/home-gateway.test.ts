import { describe, expect, it, vi } from "vitest"
import type { ContinueItemDto, ProgressSummaryDto, WorkCardDto } from "@/lib/ipc/generated/wire"

const runtime = vi.hoisted(() => ({
  getHavenClient: vi.fn(),
  isTauriRuntime: vi.fn(),
}))

vi.mock("@/lib/ipc/runtime", () => runtime)

import { continueItemToCard, workCardToMediaCard } from "./home-gateway"

function periodicalCard(): WorkCardDto {
  return {
    workId: "periodical-work",
    title: "国家地理 · 2024 年 8 月号",
    originalTitle: "National Geographic · 2024.08",
    description: "报刊测试卡片",
    categories: ["periodical"],
    availableMediaTypes: ["document"],
    posterUri: "session://periodical-poster",
    backdropUri: null,
    releaseYear: 2024,
    ratingValue: null,
    ratingScale: null,
    favorite: false,
    progress: null,
    primaryAction: null,
    externalIds: [],
  }
}

function progress(): ProgressSummaryDto {
  return {
    mediaItemId: "periodical-item",
    completion: "in_progress",
    progressRatio: 0.4,
    revision: "revision-1",
    updatedAt: "2026-09-14T00:00:00Z",
    locator: {
      version: 1,
      kind: "book",
      data: {
        publicationResource: "periodical-resource",
        formatLocator: null,
        progression: 0.4,
        textAnchor: null,
      },
    },
  }
}

describe("home gateway periodical presentation", () => {
  it("uses the product label for recently added cards", () => {
    expect(workCardToMediaCard(periodicalCard())).toEqual(expect.objectContaining({
      subtitle: "报刊资料",
      typeBadge: "报刊资料",
    }))
  })

  it("keeps continue cards readable while preserving document semantics", () => {
    const card = periodicalCard()
    const item = {
      workId: card.workId,
      mediaItemId: "periodical-item",
      progress: progress(),
      primaryAction: null,
    } as unknown as ContinueItemDto

    expect(continueItemToCard(item, [{ ...card, progress: progress() }])).toEqual(expect.objectContaining({
      subtitle: "继续翻阅",
      typeBadge: "报刊资料",
    }))
  })
})
