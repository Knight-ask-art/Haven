// @vitest-environment jsdom

import { act, cleanup, renderHook, waitFor } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"
import { HavenError } from "@/lib/ipc/errors"
import { MockHavenClient } from "@/lib/ipc/mock-client"
import type { AppearanceAssetWire } from "@/lib/ipc/settings-wire"
import type { AppearanceGateway } from "../ipc/appearance-gateway"
import {
  EMPTY_APPEARANCE_ASSET_LISTS,
  appearanceAssetDeleteBlockedReason,
  appearanceAssetLabel,
  deleteAppearanceAsset,
  groupAppearanceAssets,
  importAppearanceAsset,
  isUsableAppearanceAsset,
  selectedAppearanceAssetIds,
  useAppearanceAssets,
} from "./useAppearanceAssets"

const FONT_ID = "0196f0d2-0000-7000-8000-00000000f001"
const STATIC_ID = "0196f0d2-0000-7000-8000-00000000f002"
const DYNAMIC_ID = "0196f0d2-0000-7000-8000-00000000f003"

function asset(
  assetId: string,
  kind: AppearanceAssetWire["kind"],
  overrides: Partial<AppearanceAssetWire> = {},
): AppearanceAssetWire {
  return {
    assetId,
    kind,
    state: "validated",
    byteSize: 2048,
    displayName: null,
    ...overrides,
  }
}

function mockGateway(client: MockHavenClient = new MockHavenClient()): AppearanceGateway {
  return {
    appearanceAssetsList: (request) => client.appearanceAssetsList(request),
    appearanceAssetImport: (request) => client.appearanceAssetImport(request),
    appearanceAssetDelete: (request) => client.appearanceAssetDelete(request),
    homeLayoutGet: () => client.homeLayoutGet(),
    homeLayoutSave: (request) => client.homeLayoutSave(request),
    homeLayoutReset: (request) => client.homeLayoutReset(request),
    // 资产列表的用例不碰两条布局通道。
    overviewLayoutGet: () => client.overviewLayoutGet(),
    overviewLayoutSave: (request) => client.overviewLayoutSave(request),
    overviewLayoutReset: (request) => client.overviewLayoutReset(request),
  }
}

afterEach(cleanup)

describe("appearance asset lists", () => {
  it("groups the projection by kind and keeps an empty list empty", () => {
    expect(groupAppearanceAssets({ schemaVersion: 1, assets: [] })).toEqual(EMPTY_APPEARANCE_ASSET_LISTS)
    const lists = groupAppearanceAssets({
      schemaVersion: 1,
      assets: [asset(FONT_ID, "font"), asset(STATIC_ID, "static_wallpaper"), asset(DYNAMIC_ID, "dynamic_wallpaper")],
    })
    expect(lists.fonts.map((item) => item.assetId)).toEqual([FONT_ID])
    expect(lists.staticWallpapers.map((item) => item.assetId)).toEqual([STATIC_ID])
    expect(lists.dynamicWallpapers.map((item) => item.assetId)).toEqual([DYNAMIC_ID])
  })

  it("uses its display name or a friendly type label without exposing an internal id", () => {
    expect(appearanceAssetLabel(asset(FONT_ID, "font", { displayName: "思源宋体" }))).toBe("思源宋体")
    expect(appearanceAssetLabel(asset(FONT_ID, "font"))).toBe("未命名字体")
    expect(appearanceAssetLabel(asset(STATIC_ID, "static_wallpaper"))).toBe("未命名图片壁纸")
    expect(appearanceAssetLabel(asset(DYNAMIC_ID, "dynamic_wallpaper"))).toBe("未命名视频壁纸")
  })

  it("only treats a validated asset as usable", () => {
    expect(isUsableAppearanceAsset(asset(FONT_ID, "font"))).toBe(true)
    expect(isUsableAppearanceAsset(asset(FONT_ID, "font", { state: "pending" }))).toBe(false)
    expect(isUsableAppearanceAsset(asset(FONT_ID, "font", { state: "rejected" }))).toBe(false)
  })
})

describe("appearance asset delete guard", () => {
  it("names the assets currently in use", () => {
    expect(selectedAppearanceAssetIds({ fontAssetId: null, wallpaper: { kind: "none" } })).toEqual([])
    expect(selectedAppearanceAssetIds({
      fontAssetId: FONT_ID,
      wallpaper: { kind: "static", assetId: STATIC_ID },
    })).toEqual([FONT_ID, STATIC_ID])
    // 形状损坏的壁纸选择不算「正在使用」（它会回落成无壁纸）。
    expect(selectedAppearanceAssetIds({
      fontAssetId: null,
      wallpaper: { kind: "static", assetId: "not-a-uuid" },
    })).toEqual([])
  })

  it("blocks deleting the selected font or wallpaper", () => {
    const selections = {
      draft: { fontAssetId: FONT_ID, wallpaper: { kind: "dynamic" as const, assetId: DYNAMIC_ID } },
      saved: { fontAssetId: null, wallpaper: { kind: "none" as const } },
    }
    expect(appearanceAssetDeleteBlockedReason(FONT_ID, selections)).toContain("未保存")
    expect(appearanceAssetDeleteBlockedReason(DYNAMIC_ID, selections)).toContain("未保存")
    expect(appearanceAssetDeleteBlockedReason(STATIC_ID, selections)).toBeNull()
  })

  it("protects saved references even after the draft has switched away", () => {
    const selections = {
      draft: { fontAssetId: null, wallpaper: { kind: "none" as const } },
      saved: { fontAssetId: FONT_ID, wallpaper: { kind: "none" as const } },
    }

    expect(appearanceAssetDeleteBlockedReason(FONT_ID, selections)).toContain("已保存")
  })

  it("fails closed when the saved appearance selection is unknown", () => {
    const selections = {
      draft: { fontAssetId: null, wallpaper: { kind: "none" as const } },
      saved: null,
    }

    expect(appearanceAssetDeleteBlockedReason(FONT_ID, selections)).toContain("无法确认")
  })

  it("refuses the delete before it reaches the gateway", async () => {
    const gateway = mockGateway()
    const spy = vi.spyOn(gateway, "appearanceAssetDelete")
    const result = await deleteAppearanceAsset(gateway, FONT_ID, {
      draft: { fontAssetId: null, wallpaper: { kind: "none" } },
      saved: { fontAssetId: FONT_ID, wallpaper: { kind: "none" } },
    })
    expect(result.kind).toBe("blocked")
    expect(spy).not.toHaveBeenCalled()
  })
})

describe("appearance asset import and delete results", () => {
  it("reports a successful import", async () => {
    const gateway = mockGateway()
    vi.spyOn(gateway, "appearanceAssetImport").mockResolvedValue(
      asset(FONT_ID, "font", { displayName: "思源宋体" }),
    )
    const result = await importAppearanceAsset(gateway, "font")
    expect(result).toEqual({ kind: "success", message: "已导入 思源宋体" })
    expect(gateway.appearanceAssetImport).toHaveBeenCalledWith({ kind: "font", displayName: null })
  })

  it("distinguishes a cancelled picker from a real failure", async () => {
    const cancelled = mockGateway()
    vi.spyOn(cancelled, "appearanceAssetImport").mockImplementation(async () => {
      throw new HavenError({
        code: "OPERATION_CANCELLED",
        userMessage: "演示环境没有原生文件选择器，未选择任何文件",
        retryable: false,
      })
    })
    const cancelledResult = await importAppearanceAsset(cancelled, "font")
    expect(cancelledResult.kind).toBe("cancelled")
    expect(cancelledResult.message).toContain("已取消选择文件")

    const failing = mockGateway()
    vi.spyOn(failing, "appearanceAssetImport").mockImplementation(async () => {
      throw new HavenError({ code: "APPEARANCE_INVALID_ASSET", userMessage: "字体文件无法解析", retryable: false })
    })
    const failedResult = await importAppearanceAsset(failing, "font")
    expect(failedResult).toEqual({ kind: "failure", message: "字体文件无法解析" })
  })

  it("reports delete outcomes honestly, including a half-finished cleanup", async () => {
    const gateway = mockGateway()
    vi.spyOn(gateway, "appearanceAssetDelete").mockResolvedValue({ deleted: true, fileRemoved: false })
    const selections = {
      draft: { fontAssetId: null, wallpaper: { kind: "none" as const } },
      saved: { fontAssetId: null, wallpaper: { kind: "none" as const } },
    }
    const partial = await deleteAppearanceAsset(gateway, FONT_ID, selections)
    expect(partial.message).toContain("文件清理未完成")

    vi.spyOn(gateway, "appearanceAssetDelete").mockResolvedValue({ deleted: false, fileRemoved: false })
    const idempotent = await deleteAppearanceAsset(gateway, FONT_ID, selections)
    expect(idempotent.message).toContain("已不在列表中")
  })
})

describe("useAppearanceAssets", () => {
  it("loads the injected projection and keeps the demo runtime honest about import", async () => {
    const client = new MockHavenClient(false, {
      appearanceAssets: [asset(FONT_ID, "font"), asset(STATIC_ID, "static_wallpaper")],
    })
    const gateway = mockGateway(client)
    const selection = {
      draft: { fontAssetId: null, wallpaper: { kind: "none" as const } },
      saved: { fontAssetId: null, wallpaper: { kind: "none" as const } },
    }
    const { result } = renderHook(() => useAppearanceAssets(selection, gateway))
    await waitFor(() => expect(result.current.state.status).toBe("ready"))
    if (result.current.state.status === "ready") {
      expect(result.current.state.lists.fonts.map((item) => item.assetId)).toEqual([FONT_ID])
      expect(result.current.state.lists.staticWallpapers.map((item) => item.assetId)).toEqual([STATIC_ID])
      expect(result.current.state.lists.dynamicWallpapers).toEqual([])
    }

    await act(async () => { result.current.importAsset("font") })
    await waitFor(() => expect(result.current.pending).toBeNull())
    // 演示环境没有原生选择器：界面得到的是一句明确说明，而不是一次假导入。
    expect(result.current.action?.kind).toBe("cancelled")
  })

  it("deletes an unused asset and refuses the selected one", async () => {
    const client = new MockHavenClient(false, {
      appearanceAssets: [asset(FONT_ID, "font"), asset(STATIC_ID, "static_wallpaper")],
    })
    const gateway = mockGateway(client)
    const selection = {
      draft: { fontAssetId: FONT_ID, wallpaper: { kind: "none" as const } },
      saved: { fontAssetId: FONT_ID, wallpaper: { kind: "none" as const } },
    }
    const { result } = renderHook(() => useAppearanceAssets(selection, gateway))
    await waitFor(() => expect(result.current.state.status).toBe("ready"))

    await act(async () => { result.current.deleteAsset(FONT_ID) })
    await waitFor(() => expect(result.current.pending).toBeNull())
    expect(result.current.action?.kind).toBe("blocked")
    if (result.current.state.status === "ready") {
      expect(result.current.state.lists.fonts.map((item) => item.assetId)).toEqual([FONT_ID])
    }

    await act(async () => { result.current.deleteAsset(STATIC_ID) })
    await waitFor(() => expect(result.current.pending).toBeNull())
    expect(result.current.action?.kind).toBe("success")
    await waitFor(() => {
      if (result.current.state.status !== "ready") throw new Error("still loading")
      expect(result.current.state.lists.staticWallpapers).toEqual([])
    })
  })

  it("surfaces a list failure without inventing an empty library", async () => {
    const gateway = mockGateway()
    vi.spyOn(gateway, "appearanceAssetsList").mockImplementation(async () => {
      throw new HavenError({ code: "DATABASE_ERROR", userMessage: "资产列表读取失败", retryable: true })
    })
    const selection = {
      draft: { fontAssetId: null, wallpaper: { kind: "none" as const } },
      saved: { fontAssetId: null, wallpaper: { kind: "none" as const } },
    }
    const { result } = renderHook(() => useAppearanceAssets(selection, gateway))
    await waitFor(() => expect(result.current.state.status).toBe("error"))
    if (result.current.state.status === "error") {
      expect(result.current.state.message).toBe("资产列表读取失败")
    }
  })
})
