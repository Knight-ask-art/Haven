import type { HavenClient } from "@/lib/ipc/client"
import { HavenError } from "@/lib/ipc/errors"
import { guardAgentSettingsProposal } from "@/lib/ipc/ai-recommendation-wire"
import type { AgentSettingsProposalGetResultDto, AgentSettingsProposalListResultDto } from "@/lib/ipc/generated/wire"

export type AgentProposalHistoryClient = Partial<Pick<HavenClient, "agentSettingsProposalList">>

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function fields(value: Record<string, unknown>, names: string[]): boolean {
  return Reflect.ownKeys(value).length === names.length
    && names.every((name) => Object.prototype.hasOwnProperty.call(value, name))
}

function text(value: unknown): value is string {
  return typeof value === "string" && value.length > 0
}

function guardItem(value: unknown): value is AgentSettingsProposalGetResultDto {
  if (!record(value) || !fields(value, ["proposal", "receipt"]) || !guardAgentSettingsProposal(value.proposal)) return false
  const proposal = value.proposal
  const receipt = value.receipt
  if (receipt === null) return proposal.status !== "applied"
  return record(receipt)
    && fields(receipt, ["schemaVersion", "receiptId", "proposalId", "proposalDigest", "status", "appliedRevision", "changed", "changes", "appliedAt"])
    && proposal.status === "applied" && receipt.status === "applied" && receipt.schemaVersion === 1
    && text(receipt.receiptId) && receipt.proposalId === proposal.proposalId && receipt.proposalDigest === proposal.digest
    && typeof receipt.changed === "boolean"
    && (receipt.changed ? text(receipt.appliedRevision) : receipt.appliedRevision === null)
    && text(receipt.appliedAt) && Array.isArray(receipt.changes)
    && receipt.changes.every((change: unknown) => record(change) && fields(change, ["key", "before", "after"])
      && text(change.key) && typeof change.before === "string" && typeof change.after === "string")
}

export async function listAgentProposals(client: AgentProposalHistoryClient, before: string | null): Promise<AgentSettingsProposalListResultDto> {
  const value: unknown = await client.agentSettingsProposalList?.({ before, limit: 30 })
  if (!record(value) || !fields(value, ["schemaVersion", "items", "nextCursor"]) || value.schemaVersion !== 1
    || !Array.isArray(value.items) || value.items.length > 30 || !value.items.every(guardItem)
    || !(value.nextCursor === null || text(value.nextCursor))
    || (value.nextCursor !== null && (value.items.at(-1)?.proposal.proposalId !== value.nextCursor || value.nextCursor === before))
    || new Set(value.items.map((item) => item.proposal.proposalId)).size !== value.items.length) {
    throw new HavenError({ code: "AGENT_PROPOSAL_HISTORY_INVALID", userMessage: "提案历史读取失败，返回内容不完整。请重新读取。", retryable: true })
  }
  return { schemaVersion: 1, items: value.items, nextCursor: value.nextCursor }
}
