// 外观资产（字体 / 静态壁纸 / 动态壁纸）的列表与增删（Appearance Stage 1B）。
//
// 事实边界与 appearance-runtime 一致：资产身份只有不透明 `assetId`，导入的文件只能由
// 后端 Native 选择器选定——这里既没有路径参数，也不会在浏览器预览里伪造一份字节。
// Mock 运行时的 `appearanceAssetImport` 一律以 `OPERATION_CANCELLED` 失败，界面据此
// 显示「没有原生选择器」而不是假装导入成功。
//
// 删除的唯一守卫：**不允许删除当前正在使用的资产**。删掉之后运行时会立刻回落到默认
// 外观，用户却以为自己只清理了一个文件——所以这条规则放在提交之前，而不是事后回退。

import { useCallback, useEffect, useRef, useState } from "react"
import type {
  AppearanceAssetKindWire,
  AppearanceAssetsWire,
  AppearanceAssetWire,
  WallpaperSelection,
} from "@/lib/ipc/settings-wire"
import { toHavenError } from "@/lib/ipc/errors"
import type { AppearanceGateway } from "../ipc/appearance-gateway"
import { appearanceGateway } from "../ipc/appearance-gateway"
import { wallpaperAssetId } from "./appearance-runtime"

/** 后端取消选择文件时使用的稳定 code。 */
export const APPEARANCE_CANCELLED_CODE = "OPERATION_CANCELLED"

export interface AppearanceAssetLists {
  fonts: AppearanceAssetWire[]
  staticWallpapers: AppearanceAssetWire[]
  dynamicWallpapers: AppearanceAssetWire[]
}

export const EMPTY_APPEARANCE_ASSET_LISTS: AppearanceAssetLists = {
  fonts: [],
  staticWallpapers: [],
  dynamicWallpapers: [],
}

/** 一次列表结果 → 界面要的三组资产。未知 kind 不会出现在这里（守卫已拒绝）。 */
export function groupAppearanceAssets(assets: AppearanceAssetsWire): AppearanceAssetLists {
  const lists: AppearanceAssetLists = { fonts: [], staticWallpapers: [], dynamicWallpapers: [] }
  for (const asset of assets.assets) {
    if (asset.kind === "font") lists.fonts.push(asset)
    else if (asset.kind === "static_wallpaper") lists.staticWallpapers.push(asset)
    else lists.dynamicWallpapers.push(asset)
  }
  return lists
}

/** 当前外观设置正在使用的资产（字体 + 壁纸）。 */
export interface AppearanceAssetSelection {
  fontAssetId: string | null | undefined
  wallpaper: WallpaperSelection | undefined
}

/** 删除前必须同时考虑当前未保存草稿与数据库里最后一次确认的设置。 */
export interface AppearanceAssetDeleteSelections {
  draft: AppearanceAssetSelection
  /** null 表示尚未成功读取已保存设置；此时删除必须失败关闭。 */
  saved: AppearanceAssetSelection | null
}

export function selectedAppearanceAssetIds(selection: AppearanceAssetSelection): string[] {
  const ids: string[] = []
  if (typeof selection.fontAssetId === "string" && selection.fontAssetId.length > 0) {
    ids.push(selection.fontAssetId)
  }
  const wallpaperId = wallpaperAssetId(selection.wallpaper)
  if (wallpaperId) ids.push(wallpaperId)
  return ids
}

/** 删除被拦截时的可展示理由；可以删除时返回 null。 */
export function appearanceAssetDeleteBlockedReason(
  assetId: string,
  selections: AppearanceAssetDeleteSelections,
): string | null {
  if (selections.saved === null) {
    return "无法确认已保存的外观设置是否引用该资产，请先重新加载设置后再删除。"
  }
  if (selectedAppearanceAssetIds(selections.saved).includes(assetId)) {
    return "该资产仍被已保存的外观设置引用，请先保存其他字体或壁纸后再删除。"
  }
  if (selectedAppearanceAssetIds(selections.draft).includes(assetId)) {
    return "该资产正被未保存的外观设置引用，请先切换到其他字体或壁纸再删除。"
  }
  return null
}

export type AppearanceAssetActionResult =
  | { kind: "success"; message: string }
  | { kind: "cancelled"; message: string }
  | { kind: "blocked"; message: string }
  | { kind: "failure"; message: string }

/** 资产的可展示名称；没有名称时用类型说明，不把内部 ID 展示给用户。 */
export function appearanceAssetLabel(asset: AppearanceAssetWire): string {
  if (asset.displayName && asset.displayName.length > 0) return asset.displayName
  switch (asset.kind) {
    case "font":
      return "未命名字体"
    case "static_wallpaper":
      return "未命名图片壁纸"
    case "dynamic_wallpaper":
      return "未命名视频壁纸"
  }
}

/** 资产是否可用于运行时投影（未通过后端校验的资产不得被渲染）。 */
export function isUsableAppearanceAsset(asset: AppearanceAssetWire): boolean {
  return asset.state === "validated"
}

export async function importAppearanceAsset(
  gateway: AppearanceGateway,
  kind: AppearanceAssetKindWire,
): Promise<AppearanceAssetActionResult> {
  try {
    // 没有任何参数能承载文件位置：字节来源由后端 Native 选择器决定。
    const asset = await gateway.appearanceAssetImport({ kind, displayName: null })
    return { kind: "success", message: `已导入 ${appearanceAssetLabel(asset)}` }
  } catch (error) {
    const haven = toHavenError(error)
    if (haven.code === APPEARANCE_CANCELLED_CODE) {
      return { kind: "cancelled", message: "已取消选择文件，没有导入任何资产。" }
    }
    return { kind: "failure", message: haven.message }
  }
}

export async function deleteAppearanceAsset(
  gateway: AppearanceGateway,
  assetId: string,
  selections: AppearanceAssetDeleteSelections,
): Promise<AppearanceAssetActionResult> {
  const blocked = appearanceAssetDeleteBlockedReason(assetId, selections)
  if (blocked) return { kind: "blocked", message: blocked }
  try {
    const result = await gateway.appearanceAssetDelete({ assetId })
    if (!result.deleted) return { kind: "success", message: "该资产已不在列表中。" }
    // 登记行删掉而字节没清掉只是留下一个无引用的文件，不影响可用性；如实说明。
    return {
      kind: "success",
      message: result.fileRemoved ? "资产已删除" : "资产已从列表移除，文件清理未完成",
    }
  } catch (error) {
    return { kind: "failure", message: toHavenError(error).message }
  }
}

export type AppearanceAssetsState =
  | { status: "loading" }
  | { status: "ready"; lists: AppearanceAssetLists }
  | { status: "error"; message: string }

export interface AppearanceAssetsController {
  state: AppearanceAssetsState
  /** 最近一次导入/删除的结果（成功、取消、被拦截或失败）。 */
  action: AppearanceAssetActionResult | null
  /** 正在进行的操作；null 表示空闲。 */
  pending: "import" | "delete" | null
  reload: () => void
  importAsset: (kind: AppearanceAssetKindWire) => void
  deleteAsset: (assetId: string) => void
}

export function useAppearanceAssets(
  selections: AppearanceAssetDeleteSelections,
  gateway: AppearanceGateway = appearanceGateway,
): AppearanceAssetsController {
  const [state, setState] = useState<AppearanceAssetsState>({ status: "loading" })
  const [action, setAction] = useState<AppearanceAssetActionResult | null>(null)
  const [pending, setPending] = useState<"import" | "delete" | null>(null)
  const requestId = useRef(0)
  // 删除守卫要读最新的选择，但不必让 callbacks 随之重建（否则每次切换字体都会
  // 重新挂载导入/删除入口）。
  const selectionsRef = useRef(selections)
  selectionsRef.current = selections

  const load = useCallback(async (): Promise<void> => {
    const id = ++requestId.current
    setState({ status: "loading" })
    try {
      const assets = await gateway.appearanceAssetsList({ kind: null })
      if (id !== requestId.current) return
      setState({ status: "ready", lists: groupAppearanceAssets(assets) })
    } catch (error) {
      if (id !== requestId.current) return
      setState({ status: "error", message: toHavenError(error).message })
    }
  }, [gateway])

  useEffect(() => {
    void load()
  }, [load])

  const importAsset = useCallback((kind: AppearanceAssetKindWire) => {
    setPending("import")
    setAction(null)
    void importAppearanceAsset(gateway, kind)
      .then((result) => {
        setAction(result)
        if (result.kind === "success") return load()
        return undefined
      })
      .finally(() => setPending(null))
  }, [gateway, load])

  const deleteAsset = useCallback((assetId: string) => {
    setPending("delete")
    setAction(null)
    void deleteAppearanceAsset(gateway, assetId, selectionsRef.current)
      .then((result) => {
        setAction(result)
        if (result.kind === "success") return load()
        return undefined
      })
      .finally(() => setPending(null))
  }, [gateway, load])

  return { state, action, pending, reload: () => void load(), importAsset, deleteAsset }
}
