import { describe, expect, it } from "vitest"

import { guardAiSettingsRecommendation } from "./ai-recommendation-wire"
import type { AiSettingsRecommendationDto } from "./generated/wire"

const PROFILE_ID = "provider-main"

function recommendation(): AiSettingsRecommendationDto {
  return {
    schemaVersion: 1,
    profileId: PROFILE_ID,
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

describe("AI recommendation response guard", () => {
  it("accepts the exact pending typed response for the requested profile", () => {
    expect(guardAiSettingsRecommendation(recommendation(), PROFILE_ID)).toBe(true)
  })

  it("rejects missing model provenance and a profile mismatch", () => {
    const missingModel = { ...recommendation(), modelId: undefined }
    expect(guardAiSettingsRecommendation(missingModel, PROFILE_ID)).toBe(false)
    expect(guardAiSettingsRecommendation(recommendation(), "other-profile")).toBe(false)
  })

  it("rejects a non-pending proposal, invalid patch enum, and unrecognized fields", () => {
    const base = recommendation()
    expect(guardAiSettingsRecommendation({
      ...base,
      proposal: { ...base.proposal, status: "applied" },
    }, PROFILE_ID)).toBe(false)
    expect(guardAiSettingsRecommendation({
      ...base,
      recommendedPatch: { fontSize: "gigantic" },
    }, PROFILE_ID)).toBe(false)
    expect(guardAiSettingsRecommendation({ ...base, endpoint: "https://unexpected.invalid" }, PROFILE_ID)).toBe(false)
    expect(guardAiSettingsRecommendation({
      ...base,
      proposal: { ...base.proposal, changes: [{ ...base.proposal.changes[0], raw: "unexpected" }] },
    }, PROFILE_ID)).toBe(false)
  })

  it("rejects symbol and non-enumerable extension fields", () => {
    const symbolKey = Symbol("extension")
    const withSymbol = recommendation() as unknown as Record<PropertyKey, unknown>
    withSymbol[symbolKey] = "extra"
    expect(guardAiSettingsRecommendation(withSymbol, PROFILE_ID)).toBe(false)

    const withHidden = recommendation() as unknown as Record<string, unknown>
    Object.defineProperty(withHidden, "hidden", { value: true, enumerable: false })
    expect(guardAiSettingsRecommendation(withHidden, PROFILE_ID)).toBe(false)
  })
})
