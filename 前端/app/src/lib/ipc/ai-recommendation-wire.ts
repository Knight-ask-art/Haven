import type {
  AiSettingsRecommendationDto,
  AgentSettingChangeDto,
  AgentSettingsProposalDto,
  PreferenceReadingPatchDto,
} from "./generated/wire"

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function hasExactFields(value: Record<string, unknown>, fields: readonly string[]): boolean {
  const allowed = new Set(fields)
  for (const field of Reflect.ownKeys(value)) {
    if (typeof field !== "string" || !allowed.has(field)) return false
  }
  return fields.every((field) => Object.prototype.hasOwnProperty.call(value, field))
}

function isText(value: unknown): value is string {
  return typeof value === "string" && value.length > 0
}

function isNullableText(value: unknown): value is string | null {
  return value === null || isText(value)
}

const PATCH_FIELDS = [
  "fontFamily",
  "customFontFamily",
  "fontSize",
  "lineHeight",
  "contentWidth",
  "theme",
  "customBackground",
  "customText",
  "fontWeight",
  "letterSpacing",
  "systemAuto",
  "pagination",
] as const

const PATCH_ENUMS = {
  fontFamily: new Set(["sans", "serif", "kai", "heiti", "fangsong", "mianfei", "custom"]),
  fontSize: new Set(["small", "medium", "large"]),
  lineHeight: new Set(["compact", "comfortable", "airy"]),
  contentWidth: new Set(["narrow", "medium", "wide"]),
  theme: new Set(["system", "paper", "warm", "slate", "dark", "sepia", "eyeCare", "custom"]),
  fontWeight: new Set(["light", "regular", "medium", "semibold", "bold"]),
  letterSpacing: new Set(["tight", "normal", "relaxed", "loose"]),
  pagination: new Set(["scroll", "paginated", "double"]),
} satisfies Record<string, ReadonlySet<string>>

function guardReadingPatch(value: unknown): value is PreferenceReadingPatchDto {
  if (!isRecord(value)) return false
  for (const field of Reflect.ownKeys(value)) {
    if (typeof field !== "string" || !PATCH_FIELDS.includes(field as (typeof PATCH_FIELDS)[number])) {
      return false
    }
  }

  for (const field of PATCH_FIELDS) {
    if (!Object.prototype.hasOwnProperty.call(value, field)) continue
    const entry = value[field]
    if (entry === null) continue
    if (field in PATCH_ENUMS) {
      if (typeof entry !== "string" || !PATCH_ENUMS[field as keyof typeof PATCH_ENUMS].has(entry)) {
        return false
      }
      continue
    }
    if (field === "customFontFamily") {
      if (typeof entry !== "string") return false
      continue
    }
    if (field === "customBackground" || field === "customText") {
      if (typeof entry !== "string" || !/^#[0-9a-fA-F]{6}$/.test(entry)) return false
      continue
    }
    if (field === "systemAuto" && typeof entry !== "boolean") return false
  }
  return true
}

const CHANGE_FIELDS = ["key", "before", "after"] as const

function guardSettingChange(value: unknown): value is AgentSettingChangeDto {
  return isRecord(value)
    && hasExactFields(value, CHANGE_FIELDS)
    && isText(value.key)
    && isText(value.before)
    && isText(value.after)
}

const PROPOSAL_FIELDS = [
  "schemaVersion",
  "proposalId",
  "status",
  "subject",
  "targetLabel",
  "baseRevision",
  "digest",
  "createdAt",
  "expiresAt",
  "changes",
] as const

function guardProposal(value: unknown): value is AgentSettingsProposalDto {
  if (!isRecord(value) || !hasExactFields(value, PROPOSAL_FIELDS)) return false
  const subject = value.subject
  return value.schemaVersion === 1
    && isText(value.proposalId)
    && value.status === "pending"
    && isRecord(subject)
    && hasExactFields(subject, ["section"])
    && subject.section === "reading"
    && isText(value.targetLabel)
    && isNullableText(value.baseRevision)
    && typeof value.digest === "string"
    && /^[0-9a-f]{64}$/.test(value.digest)
    && isText(value.createdAt)
    && isText(value.expiresAt)
    && Array.isArray(value.changes)
    && value.changes.length > 0
    && value.changes.every(guardSettingChange)
}

const RECOMMENDATION_FIELDS = [
  "schemaVersion",
  "profileId",
  "modelId",
  "explanation",
  "recommendedPatch",
  "proposal",
] as const

/**
 * AI 建议响应跨越模型 Provider 边界，不能只靠 TypeScript 断言。
 * 仅接受与当前 profile 绑定、确由模型返回且仍待用户批准的完整 typed DTO。
 */
export function guardAiSettingsRecommendation(
  value: unknown,
  expectedProfileId: string,
): value is AiSettingsRecommendationDto {
  if (!isRecord(value) || !hasExactFields(value, RECOMMENDATION_FIELDS)) return false
  return value.schemaVersion === 1
    && value.profileId === expectedProfileId
    && isText(value.modelId)
    && (value.explanation === null || typeof value.explanation === "string")
    && guardReadingPatch(value.recommendedPatch)
    && guardProposal(value.proposal)
}
