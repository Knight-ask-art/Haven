// 界面字体应用逻辑（BE-INTERFACE-FONT-001）：预设字体栈、导入字体 @font-face、
// 可搜索的字体列表与「应用到整个界面」的单一解析入口。
//
// 纯函数、无 React、无 IPC：AppShell 的运行时消费者与设置页预览调用**同一个**
// `resolveInterfaceFont`，因此「预览看到的」与「实际生效的」不可能漂移。
//
// 安全边界：族名只允许来自 `isSafeInterfaceFontFamily` 通过的字符串；导入字体
// 只通过 opaque id 生成的受控资源地址引用，任何文件名或路径都不参与 CSS。

import type { AppearanceSettingsValue } from "../../../lib/ipc/settings-wire.js";
import type { InterfaceFontAsset, InterfaceFontFamily } from "../../../lib/ipc/interface-font-wire.js";
import {
  interfaceFontResourceUri,
  isSafeInterfaceFontFamily,
} from "../../../lib/ipc/interface-font-wire.js";
import { fontSearchKeys, normalizeFontQuery } from "./pinyin.js";

/**
 * 预设字体栈（外观 → 界面字体）。
 *
 * 每个栈都以 CJK 系统字体收尾：预设只决定**优先次序与风格**，
 * 不让任何一类字形因为选了某个预设而消失。
 */
export const INTERFACE_FONT_PRESET_STACKS = {
  /** 现代黑体：更紧的字面、更高的 x-height。 */
  sans: '"Inter", "Segoe UI Variable", "Helvetica Neue", Arial, "Microsoft YaHei UI", "PingFang SC", "Noto Sans CJK SC", system-ui, sans-serif',
  /** 人文衬线：正文阅读气质，中文优先宋体系。 */
  serif: '"Source Han Serif SC", "Noto Serif CJK SC", "Songti SC", "SimSun", Georgia, "Times New Roman", serif',
} as const;

/** 自定义字体后面追加的兜底栈（缺字时仍按既有观感回退，不出现豆腐块）。 */
const CUSTOM_FALLBACK_STACK =
  '"Microsoft YaHei UI", "PingFang SC", "Noto Sans CJK SC", "Inter", system-ui, sans-serif';

/** 扩展名 → CSS `format()` 关键字（闭合集合，与服务端允许的三种一致）。 */
const FONT_FORMAT_BY_EXTENSION: Record<InterfaceFontAsset["extension"], string> = {
  ttf: "truetype",
  otf: "opentype",
  woff2: "woff2",
};

/** 导入字体在 CSS 里的族名：由 opaque id 派生，避免与用户本机字体同名冲突。 */
export function importedFontCssFamily(assetId: string): string {
  return `HavenImportedFont${assetId.replace(/-/g, "")}`;
}

/**
 * 解析结果：`fontFamily` 为 null 表示**不覆盖**既有字体栈（system 模式，
 * 或自定义选择已经失效时回退），`fontFaceCss` 为需要注入的 `@font-face` 规则。
 */
export type ResolvedInterfaceFont = {
  /** 直接写进 `--ds-font-interface` 的值；null = 移除该变量。 */
  fontFamily: string | null
  /** 需要注入到 `<style>` 的 `@font-face` 文本；null = 不需要。 */
  fontFaceCss: string | null
  /**
   * 稳定的可读标识，用于 `data-haven-interface-font` 与测试断言。
   * - `system` / `preset:sans` / `preset:serif`
   * - `local:<族名>` / `asset:<id>`
   * - `stale-asset:<id>`：设置引用的导入字体已不存在（必须拒绝应用）
   * - `invalid-family`：族名未通过安全校验（必须拒绝应用）
   */
  appliedKey: string
}

export type InterfaceFontSelection = Pick<
  AppearanceSettingsValue,
  "interfaceFontMode" | "interfaceFontFamily" | "interfaceFontAssetId"
>

/**
 * 唯一的「当前界面字体是什么」解析入口。
 *
 * `assets` 是当前已知的导入资产列表；`resolveUri` 把 `haven-resource://font/<id>`
 * 转成 WebView 可请求地址（Windows 需要 `http://haven-resource.font/<id>` 兼容形态）。
 *
 * 已删除（stale）的资产 id 与不合法的族名都**不会**被应用：宁可回退到系统字体栈，
 * 也不把界面置于一个取不到字节的字体上。
 */
export function resolveInterfaceFont(
  selection: InterfaceFontSelection,
  assets: readonly InterfaceFontAsset[],
  resolveUri: (uri: string) => string = (uri) => uri,
): ResolvedInterfaceFont {
  const mode = selection.interfaceFontMode;
  if (mode === "sans" || mode === "serif") {
    return {
      fontFamily: INTERFACE_FONT_PRESET_STACKS[mode],
      fontFaceCss: null,
      appliedKey: `preset:${mode}`,
    };
  }
  if (mode !== "custom") {
    return { fontFamily: null, fontFaceCss: null, appliedKey: "system" };
  }

  const assetId = selection.interfaceFontAssetId;
  if (assetId !== undefined) {
    const asset = assets.find((candidate) => candidate.id === assetId);
    if (!asset) {
      // 引用了一个当前不存在的资产：这是失效引用，明确拒绝应用。
      return { fontFamily: null, fontFaceCss: null, appliedKey: `stale-asset:${assetId}` };
    }
    const uri = interfaceFontResourceUri(asset.id);
    if (uri === null) {
      return { fontFamily: null, fontFaceCss: null, appliedKey: `stale-asset:${assetId}` };
    }
    const cssFamily = importedFontCssFamily(asset.id);
    const format = FONT_FORMAT_BY_EXTENSION[asset.extension];
    return {
      fontFamily: `"${cssFamily}", ${CUSTOM_FALLBACK_STACK}`,
      fontFaceCss: `@font-face{font-family:"${cssFamily}";src:url("${resolveUri(uri)}") format("${format}");font-display:swap;}`,
      appliedKey: `asset:${asset.id}`,
    };
  }

  const family = selection.interfaceFontFamily?.trim();
  if (family !== undefined && family.length > 0) {
    if (!isSafeInterfaceFontFamily(family)) {
      // 安全校验不通过：不拼进 CSS（保存边界也会拒绝它，这里是渲染端的第二道门）。
      return { fontFamily: null, fontFaceCss: null, appliedKey: "invalid-family" };
    }
    return {
      fontFamily: `"${family}", ${CUSTOM_FALLBACK_STACK}`,
      fontFaceCss: null,
      appliedKey: `local:${family}`,
    };
  }

  // custom 但还没选任何字体：保持既有字体栈，不伪造一个字体。
  return { fontFamily: null, fontFaceCss: null, appliedKey: "system" };
}

/**
 * 列表行「用该字体自己渲染」的内联样式。
 *
 * 族名不合法时返回 undefined（退回默认字体），绝不把任意字符串交给 CSS。
 */
export function fontFamilyPreviewStyle(family: string): { fontFamily: string } | undefined {
  const trimmed = family.trim();
  if (!isSafeInterfaceFontFamily(trimmed)) return undefined;
  return { fontFamily: `"${trimmed}", ${CUSTOM_FALLBACK_STACK}` };
}

const fontFamilySearchKeyCache = new WeakMap<InterfaceFontFamily, string[]>();
const fontAssetSearchKeyCache = new WeakMap<InterfaceFontAsset, string[]>();

/** 一个本机字体族的全部搜索键（英文名 + 中文名 + 拼音全拼 + 拼音首字母）。 */
export function fontFamilySearchKeys(entry: InterfaceFontFamily): string[] {
  const cached = fontFamilySearchKeyCache.get(entry);
  if (cached) return cached;
  const keys = fontSearchKeys([entry.family, entry.localizedFamily ?? ""].join(" "));
  fontFamilySearchKeyCache.set(entry, keys);
  return keys;
}

/**
 * 按查询过滤本机字体族。
 *
 * 空查询返回全部；匹配是「键包含归一化查询」。英文名与中文原名始终可搜，
 * 中文拼音键由 pinyin-pro 生成，不需要维护手写汉字覆盖表。
 */
export function filterFontFamilies(
  families: readonly InterfaceFontFamily[],
  query: string,
): InterfaceFontFamily[] {
  const normalized = normalizeFontQuery(query);
  if (normalized.length === 0) return [...families];
  return families.filter((entry) =>
    fontFamilySearchKeys(entry).some((key) => key.includes(normalized)),
  );
}

/** 导入字体资产的展示名（与资产元数据同名，单独成函数便于搜索复用）。 */
export function interfaceFontAssetSearchKeys(asset: InterfaceFontAsset): string[] {
  const cached = fontAssetSearchKeyCache.get(asset);
  if (cached) return cached;
  const keys = [
    ...fontSearchKeys(asset.familyName),
    ...fontSearchKeys(asset.fileName),
    ...fontSearchKeys(asset.id),
  ];
  fontAssetSearchKeyCache.set(asset, keys);
  return keys;
}

/** 按查询过滤已导入字体（族名 / 文件名 / opaque id）。 */
export function filterInterfaceFontAssets(
  assets: readonly InterfaceFontAsset[],
  query: string,
): InterfaceFontAsset[] {
  const normalized = normalizeFontQuery(query);
  if (normalized.length === 0) return [...assets];
  return assets.filter((asset) =>
    interfaceFontAssetSearchKeys(asset).some((key) => key.includes(normalized)),
  );
}

/**
 * 解析结果的**诚实**中文说明（设置页展示，不美化状态）。
 *
 * 失效引用与非法族名必须显式说出来：界面已经回退到系统字体，
 * 静默回退会让用户以为设置生效了。
 */
export function describeResolvedInterfaceFont(
  resolved: ResolvedInterfaceFont,
  assets: readonly InterfaceFontAsset[],
): string {
  if (resolved.appliedKey === "system") return "预览：系统默认字体栈";
  if (resolved.appliedKey === "invalid-family") {
    return "预览已回退到系统字体；请重新选择有效的字体名称";
  }
  if (resolved.appliedKey.startsWith("stale-asset:")) {
    return "所选导入字体已不存在，预览已回退；请重新选择或重新导入";
  }
  if (resolved.appliedKey.startsWith("asset:")) {
    const id = resolved.appliedKey.slice("asset:".length);
    const asset = assets.find((candidate) => candidate.id === id);
    return asset ? `预览：导入字体「${asset.familyName}」` : "预览：导入字体";
  }
  if (resolved.appliedKey.startsWith("local:")) {
    return `预览：本机字体「${resolved.appliedKey.slice("local:".length)}」`;
  }
  if (resolved.appliedKey === "preset:sans") return "预览：现代黑体";
  if (resolved.appliedKey === "preset:serif") return "预览：人文衬线";
  return "预览：系统默认字体栈";
}
