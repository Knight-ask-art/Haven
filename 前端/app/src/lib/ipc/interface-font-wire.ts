// InterfaceFont 最小 IPC 类型消费层（BE-INTERFACE-FONT-001）。
//
// 单一事实源：
//   - haven-domain/src/settings.rs（InterfaceFontMode；appearance 的界面字体字段）
//   - haven-domain/src/contracts.rs（SystemFontFamily / InterfaceFontAsset）
//   - src-tauri/src/commands/interface_font.rs（命令边界 DTO：camelCase + 运行时守卫）
//   - src-tauri/src/resource_protocol.rs（haven-resource://font/<opaque id>）
//
// 为什么与 `settings-wire.ts` 一样是手写镜像而不是生成物：
// ts-rs 生成物只覆盖 haven-application 的 DTO；命令层 DTO（例如 `SettingsChangedDto`）
// 一直由前端运行时守卫镜像。字体命令的 DTO 属于同一类边界形状，因此同样在这里镜像，
// 不修改 `generated/wire.ts`（生成物只能由生成器更新）。
//
// 禁止把 Domain Entity / DB Row / 文件路径当作 IPC 类型；secret 永不进入这里。

/** 界面字体模式（appearance.interfaceFontMode，snake_case 闭合枚举）。 */
export type InterfaceFontModeWire = "system" | "sans" | "serif" | "custom";

export const INTERFACE_FONT_MODES: readonly InterfaceFontModeWire[] = [
  "system",
  "sans",
  "serif",
  "custom",
];

/** 族名长度上限（与 haven-domain `MAX_INTERFACE_FONT_FAMILY_LEN` 一致）。 */
export const INTERFACE_FONT_FAMILY_MAX_CHARS = 120;

/** 允许导入的扩展名 / MIME（与迁移 045 的 CHECK 约束一致）。 */
export const INTERFACE_FONT_EXTENSIONS = ["ttf", "otf", "woff2"] as const;
export const INTERFACE_FONT_MIME_TYPES = ["font/ttf", "font/otf", "font/woff2"] as const;

/** 本机字体族（`localizedFamily` 为同族中文名，可为 null）。 */
export type InterfaceFontFamily = {
  family: string;
  localizedFamily: string | null;
};

/** 已导入字体资产元数据（不含字节、不含路径）。 */
export type InterfaceFontAsset = {
  id: string;
  familyName: string;
  fileName: string;
  extension: (typeof INTERFACE_FONT_EXTENSIONS)[number];
  mimeType: (typeof INTERFACE_FONT_MIME_TYPES)[number];
  byteSize: number;
  createdAt: number;
};

export type InterfaceFontImportResult = {
  asset: InterfaceFontAsset;
  deduplicated: boolean;
};

/**
 * 族名是否可安全拼进 CSS `font-family` 列表。
 *
 * 与 Rust 侧 `is_safe_interface_font_family` 同一规则：保存边界已校验，渲染端
 * 仍会再查一次——前端不因为「后端应该校验过」就把任意字符串拼进样式。
 */
export function isSafeInterfaceFontFamily(name: string): boolean {
  const trimmed = name.trim();
  if (trimmed.length === 0 || [...trimmed].length > INTERFACE_FONT_FAMILY_MAX_CHARS) return false;
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f"'.;{}\\]/.test(trimmed)) return false;
  if (/[<>]/.test(trimmed)) return false;
  return !trimmed.includes("/*");
}

const CANONICAL_UUID =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

/** opaque 资产 id 是否为规范形式（与 Rust `Uuid::to_string()` 可 round-trip）。 */
export function isCanonicalInterfaceFontAssetId(assetId: string): boolean {
  return CANONICAL_UUID.test(assetId);
}

/**
 * 导入字体的受控资源 URI（`haven-resource://font/<opaque id>`）。
 *
 * 只接受规范小写 UUID——与 `resource_protocol::parse_font_id` 同一判定；
 * 不合法时返回 null，调用方必须回落到非导入字体，而不是拼一个可能被拒绝的地址。
 * 这里**不**出现文件名或任何路径成分。
 */
export function interfaceFontResourceUri(assetId: string): string | null {
  return isCanonicalInterfaceFontAssetId(assetId) ? `haven-resource://font/${assetId}` : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isNonNegativeFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0;
}

export function guardInterfaceFontFamilyList(value: unknown): value is InterfaceFontFamily[] {
  return (
    Array.isArray(value) &&
    value.every(
      (entry) =>
        isRecord(entry) &&
        typeof entry.family === "string" &&
        entry.family.trim().length > 0 &&
        (entry.localizedFamily === null || typeof entry.localizedFamily === "string"),
    )
  );
}

export function guardInterfaceFontAsset(value: unknown): value is InterfaceFontAsset {
  if (!isRecord(value)) return false;
  return (
    typeof value.id === "string" &&
    CANONICAL_UUID.test(value.id) &&
    typeof value.familyName === "string" &&
    value.familyName.trim().length > 0 &&
    typeof value.fileName === "string" &&
    value.fileName.trim().length > 0 &&
    (INTERFACE_FONT_EXTENSIONS as readonly string[]).includes(String(value.extension)) &&
    (INTERFACE_FONT_MIME_TYPES as readonly string[]).includes(String(value.mimeType)) &&
    typeof value.byteSize === "number" &&
    Number.isInteger(value.byteSize) &&
    value.byteSize > 0 &&
    isNonNegativeFiniteNumber(value.createdAt)
  );
}

export function guardInterfaceFontAssetList(value: unknown): value is InterfaceFontAsset[] {
  return Array.isArray(value) && value.every(guardInterfaceFontAsset);
}

export function guardInterfaceFontImportResult(
  value: unknown,
): value is InterfaceFontImportResult {
  return (
    isRecord(value) &&
    guardInterfaceFontAsset(value.asset) &&
    typeof value.deduplicated === "boolean"
  );
}
