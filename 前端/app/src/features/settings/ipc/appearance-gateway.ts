// Appearance Gateway（Appearance Stage 1B：设置页外观分组与首页布局的唯一数据通道）。
//
// 页面只依赖这里的 typed 方法，不直接 invoke、也不感知 Tauri/Mock 之分——运行时选择
// 由 lib/ipc/runtime 决定（浏览器生产环境在没有显式开关时 fail closed）。
//
// 边界：请求与响应都只有不透明资产 ID、typed DTO 与布局事实，没有任何参数或返回值
// 能承载路径、URL 或原始文件名；导入文件的来源只能由后端 Native 选择器决定。

import type { HavenClient } from "../../../lib/ipc/client.js";
import { getHavenClient, isTauriRuntime } from "../../../lib/ipc/runtime.js";
import type {
  AppearanceAssetDeleteRequestWire,
  AppearanceAssetDeleteResultWire,
  AppearanceAssetImportRequestWire,
  AppearanceAssetsListRequestWire,
  AppearanceAssetsWire,
  AppearanceAssetWire,
  HomeLayoutMutationResultWire,
  HomeLayoutResetRequestWire,
  HomeLayoutSaveRequestWire,
  HomeLayoutSnapshotWire,
  OverviewLayoutMutationResultWire,
  OverviewLayoutResetRequestWire,
  OverviewLayoutSaveRequestWire,
  OverviewLayoutSnapshotWire,
} from "../../../lib/ipc/settings-wire.js";

export interface AppearanceGateway {
  /** 列出外观资产（`kind` 为 null 表示不按种类过滤）。 */
  appearanceAssetsList(request: AppearanceAssetsListRequestWire): Promise<AppearanceAssetsWire>;
  /** 导入资产（文件由后端 Native 选择器选定；取消 → OPERATION_CANCELLED）。 */
  appearanceAssetImport(request: AppearanceAssetImportRequestWire): Promise<AppearanceAssetWire>;
  /** 删除资产；未知 ID 是幂等空操作（`deleted=false`）。 */
  appearanceAssetDelete(
    request: AppearanceAssetDeleteRequestWire,
  ): Promise<AppearanceAssetDeleteResultWire>;
  /** 读取首页布局；`revision=null` 表示从未保存过自定义布局。 */
  homeLayoutGet(): Promise<HomeLayoutSnapshotWire>;
  /** 保存首页布局（expectedRevision CAS）。 */
  homeLayoutSave(request: HomeLayoutSaveRequestWire): Promise<HomeLayoutMutationResultWire>;
  /** 重置首页布局（回到领域默认布局）。 */
  homeLayoutReset(request: HomeLayoutResetRequestWire): Promise<HomeLayoutMutationResultWire>;
  /**
   * 读取**设置页总览**布局；`revision=null` 表示从未保存过自定义布局。
   *
   * 与首页布局是两条独立通道（三列网格、另一组闭合模块、独立 revision）：设置页外观分区
   * 因此有「首页布局」与「总览布局」两个编辑器，各自读自己的快照。
   */
  overviewLayoutGet(): Promise<OverviewLayoutSnapshotWire>;
  /** 保存总览布局（expectedRevision CAS）。 */
  overviewLayoutSave(request: OverviewLayoutSaveRequestWire): Promise<OverviewLayoutMutationResultWire>;
  /** 重置总览布局（回到领域默认布局）。 */
  overviewLayoutReset(request: OverviewLayoutResetRequestWire): Promise<OverviewLayoutMutationResultWire>;
}

/** Runtime-selected gateway; unavailable browser production fails at the client boundary. */
export const appearanceGateway: AppearanceGateway = {
  appearanceAssetsList,
  appearanceAssetImport,
  appearanceAssetDelete,
  homeLayoutGet,
  homeLayoutSave,
  homeLayoutReset,
  overviewLayoutGet,
  overviewLayoutSave,
  overviewLayoutReset,
};

const CANONICAL_ASSET_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

/**
 * 将不透明资产 ID 映射为 Tauri 受控资源协议地址。
 *
 * 这里不接受路径、URL 或展示名；浏览器 Mock 没有字节资源，因此返回 null，让消费方
 * 走安全回退。只有 Tauri WebView 才会拿到 `haven-resource://appearance/<uuid>`。
 */
export function appearanceAssetResourceUri(assetId: string): string | null {
  if (!isTauriRuntime() || !CANONICAL_ASSET_ID.test(assetId)) return null;
  return `haven-resource://appearance/${assetId}`;
}

export async function appearanceAssetsList(
  request: AppearanceAssetsListRequestWire,
  client: HavenClient = getHavenClient(),
): Promise<AppearanceAssetsWire> {
  return client.appearanceAssetsList(request);
}

export async function appearanceAssetImport(
  request: AppearanceAssetImportRequestWire,
  client: HavenClient = getHavenClient(),
): Promise<AppearanceAssetWire> {
  return client.appearanceAssetImport(request);
}

export async function appearanceAssetDelete(
  request: AppearanceAssetDeleteRequestWire,
  client: HavenClient = getHavenClient(),
): Promise<AppearanceAssetDeleteResultWire> {
  return client.appearanceAssetDelete(request);
}

export async function homeLayoutGet(
  client: HavenClient = getHavenClient(),
): Promise<HomeLayoutSnapshotWire> {
  return client.homeLayoutGet();
}

export async function homeLayoutSave(
  request: HomeLayoutSaveRequestWire,
  client: HavenClient = getHavenClient(),
): Promise<HomeLayoutMutationResultWire> {
  return client.homeLayoutSave(request);
}

export async function homeLayoutReset(
  request: HomeLayoutResetRequestWire,
  client: HavenClient = getHavenClient(),
): Promise<HomeLayoutMutationResultWire> {
  return client.homeLayoutReset(request);
}

export async function overviewLayoutGet(
  client: HavenClient = getHavenClient(),
): Promise<OverviewLayoutSnapshotWire> {
  return client.overviewLayoutGet();
}

export async function overviewLayoutSave(
  request: OverviewLayoutSaveRequestWire,
  client: HavenClient = getHavenClient(),
): Promise<OverviewLayoutMutationResultWire> {
  return client.overviewLayoutSave(request);
}

export async function overviewLayoutReset(
  request: OverviewLayoutResetRequestWire,
  client: HavenClient = getHavenClient(),
): Promise<OverviewLayoutMutationResultWire> {
  return client.overviewLayoutReset(request);
}
