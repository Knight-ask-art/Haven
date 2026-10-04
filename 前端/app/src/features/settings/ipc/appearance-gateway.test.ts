import { describe, expect, it, vi } from "vitest"
import type { HavenClient } from "@/lib/ipc/client"
import {
  appearanceAssetDelete,
  appearanceAssetImport,
  appearanceAssetResourceUri,
  appearanceAssetsList,
  appearanceGateway,
  homeLayoutGet,
  homeLayoutReset,
  homeLayoutSave,
  overviewLayoutGet,
  overviewLayoutReset,
  overviewLayoutSave,
} from "./appearance-gateway"

const ASSET_ID = "0196f0d2-0000-7000-8000-00000000a401"

describe("appearance gateway", () => {
  it("forwards every typed appearance call to the injected client", async () => {
    const client = {
      appearanceAssetsList: vi.fn(async () => ({ schemaVersion: 1, assets: [] })),
      appearanceAssetImport: vi.fn(async () => ({
        assetId: ASSET_ID,
        kind: "font",
        state: "validated",
        byteSize: 1,
        displayName: null,
      })),
      appearanceAssetDelete: vi.fn(async () => ({ deleted: true, fileRemoved: true })),
      homeLayoutGet: vi.fn(async () => ({
        layout: { schemaVersion: 1, modules: [] },
        revision: null,
      })),
      homeLayoutSave: vi.fn(async () => ({
        layout: { schemaVersion: 1, modules: [] },
        revision: "appearance-1",
        changed: true,
      })),
      homeLayoutReset: vi.fn(async () => ({
        layout: { schemaVersion: 1, modules: [] },
        revision: null,
        changed: true,
      })),
      overviewLayoutGet: vi.fn(async () => ({
        layout: { schemaVersion: 1, modules: [] },
        revision: null,
      })),
      overviewLayoutSave: vi.fn(async () => ({
        layout: { schemaVersion: 1, modules: [] },
        revision: "overview-1",
        changed: true,
      })),
      overviewLayoutReset: vi.fn(async () => ({
        layout: { schemaVersion: 1, modules: [] },
        revision: null,
        changed: true,
      })),
    } as unknown as HavenClient

    await expect(appearanceAssetsList({ kind: "font" }, client)).resolves.toEqual({
      schemaVersion: 1,
      assets: [],
    })
    await appearanceAssetImport({ kind: "font", displayName: null }, client)
    await appearanceAssetDelete({ assetId: ASSET_ID }, client)
    await homeLayoutGet(client)
    await homeLayoutSave({ expectedRevision: null, layout: { schemaVersion: 1, modules: [] } }, client)
    await homeLayoutReset({ expectedRevision: "appearance-1" }, client)
    await overviewLayoutGet(client)
    await overviewLayoutSave({ expectedRevision: null, layout: { schemaVersion: 1, modules: [] } }, client)
    await overviewLayoutReset({ expectedRevision: "overview-1" }, client)

    expect(client.appearanceAssetsList).toHaveBeenCalledWith({ kind: "font" })
    expect(client.appearanceAssetImport).toHaveBeenCalledWith({ kind: "font", displayName: null })
    expect(client.appearanceAssetDelete).toHaveBeenCalledWith({ assetId: ASSET_ID })
    expect(client.homeLayoutGet).toHaveBeenCalledWith()
    expect(client.homeLayoutSave).toHaveBeenCalledWith({
      expectedRevision: null,
      layout: { schemaVersion: 1, modules: [] },
    })
    expect(client.homeLayoutReset).toHaveBeenCalledWith({ expectedRevision: "appearance-1" })
    // 总览布局走的是**另一个** client 方法，不会被转发到首页布局那一对命令上。
    expect(client.overviewLayoutGet).toHaveBeenCalledWith()
    expect(client.overviewLayoutSave).toHaveBeenCalledWith({
      expectedRevision: null,
      layout: { schemaVersion: 1, modules: [] },
    })
    expect(client.overviewLayoutReset).toHaveBeenCalledWith({ expectedRevision: "overview-1" })
    expect(client.homeLayoutSave).toHaveBeenCalledTimes(1)
    expect(client.homeLayoutReset).toHaveBeenCalledTimes(1)
  })

  it("exposes only the typed appearance methods (no invoke passthrough)", () => {
    // 页面拿到的是闭合的方法集合：没有自由的 invoke(command, args) 入口，
    // 因此也不可能从 Feature 层递交裸路径或任意命令名。
    expect(Object.keys(appearanceGateway).sort()).toEqual([
      "appearanceAssetDelete",
      "appearanceAssetImport",
      "appearanceAssetsList",
      "homeLayoutGet",
      "homeLayoutReset",
      "homeLayoutSave",
      "overviewLayoutGet",
      "overviewLayoutReset",
      "overviewLayoutSave",
    ])
  })

  it("does not manufacture resource URLs in browser/mock runtime", () => {
    expect(appearanceAssetResourceUri(ASSET_ID)).toBeNull()
    expect(appearanceAssetResourceUri("C:/Users/secret/font.ttf")).toBeNull()
  })
})
