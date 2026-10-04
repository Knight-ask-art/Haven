// 输入 schema（Zod，全部 `.strict()`）。
//
// 规范：docs/architecture/AI_SYSTEM.md §9；mcp-builder 规范（strict Zod + snake_case）。
//
// 这个文件是**契约镜像**：枚举值与 `前端/app/src/lib/ipc/generated/wire.ts` 里
// `PreferenceReadingPatchDto` / `PreferenceComicPatchDto` 的联合类型逐一对应。
// `test/wire-drift.test.ts` 直接读那份生成物做对照，因此任何一侧漂移都会失败——
// 一个凭空发明的枚举值会让后端拒绝整份提案，而模型会以为是 Haven 的问题。
//
// `.strictObject` 而不是 `.object`：Zod 的 `.object()` 默认**丢弃**未知键。丢弃意味着
// "模型传了 `apply: true` 而服务器静默忽略"，那是最坏的一种失败——调用方以为自己的
// 意图被接受了。这里必须让未知键变成显式错误。

import { z } from "zod";

import { DEFAULT_LIST_ITEMS, MAX_LIST_ITEMS, MAX_PATCH_KEYS, RESPONSE_FORMATS } from "./constants.js";

/** UUID 形状。不用 `z.uuid()`，避免 Zod 版本间 API 变动影响冻结契约。 */
export const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
/** canonical digest：64 位**小写**十六进制（与领域层 `is_canonical_digest` 同规则）。 */
export const DIGEST_PATTERN = /^[0-9a-f]{64}$/;

const uuid = (label: string) =>
  z.string().regex(UUID_PATTERN, `${label} 必须是 UUID（8-4-4-4-12 小写十六进制）`);

const digest = (label: string) =>
  z.string().regex(DIGEST_PATTERN, `${label} 必须是 64 位小写十六进制 SHA-256`);

// ---- 与生成 wire 一一对应的闭合枚举 ----
//
// 导出成常量数组而不是内联字面量：`test/wire-drift.test.ts` 要拿它们和
// `前端/app/src/lib/ipc/generated/wire.ts` 的联合类型逐项比对。内联的话，
// 那条测试就只能去解析 TypeScript 源码，脆且容易自我印证。

export const READING_FONT_FAMILY_VALUES = ["sans", "serif", "kai", "heiti", "fangsong", "mianfei", "custom"] as const;
export const READING_FONT_SIZE_VALUES = ["small", "medium", "large"] as const;
export const READING_LINE_HEIGHT_VALUES = ["compact", "comfortable", "airy"] as const;
export const READING_CONTENT_WIDTH_VALUES = ["narrow", "medium", "wide"] as const;
export const READING_FONT_WEIGHT_VALUES = ["light", "regular", "medium", "semibold", "bold"] as const;
export const READING_LETTER_SPACING_VALUES = ["tight", "normal", "relaxed", "loose"] as const;
// 注意：'eyeCare' 是协议里的真实取值（camelCase）——不要"顺手"改成 snake_case。
export const READING_THEME_VALUES = ["system", "paper", "warm", "slate", "dark", "sepia", "eyeCare", "custom"] as const;
export const READING_PAGINATION_VALUES = ["scroll", "paginated", "double"] as const;
export const COMIC_VIEW_MODE_VALUES = ["single", "double", "strip"] as const;
export const COMIC_DIRECTION_VALUES = ["rtl", "ltr"] as const;
export const COMIC_PAGE_GAP_VALUES = ["zero", "twelve", "twenty_four"] as const;
export const COMIC_PRELOAD_PAGES_VALUES = ["one", "three", "five", "unlimited"] as const;

/** 生成 wire 里的类型名 → 本文件导出的取值表（漂移测试用）。 */
export const PREFERENCE_ENUM_BINDINGS = {
  PreferenceReadingFontFamilyDto: READING_FONT_FAMILY_VALUES,
  PreferenceReadingFontSizeDto: READING_FONT_SIZE_VALUES,
  PreferenceReadingLineHeightDto: READING_LINE_HEIGHT_VALUES,
  PreferenceReadingContentWidthDto: READING_CONTENT_WIDTH_VALUES,
  PreferenceReadingFontWeightDto: READING_FONT_WEIGHT_VALUES,
  PreferenceReadingLetterSpacingDto: READING_LETTER_SPACING_VALUES,
  PreferenceReadingThemeDto: READING_THEME_VALUES,
  PreferenceReadingPaginationDto: READING_PAGINATION_VALUES,
  PreferenceComicViewModeDto: COMIC_VIEW_MODE_VALUES,
  PreferenceComicDirectionDto: COMIC_DIRECTION_VALUES,
  PreferenceComicPageGapDto: COMIC_PAGE_GAP_VALUES,
  PreferenceComicPreloadPagesDto: COMIC_PRELOAD_PAGES_VALUES,
} as const;

/** 自定义文本字段（自定义字体名/背景色/文字色）。长度有界；语义校验由 Haven 提案内核负责。 */
const customText = (label: string) =>
  z
    .string()
    .max(200, `${label} 不得超过 200 字符`)
    .refine((value) => !/[\x00-\x1f\x7f]/.test(value), `${label} 不得包含控制字符`);

export const responseFormatSchema = z
  .enum(RESPONSE_FORMATS)
  .default("json")
  .describe("输出格式：'json' 为与 structuredContent 完全一致的机器可读 JSON；'markdown' 为人类可读摘要。");

export const sectionSchema = z
  .enum(["reading"])
  .default("reading")
  .describe("设置分区。当前闭合集合只有 'reading'（全局阅读排版）。");

export const targetScopeSchema = z
  .enum(["edition", "media_item"])
  .describe("资源偏好作用域：'edition' 作用于整个版本，'media_item' 作用于单个条目。");

export const preferenceScopeSchema = z
  .enum(["edition", "media_item"])
  .default("media_item")
  .describe("查询的资源偏好作用域。");

const listLimitSchema = z
  .number()
  .int()
  .min(1)
  .max(MAX_LIST_ITEMS)
  .default(DEFAULT_LIST_ITEMS)
  .describe(`返回条数上限（1–${MAX_LIST_ITEMS}，默认 ${DEFAULT_LIST_ITEMS}）。`);

// ---- 阅读排版 patch（镜像 PreferenceReadingPatchDto 的闭合枚举）----

export const readingPatchSchema = z
  .strictObject({
    font_family: z
      .enum(READING_FONT_FAMILY_VALUES)
      .nullable()
      .optional()
      .describe("字体族档位。"),
    custom_font_family: customText("custom_font_family").nullable().optional().describe("自定义字体名；仅当 font_family='custom' 时有意义。"),
    font_size: z.enum(READING_FONT_SIZE_VALUES).nullable().optional().describe("字号档位。"),
    line_height: z.enum(READING_LINE_HEIGHT_VALUES).nullable().optional().describe("行高档位。"),
    content_width: z.enum(READING_CONTENT_WIDTH_VALUES).nullable().optional().describe("正文宽度档位。"),
    theme: z
      .enum(READING_THEME_VALUES)
      .nullable()
      .optional()
      .describe("阅读主题；注意 'eyeCare' 是协议里的真实取值（camelCase）。"),
    custom_background: customText("custom_background").nullable().optional().describe("自定义背景色；仅当 theme='custom' 时有意义。"),
    custom_text: customText("custom_text").nullable().optional().describe("自定义文字色；仅当 theme='custom' 时有意义。"),
    font_weight: z.enum(READING_FONT_WEIGHT_VALUES).nullable().optional().describe("字重档位。"),
    letter_spacing: z.enum(READING_LETTER_SPACING_VALUES).nullable().optional().describe("字距档位。"),
    system_auto: z.boolean().nullable().optional().describe("是否跟随系统排版。"),
    pagination: z.enum(READING_PAGINATION_VALUES).nullable().optional().describe("翻页模式。"),
  })
  .refine((patch) => Object.values(patch).some((value) => value !== undefined && value !== null), {
    message: "patch 至少要包含一个非 null 字段；空提案没有任何意义。",
  })
  .refine((patch) => Object.values(patch).filter((value) => value !== undefined).length <= MAX_PATCH_KEYS, {
    message: `一次提案最多修改 ${MAX_PATCH_KEYS} 个键。`,
  });

// ---- 漫画阅读 patch（镜像 PreferenceComicPatchDto）----

export const comicPatchSchema = z
  .strictObject({
    view_mode: z.enum(COMIC_VIEW_MODE_VALUES).nullable().optional().describe("页面布局。"),
    direction: z.enum(COMIC_DIRECTION_VALUES).nullable().optional().describe("翻页方向。"),
    page_gap: z.enum(COMIC_PAGE_GAP_VALUES).nullable().optional().describe("页间距档位。"),
    preload_pages: z.enum(COMIC_PRELOAD_PAGES_VALUES).nullable().optional().describe("预加载页数档位。"),
  })
  .refine((patch) => Object.values(patch).some((value) => value !== undefined && value !== null), {
    message: "patch 至少要包含一个非 null 字段；空提案没有任何意义。",
  });

export const resourcePreferencePatchSchema = z
  .strictObject({
    reading: readingPatchSchema.nullable().optional().describe("文本阅读偏好改动。"),
    comic: comicPatchSchema.nullable().optional().describe("漫画阅读偏好改动。"),
  })
  .refine(
    (patch) =>
      (patch.reading !== undefined && patch.reading !== null) ||
      (patch.comic !== undefined && patch.comic !== null),
    { message: "patch 至少要包含 reading 或 comic 之一。" },
  );

// ---- 提案锚点：创建提案必须回传最近一次读取得到的身份 ----

/**
 * `context_id` / `context_hash` / `base_revision` 来自**最近一次**读取工具。
 *
 * 这三者不是可选装饰：Haven 的提案内核会重新读取 authoritative 状态并逐项比对，
 * 任何一项对不上都 fail-closed（零写入）。因此这里也必须要求它们存在，
 * 不允许"先生成提案再说"。
 */
const proposalAnchorShape = {
  context_id: uuid("context_id").describe("最近一次读取得到的 context id。"),
  context_hash: digest("context_hash").describe("最近一次读取得到的 context hash（canonical 载荷的 SHA-256）。"),
  base_revision: z
    .string()
    .max(128)
    .nullable()
    .describe("最近一次读取得到的 authoritative revision；从未保存过时为 null。"),
};

export const settingsProposalInputSchema = z.strictObject({
  section: sectionSchema,
  ...proposalAnchorShape,
  patch: readingPatchSchema.describe("要提议的全局阅读排版改动；只创建提案，不写设置。"),
  response_format: responseFormatSchema,
});

export const resourcePreferenceProposalInputSchema = z
  .strictObject({
    target_scope: targetScopeSchema,
    edition_id: uuid("edition_id").describe("目标版本 id。"),
    media_item_id: uuid("media_item_id")
      .nullable()
      .describe("target_scope='media_item' 时必填；'edition' 时必须为 null。"),
    ...proposalAnchorShape,
    patch: resourcePreferencePatchSchema.describe("要提议的资源级偏好改动；只创建提案，不写设置。"),
    response_format: responseFormatSchema,
  })
  .refine(
    (value) => (value.target_scope === "media_item" ? value.media_item_id !== null : value.media_item_id === null),
    {
      message:
        "media_item_id 与 target_scope 必须一致：scope='media_item' 时必填，scope='edition' 时必须为 null。",
    },
  );

// ---- 无参 / 简单查询输入 ----

export const emptyInputSchema = z.strictObject({
  response_format: responseFormatSchema,
});

export const sectionInputSchema = z.strictObject({
  section: sectionSchema,
  response_format: responseFormatSchema,
});

export const preferenceSnapshotInputSchema = z.strictObject({
  target_scope: preferenceScopeSchema,
  edition_id: uuid("edition_id").describe("要查询的版本 id。"),
  media_item_id: uuid("media_item_id")
    .nullable()
    .default(null)
    .describe("target_scope='media_item' 时必填；否则为 null。"),
  response_format: responseFormatSchema,
});

export const librarySummaryInputSchema = z.strictObject({
  limit: listLimitSchema,
  response_format: responseFormatSchema,
});

export const mediaCapabilitiesInputSchema = z.strictObject({
  media_item_id: uuid("media_item_id")
    .nullable()
    .default(null)
    .describe("只查询单个条目时填写；null 表示按 limit 返回一批。"),
  limit: listLimitSchema,
  response_format: responseFormatSchema,
});

export type SettingsProposalInput = z.infer<typeof settingsProposalInputSchema>;
export type ResourcePreferenceProposalInput = z.infer<typeof resourcePreferenceProposalInputSchema>;
export type EmptyInput = z.infer<typeof emptyInputSchema>;
export type SectionInput = z.infer<typeof sectionInputSchema>;
export type PreferenceSnapshotInput = z.infer<typeof preferenceSnapshotInputSchema>;
export type LibrarySummaryInput = z.infer<typeof librarySummaryInputSchema>;
export type MediaCapabilitiesInput = z.infer<typeof mediaCapabilitiesInputSchema>;
