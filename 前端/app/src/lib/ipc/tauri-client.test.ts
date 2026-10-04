import { beforeEach, describe, expect, it, vi } from "vitest"
import type {
  ComicChapterCatalogGetRequest,
  ComicRegisteredChapterCatalogDto,
  ComicChapterSourceCandidatesGetRequestDto,
  ComicProgressMigrationRequestDto,
  ComicPageProgressRemapRequestDto,
  ComicProgressMigrationRevertRequestDto,
  ComicPageManifestGetRequest,
  AiSettingsRecommendationGenerateRequest,
  AiSettingsRecommendationDto,
  ProgressSaveRequest,
  ReaderTocGetRequest,
} from "./generated/wire"
import type {
  HomeLayoutSaveRequestWire,
  OverviewLayoutSaveRequestWire,
  SettingsUpdateRequest,
} from "./settings-wire"
import { defaultHomeLayout, defaultOverviewLayout } from "./settings-wire"

const { invoke, check } = vi.hoisted(() => ({ invoke: vi.fn(), check: vi.fn() }))
vi.mock("@tauri-apps/api/core", () => ({
  Channel: class Channel<T> { onmessage?: (event: T) => void },
  invoke,
}))
vi.mock("@tauri-apps/plugin-updater", () => ({ check }))

import { TauriHavenClient } from "./tauri-client"

beforeEach(() => {
  invoke.mockReset()
})

describe("TauriHavenClient progress_save", () => {
  it("maps the request under the command's request argument", async () => {
    invoke.mockResolvedValueOnce({ revision: "revision-1" })
    const request: ProgressSaveRequest = {
      mediaItemId: "0196f0d2-0000-7000-8000-000000000000",
      locator: { version: 1, kind: "video", data: { positionMs: 1234 } },
      completion: "in_progress",
      expectedRevision: null,
    }
    await expect(new TauriHavenClient().progressSave(request)).resolves.toEqual({ revision: "revision-1" })
    expect(invoke).toHaveBeenCalledWith("progress_save", { request })
  })
})

describe("TauriHavenClient comic_page_manifest_get", () => {
  it("maps only the typed session request under the command request argument", async () => {
    const request: ComicPageManifestGetRequest = {
      sessionId: "0196f0d2-0000-7000-8000-000000000001",
    }
    const manifest = {
      schemaVersion: 1,
      sessionId: request.sessionId,
      mediaItemId: "0196f0d2-0000-7000-8000-000000000000",
      pageCount: 0,
      pages: [],
    } as const
    invoke.mockResolvedValueOnce(manifest)

    await expect(new TauriHavenClient().comicPageManifestGet(request)).resolves.toBe(manifest)
    expect(invoke).toHaveBeenCalledWith("comic_page_manifest_get", { request })
  })
})

describe("TauriHavenClient comic_chapter_catalog_get", () => {
  it("maps the source/work request under the command request argument", async () => {
    const request: ComicChapterCatalogGetRequest = {
      sourceId: "mangadex",
      remoteWorkId: "aaaaaaaa-aaaa-4aaa-8000-aaaaaaaaaaaa",
    }
    const catalog = {
      schemaVersion: 1,
      sourceId: request.sourceId,
      remoteWorkId: request.remoteWorkId,
      fetchedAt: "2026-09-04T00:00:00Z",
      total: 0,
      truncated: false,
      chapters: [],
    } as const
    invoke.mockResolvedValueOnce(catalog)

    await expect(new TauriHavenClient().comicChapterCatalogGet(request)).resolves.toBe(catalog)
    expect(invoke).toHaveBeenCalledWith("comic_chapter_catalog_get", { request })
  })
})

describe("TauriHavenClient comic_chapter_catalog_registered_get", () => {
  it("maps the persisted catalog request under the command request argument", async () => {
    const request: ComicChapterCatalogGetRequest = {
      sourceId: "mangadex",
      remoteWorkId: "aaaaaaaa-aaaa-4aaa-8000-aaaaaaaaaaaa",
    }
    const catalog: ComicRegisteredChapterCatalogDto = {
      schemaVersion: 1,
      sourceId: request.sourceId,
      remoteWorkId: request.remoteWorkId,
      refreshState: null,
      chapters: [],
    }
    invoke.mockResolvedValueOnce(catalog)

    await expect(new TauriHavenClient().comicChapterCatalogRegisteredGet(request)).resolves.toBe(catalog)
    expect(invoke).toHaveBeenCalledWith("comic_chapter_catalog_registered_get", { request })
  })
})

describe("TauriHavenClient comic_chapter_catalog_refresh", () => {
  it("maps the explicit refresh request under the command request argument", async () => {
    const request: ComicChapterCatalogGetRequest = {
      sourceId: "mangadex",
      remoteWorkId: "aaaaaaaa-aaaa-4aaa-8000-aaaaaaaaaaaa",
    }
    const catalog = {
      schemaVersion: 1,
      sourceId: request.sourceId,
      remoteWorkId: request.remoteWorkId,
      fetchedAt: "2026-09-04T00:00:00Z",
      total: 0,
      truncated: false,
      chapters: [],
    } as const
    invoke.mockResolvedValueOnce(catalog)

    await expect(new TauriHavenClient().comicChapterCatalogRefresh(request)).resolves.toBe(catalog)
    expect(invoke).toHaveBeenCalledWith("comic_chapter_catalog_refresh", { request })
  })
})

describe("TauriHavenClient comic_chapter_source_candidates_get", () => {
  it("passes the opaque source identity under the command request argument", async () => {
    const request: ComicChapterSourceCandidatesGetRequestDto = {
      source: {
        sourceId: "mangadex",
        remoteWorkId: "aaaaaaaa-aaaa-4aaa-8000-aaaaaaaaaaaa",
        remoteChapterId: "bbbbbbbb-bbbb-4bbb-8000-bbbbbbbbbbbb",
      },
    }
    const result = {
      schemaVersion: 1,
      source: request.source,
      currentMediaItemId: "cccccccc-cccc-4ccc-8000-cccccccccccc",
      candidates: [],
      truncated: false,
    } as const
    invoke.mockResolvedValueOnce(result)

    await expect(new TauriHavenClient().comicChapterSourceCandidatesGet(request)).resolves.toBe(result)
    expect(invoke).toHaveBeenCalledWith("comic_chapter_source_candidates_get", { request })
  })
})

describe("TauriHavenClient comic progress migration", () => {
  const source = {
    sourceId: "mangadex",
    remoteWorkId: "aaaaaaaa-aaaa-4aaa-8000-aaaaaaaaaaaa",
    remoteChapterId: "bbbbbbbb-bbbb-4bbb-8000-bbbbbbbbbbbb",
  }
  const target = {
    sourceId: "reader-ws",
    remoteWorkId: "reader-work-12",
    remoteChapterId: "reader-chapter-12",
  }

  it("passes the migration request under the command request argument", async () => {
    const request: ComicProgressMigrationRequestDto = {
      source,
      target,
      allowBestEffort: false,
      allowTargetOverwrite: false,
    }
    const result = {
      status: "applied",
      matchResult: null,
      pageMigration: {
        targetPageIndex: 1,
        confidence: "high",
        strategy: "stable_key",
        reversible: true,
      },
      snapshotId: "dddddddd-dddd-4ddd-8000-dddddddddddd",
      appliedRevision: "456",
    } as const
    invoke.mockResolvedValueOnce(result)

    await expect(new TauriHavenClient().comicProgressMigrate(request)).resolves.toBe(result)
    expect(invoke).toHaveBeenCalledWith("comic_progress_migrate", { request })
  })

  it("passes page remap and CAS-protected revert requests without reshaping them", async () => {
    const remap: ComicPageProgressRemapRequestDto = {
      sessionId: "cccccccc-cccc-4ccc-8000-cccccccccccc",
      expectedRevision: "123",
    }
    const migrationResult = {
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
    } as const
    invoke.mockResolvedValueOnce(migrationResult)
    await expect(new TauriHavenClient().comicProgressRemap(remap)).resolves.toBe(migrationResult)
    expect(invoke).toHaveBeenLastCalledWith("comic_progress_remap", { request: remap })

    const revert: ComicProgressMigrationRevertRequestDto = {
      migrationId: "dddddddd-dddd-4ddd-8000-dddddddddddd",
      expectedAppliedRevision: "456",
    }
    const revertResult = { reverted: true } as const
    invoke.mockResolvedValueOnce(revertResult)
    await expect(new TauriHavenClient().comicProgressRevert(revert)).resolves.toBe(revertResult)
    expect(invoke).toHaveBeenLastCalledWith("comic_progress_revert", { request: revert })
  })
})

describe("TauriHavenClient reader_toc_get", () => {
  it("maps only the typed session request under the command request argument", async () => {
    const request: ReaderTocGetRequest = {
      sessionId: "0196f0d2-0000-7000-8000-000000000001",
    }
    const result = {
      schemaVersion: 1,
      sessionId: request.sessionId,
      items: [
        { id: "a1b2c3d4e5f60718", title: "序言", depth: 0, progression: 0 },
      ],
    } as const
    invoke.mockResolvedValueOnce(result)

    await expect(new TauriHavenClient().readerTocGet(request)).resolves.toBe(result)
    expect(invoke).toHaveBeenCalledWith("reader_toc_get", { request })
  })
})

describe("TauriHavenClient settings", () => {
  it("maps settings_get and settings_update to their command argument shapes", async () => {
    const snapshot = {
      value: {
        section: "general",
        launchPage: "home",
        restoreSession: false,
        language: "zh_cn",
        notifications: true,
      },
      revision: null,
    }
    invoke.mockResolvedValueOnce(snapshot)

    const client = new TauriHavenClient()
    await expect(client.settingsGet("general")).resolves.toEqual(snapshot)
    expect(invoke).toHaveBeenLastCalledWith("settings_get", { section: "general" })

    const request: SettingsUpdateRequest = {
      section: "general",
      expectedRevision: null,
      patch: { section: "general", launchPage: "library" },
    }
    const result = {
      value: { ...snapshot.value, launchPage: "library" },
      revision: "settings-revision-1",
      changed: true,
    }
    invoke.mockResolvedValueOnce(result)

    await expect(client.settingsUpdate(request)).resolves.toEqual(result)
    expect(invoke).toHaveBeenLastCalledWith("settings_update", {
      section: "general",
      expectedRevision: null,
      patch: request.patch,
    })
  })

  it("rejects an invalid Settings response at the typed client boundary", async () => {
    invoke.mockResolvedValueOnce({ value: { section: "general" }, revision: null })

    await expect(new TauriHavenClient().settingsGet("general")).rejects.toMatchObject({
      code: "INTERNAL_ERROR",
      retryable: false,
    })
  })
})

describe("TauriHavenClient agent broker", () => {
  const DISABLED = {
    schemaVersion: 1,
    status: "disabled",
    endpoint: null,
    reason: null,
  } as const

  it("maps each command with no arguments at all", async () => {
    invoke.mockResolvedValue(DISABLED)

    const client = new TauriHavenClient()
    await expect(client.agentBrokerStatus()).resolves.toEqual(DISABLED)
    expect(invoke).toHaveBeenLastCalledWith("agent_broker_status")

    await expect(client.agentBrokerEnable()).resolves.toEqual(DISABLED)
    expect(invoke).toHaveBeenLastCalledWith("agent_broker_enable")

    await expect(client.agentBrokerDisable()).resolves.toEqual(DISABLED)
    expect(invoke).toHaveBeenLastCalledWith("agent_broker_disable")

    // 端点由 Rust 按平台解析：传输层不夹带 request / endpoint 等自由参数，
    // 也就没有人能从 WebView 指定要监听的地址。
    expect(invoke.mock.calls.map((call) => call.length)).toEqual([1, 1, 1])
  })

  it("passes the listening endpoint through and never rewrites fail-closed states", async () => {
    // 端点不是 secret（契约 §4.2）：它是可复制的本地配置值，原样透传。
    const listening = {
      schemaVersion: 1,
      status: "listening",
      endpoint: "\\\\.\\pipe\\haven-agent-v1-3f2a91c4",
      reason: null,
    } as const
    invoke.mockResolvedValueOnce(listening)
    const enabled = await new TauriHavenClient().agentBrokerEnable()
    // 端点逐字透传：传输层不解析、不重写、不隐藏这一段本地地址。
    expect(enabled).toBe(listening)
    expect(enabled.endpoint).toBe("\\\\.\\pipe\\haven-agent-v1-3f2a91c4")

    // busy / unavailable 是 Rust 的 fail-closed 结论。传输层若在此"顺手"降级成
    // disabled，用户就看不到端点被占用这件事。
    const busy = {
      schemaVersion: 1,
      status: "busy",
      endpoint: null,
      reason: "端点已被另一个栖阅实例占用",
    } as const
    invoke.mockResolvedValueOnce(busy)
    await expect(new TauriHavenClient().agentBrokerStatus()).resolves.toBe(busy)
  })

  it("normalizes contract error dtos and unknown rejections into HavenError", async () => {
    invoke.mockRejectedValueOnce({
      code: "HAVEN_BROKER_ENDPOINT_BUSY",
      userMessage: "端点已被另一个栖阅实例占用",
      retryable: false,
    })
    await expect(new TauriHavenClient().agentBrokerEnable())
      .rejects.toMatchObject({ code: "HAVEN_BROKER_ENDPOINT_BUSY", retryable: false })

    invoke.mockRejectedValueOnce(new Error("ipc exploded"))
    await expect(new TauriHavenClient().agentBrokerDisable())
      .rejects.toMatchObject({ code: "INTERNAL_ERROR", retryable: false })
  })
})

describe("TauriHavenClient 内置技能", () => {
  const SKILL = {
    schemaVersion: 1,
    skillId: "haven-agent-proposal",
    description: "读取脱敏上下文并创建待批准提案",
    instructionsChars: 5_800,
    state: "disabled",
  } as const

  it("列表无参数、启停只传 skillId 与布尔值", async () => {
    invoke.mockResolvedValueOnce({ schemaVersion: 1, skills: [SKILL] })
    await expect(new TauriHavenClient().agentSkillList()).resolves.toEqual({
      schemaVersion: 1,
      skills: [SKILL],
    })
    expect(invoke).toHaveBeenLastCalledWith("agent_skill_list")
    // 列表命令不接受任何参数：没有"按路径列出技能"这种自由输入。
    expect(invoke.mock.calls[invoke.mock.calls.length - 1].length).toBe(1)

    invoke.mockResolvedValueOnce({ ...SKILL, state: "enabled" })
    await new TauriHavenClient().agentSkillSetEnabled({ skillId: SKILL.skillId, enabled: true })
    // 传输层只能提交 id 与布尔值：没有正文、没有摘要、没有路径可以夹带。
    expect(invoke).toHaveBeenLastCalledWith("agent_skill_set_enabled", {
      request: { skillId: "haven-agent-proposal", enabled: true },
    })
  })

  it("在类型边界拒绝畸形响应，而不是把它当作合法状态渲染", async () => {
    // 多带一段技能正文 = 契约漂移（正文必须留在 Rust 侧）。
    invoke.mockResolvedValueOnce({
      schemaVersion: 1,
      skills: [{ ...SKILL, instructions: "任意正文" }],
    })
    await expect(new TauriHavenClient().agentSkillList())
      .rejects.toMatchObject({ code: "INTERNAL_ERROR" })

    // 三态之外的取值不得被当成"未知但可用"。
    invoke.mockResolvedValueOnce({ ...SKILL, state: "active" })
    await expect(new TauriHavenClient().agentSkillSetEnabled({ skillId: SKILL.skillId, enabled: true }))
      .rejects.toMatchObject({ code: "INTERNAL_ERROR" })
  })

  it("把契约错误与未知拒绝都归一成 HavenError", async () => {
    invoke.mockRejectedValueOnce({
      code: "AGENT_SKILL_UNKNOWN",
      userMessage: "没有这项内置技能",
      retryable: false,
    })
    await expect(new TauriHavenClient().agentSkillSetEnabled({ skillId: "nope", enabled: true }))
      .rejects.toMatchObject({ code: "AGENT_SKILL_UNKNOWN", retryable: false })

    invoke.mockRejectedValueOnce(new Error("ipc exploded"))
    await expect(new TauriHavenClient().agentSkillList())
      .rejects.toMatchObject({ code: "INTERNAL_ERROR", retryable: false })
  })
})

describe("TauriHavenClient AI 推荐生成", () => {
  const request: AiSettingsRecommendationGenerateRequest = {
    profileId: "provider-main",
    sessionId: "session-1",
    requestId: "request-1",
    userIntent: "把正文字号调大一级，不要改动其他项目",
    contextId: "context-1",
    contextHash: "a".repeat(64),
    baseRevision: "revision-1",
  }

  function response(): AiSettingsRecommendationDto {
    return {
      schemaVersion: 1,
      profileId: request.profileId,
      modelId: "reader-model",
      explanation: null,
      recommendedPatch: { fontSize: "large" },
      proposal: {
        schemaVersion: 1,
        proposalId: "proposal-1",
        status: "pending",
        subject: { section: "reading" },
        targetLabel: "全局阅读设置",
        baseRevision: "revision-1",
        digest: "b".repeat(64),
        createdAt: "2026-09-27T00:00:00Z",
        expiresAt: "2026-09-27T00:15:00Z",
        changes: [{ key: "reading.fontSize", before: "medium", after: "large" }],
      },
    }
  }

  it("把用户原话与最近一次上下文锚点通过固定命令载荷传入", async () => {
    const recommendation = response()
    invoke.mockResolvedValueOnce(recommendation)

    await expect(new TauriHavenClient().aiSettingsRecommendationGenerate(request)).resolves.toBe(recommendation)
    expect(invoke).toHaveBeenCalledWith("ai_settings_recommendation_generate", { request })
  })

  it("对缺字段、错 profile 或非待批准提案 fail closed", async () => {
    const malformed = [
      {},
      { ...response(), modelId: undefined },
      { ...response(), profileId: "another-profile" },
      { ...response(), proposal: { ...response().proposal, status: "applied" } },
      { ...response(), unexpected: "not in the wire contract" },
    ]

    for (const value of malformed) {
      invoke.mockResolvedValueOnce(value)
      await expect(new TauriHavenClient().aiSettingsRecommendationGenerate(request))
        .rejects.toMatchObject({ code: "INTERNAL_ERROR", retryable: false })
    }
  })
})

describe("TauriHavenClient updater", () => {
  beforeEach(() => {
    check.mockReset()
  })

  it("returns a redacted up_to_date result when no signed update is available", async () => {
    check.mockResolvedValueOnce(null)

    await expect(new TauriHavenClient().updateCheck()).resolves.toEqual({
      status: "up_to_date",
      currentVersion: null,
      availableVersion: null,
      releaseNotes: null,
      publishedAt: null,
    })
    expect(check).toHaveBeenCalledWith({ timeout: 15_000 })
  })

  it("keeps only bounded update metadata and installs the pending update", async () => {
    invoke.mockResolvedValue(undefined)
    const close = vi.fn().mockResolvedValue(undefined)
    const download = vi.fn().mockResolvedValue(undefined)
    const install = vi.fn().mockResolvedValue(undefined)
    check.mockResolvedValueOnce({
      currentVersion: "0.1.0-beta.1",
      version: "0.1.1",
      body: "修复\n\u0000泄漏",
      date: "2026-08-30T00:00:00Z",
      rawJson: { signature: "must-not-cross-boundary", body: "raw" },
      close,
      download,
      install,
    })

    const client = new TauriHavenClient()
    await expect(client.updateCheck()).resolves.toEqual({
      status: "available",
      currentVersion: "0.1.0-beta.1",
      availableVersion: "0.1.1",
      releaseNotes: "修复  泄漏",
      publishedAt: "2026-08-30T00:00:00Z",
    })
    await expect(client.updateInstall()).resolves.toEqual({ status: "installed" })
    expect(download).toHaveBeenCalledOnce()
    expect(invoke).toHaveBeenCalledWith("app_update_prepare")
    expect(invoke).toHaveBeenCalledWith("app_update_cancel")
    expect(install).toHaveBeenCalledWith({ restartAfterInstall: true })
    expect(close).toHaveBeenCalledOnce()
  })

  it("rejects install before a check without exposing updater internals", async () => {
    await expect(new TauriHavenClient().updateInstall()).rejects.toMatchObject({
      code: "UPDATER_NO_UPDATE",
      retryable: true,
    })
  })
})

describe("TauriHavenClient appearance commands", () => {
  const ASSET_ID = "0196f0d2-0000-7000-8000-00000000a301"

  it("maps the asset list request under the command's request argument", async () => {
    const payload = {
      schemaVersion: 1,
      assets: [
        { assetId: ASSET_ID, kind: "font", state: "validated", byteSize: 1024, displayName: "霞鹜文楷" },
      ],
    }
    invoke.mockResolvedValueOnce(payload)

    await expect(new TauriHavenClient().appearanceAssetsList({ kind: "font" })).resolves.toBe(payload)
    expect(invoke).toHaveBeenCalledWith("appearance_assets_list", { request: { kind: "font" } })
  })

  it("refuses a response that carries a path instead of an opaque asset id", async () => {
    invoke.mockResolvedValueOnce({
      schemaVersion: 1,
      assets: [
        {
          assetId: ASSET_ID,
          kind: "font",
          state: "validated",
          byteSize: 1024,
          displayName: null,
          path: "C:/Users/me/font.ttf",
        },
      ],
    })

    await expect(new TauriHavenClient().appearanceAssetsList({ kind: null }))
      .rejects.toMatchObject({ code: "INTERNAL_ERROR", retryable: false })
  })

  it("imports only the typed request (no path field) and surfaces a user cancel verbatim", async () => {
    invoke.mockRejectedValueOnce({
      code: "OPERATION_CANCELLED",
      userMessage: "已取消选择文件",
      retryable: false,
    })

    await expect(
      new TauriHavenClient().appearanceAssetImport({ kind: "static_wallpaper", displayName: "落日" }),
    ).rejects.toMatchObject({ code: "OPERATION_CANCELLED", retryable: false })

    expect(invoke).toHaveBeenCalledWith("appearance_asset_import", {
      request: { kind: "static_wallpaper", displayName: "落日" },
    })
    const [, args] = invoke.mock.calls[0] as [string, { request: Record<string, unknown> }]
    expect(Object.keys(args.request).sort()).toEqual(["displayName", "kind"])
  })

  it("deletes through the typed asset id request and keeps both facts separate", async () => {
    invoke.mockResolvedValueOnce({ deleted: true, fileRemoved: false })

    await expect(new TauriHavenClient().appearanceAssetDelete({ assetId: ASSET_ID }))
      .resolves.toEqual({ deleted: true, fileRemoved: false })
    expect(invoke).toHaveBeenCalledWith("appearance_asset_delete", { request: { assetId: ASSET_ID } })
  })

  it("reads the home layout with no arguments and guards the snapshot", async () => {
    const snapshot = { layout: { schemaVersion: 1, modules: [] }, revision: null }
    invoke.mockResolvedValueOnce(snapshot)

    await expect(new TauriHavenClient().homeLayoutGet()).resolves.toBe(snapshot)
    expect(invoke).toHaveBeenCalledWith("home_layout_get")
  })

  it("rejects a home layout response that is out of the fixed grid", async () => {
    invoke.mockResolvedValueOnce({
      layout: {
        schemaVersion: 1,
        modules: [{ module: "continue", size: "large", row: 0, column: 1, order: 0 }],
      },
      revision: "appearance-1",
    })

    await expect(new TauriHavenClient().homeLayoutGet())
      .rejects.toMatchObject({ code: "INTERNAL_ERROR" })
  })

  it("sends the expected revision and layout under the request argument", async () => {
    const request: HomeLayoutSaveRequestWire = {
      expectedRevision: "appearance-1",
      layout: { schemaVersion: 1, modules: [] },
    }
    invoke.mockResolvedValueOnce({
      layout: { schemaVersion: 1, modules: [] },
      revision: "appearance-2",
      changed: true,
    })

    await expect(new TauriHavenClient().homeLayoutSave(request))
      .resolves.toMatchObject({ changed: true, revision: "appearance-2" })
    expect(invoke).toHaveBeenCalledWith("home_layout_save", { request })
  })

  it("maps the reset CAS through its own command and normalizes the conflict error", async () => {
    invoke.mockRejectedValueOnce({
      code: "REVISION_CONFLICT",
      userMessage: "外观设置已被其他窗口更新，请重新加载后再保存",
      retryable: false,
    })

    await expect(new TauriHavenClient().homeLayoutReset({ expectedRevision: null }))
      .rejects.toMatchObject({ code: "REVISION_CONFLICT", retryable: false })
    expect(invoke).toHaveBeenCalledWith("home_layout_reset", { request: { expectedRevision: null } })
  })
})

describe("TauriHavenClient overview layout commands", () => {
  it("reads the overview layout with no arguments and guards the snapshot", async () => {
    const snapshot = { layout: defaultOverviewLayout(), revision: null }
    invoke.mockResolvedValueOnce(snapshot)

    await expect(new TauriHavenClient().overviewLayoutGet()).resolves.toBe(snapshot)
    expect(invoke).toHaveBeenCalledWith("overview_layout_get")
  })

  it("rejects a home layout response coming back from the overview command", async () => {
    // 两份布局各有自己的闭合模块集合：首页布局的形状不是合法的总览布局。
    invoke.mockResolvedValueOnce({ layout: defaultHomeLayout(), revision: "overview-1" })

    await expect(new TauriHavenClient().overviewLayoutGet())
      .rejects.toMatchObject({ code: "INTERNAL_ERROR" })
  })

  it("rejects an overview layout response that is out of the three-column grid", async () => {
    invoke.mockResolvedValueOnce({
      layout: {
        schemaVersion: 1,
        modules: [{ module: "preferences", size: "medium", row: 0, column: 2, order: 0 }],
      },
      revision: "overview-1",
    })

    await expect(new TauriHavenClient().overviewLayoutGet())
      .rejects.toMatchObject({ code: "INTERNAL_ERROR" })
  })

  it("sends the expected revision and layout under the request argument", async () => {
    const request: OverviewLayoutSaveRequestWire = {
      expectedRevision: "overview-1",
      layout: { schemaVersion: 1, modules: [] },
    }
    invoke.mockResolvedValueOnce({
      layout: { schemaVersion: 1, modules: [] },
      revision: "overview-2",
      changed: true,
    })

    await expect(new TauriHavenClient().overviewLayoutSave(request))
      .resolves.toMatchObject({ changed: true, revision: "overview-2" })
    expect(invoke).toHaveBeenCalledWith("overview_layout_save", { request })
  })

  it("maps the reset CAS through its own command and normalizes the conflict error", async () => {
    invoke.mockRejectedValueOnce({
      code: "REVISION_CONFLICT",
      userMessage: "外观设置已被其他窗口更新，请重新加载后再保存",
      retryable: false,
    })

    await expect(new TauriHavenClient().overviewLayoutReset({ expectedRevision: null }))
      .rejects.toMatchObject({ code: "REVISION_CONFLICT", retryable: false })
    expect(invoke).toHaveBeenCalledWith("overview_layout_reset", { request: { expectedRevision: null } })
  })
})

describe("TauriHavenClient reading overview command", () => {
  const REQUEST = { days: 7, utcOffsetMinutes: 480 }

  /** 空库的 wire 形态（`daily` 与窗口同长，可选统计量为 null）。 */
  function emptyOverview() {
    const localDates = [
      "2026-03-05",
      "2026-03-06",
      "2026-03-07",
      "2026-03-08",
      "2026-03-09",
      "2026-03-10",
      "2026-03-11",
    ]
    // ISO 星期序号：2026-03-05 是星期四。
    const weekdays = [4, 5, 6, 7, 1, 2, 3]
    return {
      schemaVersion: 1,
      range: {
        startLocalDate: localDates[0],
        endLocalDate: localDates[localDates.length - 1],
        days: 7,
        utcOffsetMinutes: 480,
      },
      sessionCount: 0,
      totalDurationMs: 0,
      daily: localDates.map((localDate, index) => ({
        localDate,
        weekday: weekdays[index],
        durationMs: 0,
      })),
      categories: [],
      heatmapCells: [],
      peakStartHour: null,
      peakEndHour: null,
      longestStreakDays: null,
      averageDailyDurationMs: null,
      recentWeekDurationMs: null,
    }
  }

  it("sends the window under the command's request argument", async () => {
    const payload = emptyOverview()
    invoke.mockResolvedValueOnce(payload)

    await expect(new TauriHavenClient().readingOverviewGet(REQUEST)).resolves.toBe(payload)
    expect(invoke).toHaveBeenCalledWith("reading_overview_get", { request: REQUEST })
    const [, args] = invoke.mock.calls[0] as [string, { request: Record<string, unknown> }]
    expect(Object.keys(args.request).sort()).toEqual(["days", "utcOffsetMinutes"])
  })

  it("rejects an overview that fabricates zeroed aggregates for an empty library", async () => {
    // 空库必须表达为 null（没有记录），把「没有数据」写成 0 分钟是伪造统计量。
    invoke.mockResolvedValueOnce({
      ...emptyOverview(),
      peakStartHour: 0,
      averageDailyDurationMs: 0,
    })

    await expect(new TauriHavenClient().readingOverviewGet(REQUEST))
      .rejects.toMatchObject({ code: "INTERNAL_ERROR", retryable: false })
  })

  it("rejects an overview whose daily buckets do not cover the whole window", async () => {
    const payload = emptyOverview()
    invoke.mockResolvedValueOnce({ ...payload, daily: payload.daily.slice(0, 3) })

    await expect(new TauriHavenClient().readingOverviewGet(REQUEST))
      .rejects.toMatchObject({ code: "INTERNAL_ERROR" })
  })

  it("normalizes the domain range error without inventing a fallback window", async () => {
    invoke.mockRejectedValueOnce({
      code: "READING_OVERVIEW_INVALID_RANGE",
      userMessage: "总览统计天数超出允许范围",
      retryable: false,
    })

    await expect(new TauriHavenClient().readingOverviewGet({ days: 0, utcOffsetMinutes: 0 }))
      .rejects.toMatchObject({ code: "READING_OVERVIEW_INVALID_RANGE", retryable: false })
  })
})

describe("TauriHavenClient tvbox_config_preview", () => {
  it("maps the address under the command's request argument", async () => {
    const request = { url: "https://config.example.invalid/tvbox.json" }
    const response = {
      schemaVersion: 1,
      siteCount: 2,
      liveCount: 0,
      parserCount: 0,
      skippedSiteRows: 0,
      skippedLiveRows: 0,
      skippedParserRows: 0,
      spiderConfigured: false,
      spiderKind: null,
      spiderHasIntegrityDigest: false,
      httpEndpointSiteCount: 2,
      spiderSiteCount: 0,
      unclassifiedSiteCount: 0,
      opaqueTopLevelFieldNames: [],
      unrecognizedTopLevelFieldCount: 0,
      withheldTopLevelFieldCount: 0,
    } as const
    invoke.mockResolvedValueOnce(response)

    await expect(new TauriHavenClient().tvboxConfigPreview(request)).resolves.toBe(response)
    expect(invoke).toHaveBeenCalledWith("tvbox_config_preview", { request })
  })

  it("keeps the stable backend error code and does not echo the address", async () => {
    invoke.mockRejectedValueOnce({
      code: "SECURITY_POLICY_DENIED",
      userMessage: "TVBox 配置地址不安全",
      retryable: false,
    })

    const rejection = await new TauriHavenClient()
      .tvboxConfigPreview({ url: "http://127.0.0.1/tvbox.json" })
      .catch((error: unknown) => error)
    expect(rejection).toMatchObject({ code: "SECURITY_POLICY_DENIED", retryable: false })
    expect(String((rejection as Error).message)).not.toContain("127.0.0.1")
  })
})

describe("TauriHavenClient tvbox_config_save", () => {
  it("maps the request under the command's request argument", async () => {
    const request = { displayName: "电视源", url: "https://config.example.invalid/tvbox.json" }
    const response = {
      schemaVersion: 1,
      sourceId: "custom_tvbox_0123456789ab",
      preview: {
        schemaVersion: 1,
        siteCount: 2,
        liveCount: 0,
        parserCount: 0,
        skippedSiteRows: 0,
        skippedLiveRows: 0,
        skippedParserRows: 0,
        spiderConfigured: false,
        spiderKind: null,
        spiderHasIntegrityDigest: false,
        httpEndpointSiteCount: 2,
        spiderSiteCount: 0,
        unclassifiedSiteCount: 0,
        opaqueTopLevelFieldNames: [],
        unrecognizedTopLevelFieldCount: 0,
        withheldTopLevelFieldCount: 0,
      },
    } as const
    invoke.mockResolvedValueOnce(response)

    await expect(new TauriHavenClient().tvboxConfigSave(request)).resolves.toBe(response)
    expect(invoke).toHaveBeenCalledWith("tvbox_config_save", { request })
  })

  it("keeps the stable backend error code and does not echo the address", async () => {
    invoke.mockRejectedValueOnce({
      code: "INVALID_ARGUMENT",
      userMessage: "该端点的自定义来源已存在",
      retryable: false,
    })

    const rejection = await new TauriHavenClient()
      .tvboxConfigSave({
        displayName: "电视源",
        url: "https://config.example.invalid/a.json?token=secret-value",
      })
      .catch((error: unknown) => error)
    expect(rejection).toMatchObject({ code: "INVALID_ARGUMENT", retryable: false })
    expect(String((rejection as Error).message)).not.toContain("secret-value")
  })
})
