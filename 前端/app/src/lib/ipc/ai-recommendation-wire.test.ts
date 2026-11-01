import { describe, expect, it, vi } from "vitest"

import { guardAiSettingsRecommendation } from "./ai-recommendation-wire"
import type { AiSettingsRecommendationDto } from "./generated/wire"
import { listAgentProposals } from "@/features/settings/ipc/agent-proposal-gateway"

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
  it.each(["", "  ", "#f5efe3", " #f5efe3 "])("accepts the domain's optional color semantics: %j", (customBackground) => {
    const value = recommendation()
    value.recommendedPatch = { customBackground }
    value.proposal.changes = [{ key: "reading.customBackground", before: "", after: customBackground.trim() }]
    expect(guardAiSettingsRecommendation(value, PROFILE_ID)).toBe(true)
    expect(guardAiSettingsRecommendation({ ...value, recommendedPatch: { customBackground: "red" } }, PROFILE_ID)).toBe(false)
  })

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

describe("restored proposal history guard", () => {
  it("restores empty optional values in proposal and approval receipt changes", async () => {
    const proposal = { ...recommendation().proposal, status: "applied" as const, changes: [{ key: "reading.customFontFamily", before: "", after: "Test Font" }] }
    const receipt = { schemaVersion: 1, receiptId: "receipt-1", proposalId: proposal.proposalId, proposalDigest: proposal.digest, status: "applied", appliedRevision: "rev-2", changed: true, changes: proposal.changes, appliedAt: proposal.createdAt }
    const client = { agentSettingsProposalList: vi.fn().mockResolvedValue({ schemaVersion: 1, items: [{ proposal, receipt }], nextCursor: null }) }
    expect((await listAgentProposals(client, null)).items[0].receipt?.changes[0].before).toBe("")
    client.agentSettingsProposalList.mockResolvedValue({ schemaVersion: 1, items: [{ proposal, receipt: { ...receipt, changes: [{ key: "reading.customFontFamily", before: null, after: "Test Font" }] } }], nextCursor: null })
    await expect(listAgentProposals(client, null)).rejects.toMatchObject({ code: "AGENT_PROPOSAL_HISTORY_INVALID" })
  })

  it("accepts stored pending proposals but rejects an applied row without its receipt", async () => {
    const proposal = recommendation().proposal
    const client = { agentSettingsProposalList: vi.fn().mockResolvedValue({ schemaVersion: 1, items: [{ proposal, receipt: null }], nextCursor: null }) }
    expect((await listAgentProposals(client, null)).items[0].proposal.proposalId).toBe(proposal.proposalId)
    client.agentSettingsProposalList.mockResolvedValue({ schemaVersion: 1, items: [{ proposal: { ...proposal, status: "applied" }, receipt: null }], nextCursor: null })
    await expect(listAgentProposals(client, null)).rejects.toMatchObject({ code: "AGENT_PROPOSAL_HISTORY_INVALID" })
  })

  it("rejects mismatched approval receipts and a looping pagination cursor", async () => {
    const proposal = { ...recommendation().proposal, status: "applied" as const }
    const receipt = { schemaVersion: 1, receiptId: "receipt-1", proposalId: "another-proposal", proposalDigest: proposal.digest, status: "applied", appliedRevision: "rev-2", changed: true, changes: proposal.changes, appliedAt: proposal.createdAt }
    const client = { agentSettingsProposalList: vi.fn().mockResolvedValue({ schemaVersion: 1, items: [{ proposal, receipt }], nextCursor: null }) }
    await expect(listAgentProposals(client, null)).rejects.toMatchObject({ code: "AGENT_PROPOSAL_HISTORY_INVALID" })
    client.agentSettingsProposalList.mockResolvedValue({ schemaVersion: 1, items: [{ proposal: recommendation().proposal, receipt: null }], nextCursor: proposal.proposalId })
    await expect(listAgentProposals(client, proposal.proposalId)).rejects.toMatchObject({ code: "AGENT_PROPOSAL_HISTORY_INVALID" })
  })
})
