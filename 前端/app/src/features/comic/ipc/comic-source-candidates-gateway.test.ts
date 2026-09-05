import { describe, expect, it, vi } from "vitest"
import type { HavenClient } from "@/lib/ipc/client"
import { HavenError } from "@/lib/ipc/errors"
import type { ComicChapterSourceCandidatesDto } from "@/lib/ipc/generated/wire"
import { getComicChapterSourceCandidates } from "./comic-source-candidates-gateway"

const request = {
  source: {
    sourceId: "mangadex",
    remoteWorkId: "aaaaaaaa-aaaa-4aaa-8000-aaaaaaaaaaaa",
    remoteChapterId: "bbbbbbbb-bbbb-4bbb-8000-bbbbbbbbbbbb",
  },
} as const

const result: ComicChapterSourceCandidatesDto = {
  schemaVersion: 1,
  source: request.source,
  currentMediaItemId: "cccccccc-cccc-4ccc-8000-cccccccccccc",
  candidates: [
    {
      source: {
        sourceId: "reader-ws",
        remoteWorkId: "reader-work-12",
        remoteChapterId: "reader-chapter-12",
      },
      mediaItemId: "dddddddd-dddd-4ddd-8000-dddddddddddd",
      chapterNumber: 12,
      volumeNumber: null,
      title: "第 12 话",
      pageCount: 24,
      sourceOrder: 0,
      availability: "available",
      publishedAt: null,
      sourceUpdatedAt: null,
      lastSeenGeneration: 2,
      editionProfile: {
        language: "zh-cn",
        languageKind: "known",
        translationLine: null,
        translationLineKind: "unknown",
        scanGroup: "group-a",
        scanGroupKind: "content_line",
        colorMode: "unknown",
      },
      matchResult: {
        kind: "same_logical_chapter_variant",
        confidence: "medium",
        progressMigration: "one_time",
        evidence: [
          { kind: "matching_chapter_metadata", matched: null },
          { kind: "partial_page_identity", matched: 12 },
        ],
      },
    },
  ],
  truncated: false,
}

describe("comic chapter source candidates gateway", () => {
  it("returns the backend-ranked safe candidate projection", async () => {
    const comicChapterSourceCandidatesGet = vi.fn<HavenClient["comicChapterSourceCandidatesGet"]>()
      .mockResolvedValue(result)

    await expect(getComicChapterSourceCandidates(
      request,
      { comicChapterSourceCandidatesGet },
    )).resolves.toBe(result)
    expect(comicChapterSourceCandidatesGet).toHaveBeenCalledWith(request)
  })

  it("rejects malformed source identities before invoking the client", async () => {
    const comicChapterSourceCandidatesGet = vi.fn<HavenClient["comicChapterSourceCandidatesGet"]>()
    await expect(getComicChapterSourceCandidates({
      source: { ...request.source, remoteChapterId: "https://example.invalid/chapter" },
    }, { comicChapterSourceCandidatesGet })).rejects.toMatchObject({
      code: "INVALID_ARGUMENT",
      retryable: false,
    })
    expect(comicChapterSourceCandidatesGet).not.toHaveBeenCalled()
  })

  it("rejects a response whose echoed source or candidate contains internal fields", async () => {
    const comicChapterSourceCandidatesGet = vi.fn<HavenClient["comicChapterSourceCandidatesGet"]>()
      .mockResolvedValue({
        ...result,
        source: { ...result.source, remoteChapterId: "other-chapter" },
      })
    await expect(getComicChapterSourceCandidates(request, {
      comicChapterSourceCandidatesGet,
    })).rejects.toMatchObject({
      code: "COMIC_SOURCE_CANDIDATES_INVALID_RESPONSE",
      retryable: false,
    })

    const expected = new HavenError({
      code: "SOURCE_UNAVAILABLE",
      userMessage: "来源暂时不可用",
      retryable: true,
    })
    comicChapterSourceCandidatesGet.mockRejectedValueOnce(expected)
    await expect(getComicChapterSourceCandidates(request, {
      comicChapterSourceCandidatesGet,
    })).rejects.toBe(expected)
  })
})
