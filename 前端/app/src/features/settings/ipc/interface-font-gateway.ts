// InterfaceFont Gateway（BE-INTERFACE-FONT-001）。
// Feature 只依赖统一 HavenClient；Runtime 决定 Tauri、显式 Mock 或不可用状态。
// 页面不直接调用 invoke/listen，也不接触任何文件路径。

import type {
  InterfaceFontAsset,
  InterfaceFontFamily,
  InterfaceFontImportResult,
} from "../../../lib/ipc/interface-font-wire.js";
import { getHavenClient } from "../../../lib/ipc/runtime.js";

export interface InterfaceFontGateway {
  /** 本机已安装字体族（只含族名）。 */
  systemList(): Promise<InterfaceFontFamily[]>;
  /** 已导入字体资产元数据（新的在前）。 */
  assetList(): Promise<InterfaceFontAsset[]>;
  /** Native 选择器导入；用户取消 → code `OPERATION_CANCELLED`。 */
  assetImport(): Promise<InterfaceFontImportResult>;
  /** 删除导入字体；正在使用 → code `FONT_ASSET_IN_USE`。 */
  assetDelete(assetId: string): Promise<void>;
}

/** Runtime-selected gateway; unavailable browser production fails at the client boundary. */
export const interfaceFontGateway: InterfaceFontGateway = {
  systemList: () => getHavenClient().interfaceFontSystemList(),
  assetList: () => getHavenClient().interfaceFontAssetList(),
  assetImport: () => getHavenClient().interfaceFontAssetImport(),
  assetDelete: (assetId) => getHavenClient().interfaceFontAssetDelete(assetId),
};
