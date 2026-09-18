import { describe, expect, it, vi } from "vitest"
import type { HavenClient } from "@/lib/ipc/client"
import { HavenError } from "@/lib/ipc/errors"
import type {
  ComicPageProgressRemapRequestDto,
  ComicProgressMigrationReceiptDto,
  ComicProgressMigrationRequestDto,
  ComicProgressMigrationResultDto,
  ComicProgressMigrationRevertResultDto,
} from "@/lib/ipc/generated/wire"
import {
  isComicPageProgressRemapRequestDto,
  isComicProgressMigrationRequestDto,
  isComicProgressMigrationResultDto,
  isComicProgressMigrationRevertResultDto,
} from "@/lib/ipc/comic-progress-migration"
import {
  migrateComicProgress,
  remapComicProgress,
  revertComicProgress,
} from "./comic-progress-migration-gateway"

const source = {
  sourceId: "mangadex",
  remoteWorkId: "aaaaaaaa-aaaa-4aaa-8000-aaaaaaaaaaaa",
  remoteChapterId: "bbbbbbbb-bbbb-4bbb-8000-bbbbbbbbbbbb",
} as const

const target = {
  sourceId: "reader-ws",
  remoteWorkId: "reader-work-12",
  remoteChapterId: "reader-chapter-12",
} as const

const remapRequest: ComicPageProgressRemapRequestDto = {
  sessionId: "cccccccc-cccc-4ccc-8000-cccccccccccc",
  expectedRevision: "123",
}

const migrationEvidence = [
  { kind: "edition_compatible", matched: null },
  { kind: "partial_page_identity", matched: 1 },
] as const

const appliedPageMigration = {
  targetPageIndex: 1,
  confidence: "medium",
  strategy: "nearest_surviving_page",
  reversible: true,
} as const

const appliedReceipt: ComicProgressMigrationReceiptDto = {
  migrationId: "dddddddd-dddd-4ddd-8000-dddddddddddd",
  sourceMediaItemId: "source-media-1",
  targetMediaItemId: "target-media-1",
  strategy: "nearest_surviving_page",
  confidence: "medium",
  evidence: [...migrationEvidence],
  sourceProgressSnapshot: {
    mediaItemId: "source-media-1",
    pageIndex: 1,
    pageProgression: 0.5,
    completion: "in_progress",
    percentage: 0.5,
    revision: "123",
    updatedAt: "2026-09-12T00:00:00.000Z",
  },
  targetProgressBefore: null,
  targetProgressAfter: {
    mediaItemId: "target-media-1",
    pageIndex: 1,
    pageProgression: 0.5,
    completion: "in_progress",
    percentage: 0.5,
    revision: "456",
    updatedAt: "2026-09-12T00:00:00.000Z",
  },
  pageMapping: { ...appliedPageMigration },
  algorithmVersion: "comic-progress/v2",
  createdAt: "2026-09-12T00:00:00.000Z",
  undoable: true,
  appliedRevision: "456",
}

const appliedResult: ComicProgressMigrationResultDto = {
  status: "applied",
  matchResult: {
    kind: "same_logical_chapter_variant",
    confidence: "medium",
    progressMigration: "one_time",
    evidence: [...migrationEvidence],
  },
  pageMigration: { ...appliedPageMigration },
  snapshotId: "dddddddd-dddd-4ddd-8000-dddddddddddd",
  appliedRevision: "456",
  receipt: appliedReceipt,
}

const noProgressReceipt: ComicProgressMigrationReceiptDto = {
  migrationId: "eeeeeeee-eeee-4eee-8000-eeeeeeeeeeee",
  sourceMediaItemId: "source-media-1",
  targetMediaItemId: "target-media-1",
  strategy: "no_target",
  confidence: "low",
  evidence: [],
  sourceProgressSnapshot: null,
  targetProgressBefore: null,
  targetProgressAfter: null,
  pageMapping: {
    targetPageIndex: null,
    confidence: "low",
    strategy: "no_target",
    reversible: true,
  },
  algorithmVersion: "comic-progress/v2",
  createdAt: "2026-09-12T00:00:00.000Z",
  undoable: false,
  appliedRevision: null,
}

const noProgressResult: ComicProgressMigrationResultDto = {
  status: "no_source_progress",
  matchResult: null,
  pageMigration: {
    targetPageIndex: null,
    confidence: "low",
    strategy: "no_target",
    reversible: true,
  },
  snapshotId: null,
  appliedRevision: null,
  receipt: noProgressReceipt,
}

const nonAppliedStatuses: Array<ComicProgressMigrationResultDto["status"]> = [
  "unchanged",
  "not_applicable",
  "shared_content",
  "suggested",
  "target_progress_preserved",
  "no_target_page",
]

function withHiddenOwnKey<T extends object>(value: T): T {
  Object.defineProperty(value, "runtimeOnly", { value: "must-be-rejected", enumerable: false })
  return value
}

describe("comic progress migration wire guards", () => {
  it("accepts opaque source identities, an owner-bound remap request, and a complete applied result", () => {
    const request: ComicProgressMigrationRequestDto = {
      source,
      target,
      allowBestEffort: false,
      allowTargetOverwrite: false,
    }
    expect(isComicProgressMigrationRequestDto(request)).toBe(true)
    expect(isComicPageProgressRemapRequestDto(remapRequest)).toBe(true)
    expect(isComicProgressMigrationResultDto(appliedResult)).toBe(true)
    expect(isComicProgressMigrationRevertResultDto({ reverted: true })).toBe(true)
  })

  it("rejects URLs, unknown internal fields, and an applied result without rollback data", () => {
    expect(isComicProgressMigrationRequestDto({
      source: { ...source, remoteChapterId: "https://example.invalid/chapter" },
      target,
      allowBestEffort: false,
      allowTargetOverwrite: false,
    })).toBe(false)

    expect(isComicPageProgressRemapRequestDto({
      ...remapRequest,
      runtimeOnly: "must-be-rejected",
    })).toBe(false)

    expect(isComicProgressMigrationResultDto({
      ...appliedResult,
      snapshotId: null,
    })).toBe(false)
    expect(isComicProgressMigrationRevertResultDto({ reverted: "yes" })).toBe(false)
  })

  it("accepts every non-applied terminal state without inventing a snapshot", () => {
    for (const status of nonAppliedStatuses) {
      expect(isComicProgressMigrationResultDto({
        ...noProgressResult,
        status,
      })).toBe(true)
    }
  })

  it("rejects malformed CAS, identity, evidence, and page-map fields", () => {
    expect(isComicPageProgressRemapRequestDto({
      ...remapRequest,
      expectedRevision: "https://example.invalid/revision",
    })).toBe(false)

    expect(isComicProgressMigrationResultDto({
      ...appliedResult,
      snapshotId: "DDDDDDDD-DDDD-4DDD-8000-DDDDDDDDDDDD",
    })).toBe(false)
    expect(isComicProgressMigrationResultDto({
      ...appliedResult,
      appliedRevision: null,
    })).toBe(false)
    expect(isComicProgressMigrationResultDto({
      ...appliedResult,
      matchResult: {
        ...appliedResult.matchResult!,
        evidence: [{ kind: "edition_compatible", matched: 1 }],
      },
    })).toBe(false)

    expect(isComicPageProgressRemapRequestDto({
      ...remapRequest,
      sessionId: "CCCCCCCC-CCCC-4CCC-8000-CCCCCCCCCCCC",
    })).toBe(false)
  })

  it("rejects an unknown symbol own key on a request, a result, and nested objects", () => {
    const request: ComicProgressMigrationRequestDto = {
      source,
      target,
      allowBestEffort: false,
      allowTargetOverwrite: false,
    }
    expect(isComicProgressMigrationRequestDto(request)).toBe(true)
    expect(isComicProgressMigrationResultDto(appliedResult)).toBe(true)

    const symbolKey = Symbol("runtimeOnly")
    expect(isComicProgressMigrationRequestDto({
      ...request,
      [symbolKey]: "must-be-rejected",
    })).toBe(false)
    expect(isComicProgressMigrationResultDto({
      ...appliedResult,
      [symbolKey]: "must-be-rejected",
    })).toBe(false)

    expect(isComicProgressMigrationRequestDto({
      ...request,
      source: { ...source, [symbolKey]: "must-be-rejected" },
    })).toBe(false)
    expect(isComicProgressMigrationResultDto({
      ...appliedResult,
      pageMigration: { ...appliedResult.pageMigration, [symbolKey]: "must-be-rejected" },
    })).toBe(false)
  })

  it("rejects an unknown non-enumerable own key on a request, a result, and nested objects", () => {
    const request: ComicProgressMigrationRequestDto = {
      source: { ...source },
      target: { ...target },
      allowBestEffort: false,
      allowTargetOverwrite: false,
    }
    expect(isComicProgressMigrationRequestDto(request)).toBe(true)
    expect(isComicProgressMigrationRequestDto(withHiddenOwnKey({ ...request }))).toBe(false)
    expect(isComicProgressMigrationRequestDto({
      ...request,
      source: withHiddenOwnKey({ ...source }),
    })).toBe(false)
    expect(isComicProgressMigrationRequestDto({
      ...request,
      target: withHiddenOwnKey({ ...target }),
    })).toBe(false)

    expect(isComicProgressMigrationResultDto(appliedResult)).toBe(true)
    expect(isComicProgressMigrationResultDto(withHiddenOwnKey({ ...appliedResult }))).toBe(false)
    expect(isComicProgressMigrationResultDto({
      ...appliedResult,
      receipt: {
        ...appliedReceipt,
        pageMapping: withHiddenOwnKey({ ...appliedReceipt.pageMapping }),
      },
    })).toBe(false)
    expect(isComicProgressMigrationResultDto({
      ...appliedResult,
      matchResult: {
        ...appliedResult.matchResult!,
        evidence: [withHiddenOwnKey({ ...migrationEvidence[0] })],
      },
    })).toBe(false)
  })
})

describe("comic progress migration gateways", () => {
  it("calls the migration client and preserves the validated result", async () => {
    const comicProgressMigrate = vi.fn<HavenClient["comicProgressMigrate"]>()
      .mockResolvedValue(appliedResult)
    const request: ComicProgressMigrationRequestDto = {
      source,
      target,
      allowBestEffort: false,
      allowTargetOverwrite: false,
    }

    await expect(migrateComicProgress(request, { comicProgressMigrate })).resolves.toBe(appliedResult)
    expect(comicProgressMigrate).toHaveBeenCalledWith(request)
  })

  it("validates remap input before invoking the client", async () => {
    const comicProgressRemap = vi.fn<HavenClient["comicProgressRemap"]>()
    await expect(remapComicProgress({
      ...remapRequest,
      sessionId: "not-a-uuid",
    }, { comicProgressRemap })).rejects.toMatchObject({
      code: "INVALID_ARGUMENT",
      retryable: false,
    })
    expect(comicProgressRemap).not.toHaveBeenCalled()
  })

  it("rejects a malformed response and keeps a canonical HavenError", async () => {
    const comicProgressMigrate = vi.fn<HavenClient["comicProgressMigrate"]>()
      .mockResolvedValue({ ...noProgressResult, pageMigration: { targetPageIndex: 0 } } as never)
    await expect(migrateComicProgress({
      source,
      target,
      allowBestEffort: false,
      allowTargetOverwrite: false,
    }, { comicProgressMigrate })).rejects.toMatchObject({
      code: "COMIC_PROGRESS_MIGRATION_INVALID_RESPONSE",
      retryable: false,
    })

    const expected = new HavenError({
      code: "REVISION_CONFLICT",
      userMessage: "进度已变化",
      retryable: false,
    })
    const comicProgressRevert = vi.fn<HavenClient["comicProgressRevert"]>()
      .mockRejectedValue(expected)
    await expect(revertComicProgress({
      migrationId: appliedResult.snapshotId!,
      expectedAppliedRevision: appliedResult.appliedRevision!,
    }, { comicProgressRevert })).rejects.toBe(expected)
  })

  it("uses the explicit remap and revert commands", async () => {
    const comicProgressRemap = vi.fn<HavenClient["comicProgressRemap"]>()
      .mockResolvedValue(noProgressResult)
    await expect(remapComicProgress(remapRequest, { comicProgressRemap })).resolves.toBe(noProgressResult)
    expect(comicProgressRemap).toHaveBeenCalledWith(remapRequest)

    const reverted: ComicProgressMigrationRevertResultDto = { reverted: true }
    const comicProgressRevert = vi.fn<HavenClient["comicProgressRevert"]>()
      .mockResolvedValue(reverted)
    const request = {
      migrationId: appliedResult.snapshotId!,
      expectedAppliedRevision: appliedResult.appliedRevision!,
    }
    await expect(revertComicProgress(request, { comicProgressRevert })).resolves.toBe(reverted)
    expect(comicProgressRevert).toHaveBeenCalledWith(request)
  })
})
