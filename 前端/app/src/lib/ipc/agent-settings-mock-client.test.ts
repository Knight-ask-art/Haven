import { afterEach, describe, expect, it, vi } from "vitest"

import { MockHavenClient } from "./mock-client"

const DIGEST_PATTERN = /^[0-9a-f]{64}$/
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/

afterEach(() => {
  vi.useRealTimers()
})

/** 递归收集投影中的全部字符串，用于断言没有夹带凭据/路径。 */
function collectStrings(value: unknown, collected: string[] = []): string[] {
  if (typeof value === "string") {
    collected.push(value)
  } else if (Array.isArray(value)) {
    for (const item of value) collectStrings(item, collected)
  } else if (typeof value === "object" && value !== null) {
    for (const item of Object.values(value)) collectStrings(item, collected)
  }
  return collected
}

/** Mock 的上下文/提案行：先读上下文，再按它建提案。 */
async function createProposal(client: MockHavenClient) {
  const context = await client.agentSettingsContextGet()
  const proposal = await client.agentSettingsProposalCreate({
    sessionId: "0196f0d2-0000-7000-8000-00000000e701",
    requestId: "0196f0d2-0000-7000-8000-00000000e702",
    contextId: context.contextId,
    contextHash: context.contextHash,
    baseRevision: context.revision,
    patch: { fontSize: "large", lineHeight: "airy", contentWidth: "narrow" },
  })
  return { context, proposal }
}

describe("MockHavenClient Agent 全局设置 Typed 契约", () => {
  it("上下文只暴露脱敏阅读快照 + revision + 派生身份 + 固定能力清单", async () => {
    const client = new MockHavenClient()
    const context = await client.agentSettingsContextGet()

    expect(context.schemaVersion).toBe(1)
    expect(context.subject).toEqual({ section: "reading" })
    expect(context.contextId).toMatch(UUID_PATTERN)
    expect(context.contextHash).toMatch(DIGEST_PATTERN)
    expect(context.reading.section).toBe("reading")
    expect(Array.isArray(context.reading.redactedFields)).toBe(true)
    // 服务端固定清单：只声明已实现能力，未实现能力一律 false。
    expect(context.capabilities.capabilities).toMatchObject({
      settingsRead: true,
      settingsProposal: true,
      librarySummaryRead: false,
      metadataProposal: false,
      renameProposal: false,
      secretRead: false,
      filesystemWrite: false,
    })
    // 没有 secret、绝对路径或任意 JSON 通道。
    for (const text of collectStrings(context)) {
      expect(text).not.toMatch(/[A-Za-z]:\\/)
      expect(text).not.toMatch(/api[_-]?key/i)
    }
  })

  it("同一设置版本得到同一个 context id/hash；设置变化后身份随之改变", async () => {
    const client = new MockHavenClient()
    const first = await client.agentSettingsContextGet()
    const second = await client.agentSettingsContextGet()
    expect(second.contextId).toBe(first.contextId)
    expect(second.contextHash).toBe(first.contextHash)

    // 应用一次提案后，阅读设置被写回 Mock 状态 → 上下文身份必须变化。
    const { proposal } = await createProposal(client)
    await client.agentSettingsProposalApprove({
      proposalId: proposal.proposalId,
      expectedDigest: proposal.digest,
    })
    const after = await client.agentSettingsContextGet()
    expect(after.contextHash).not.toBe(first.contextHash)
    expect(after.contextId).not.toBe(first.contextId)
  })

  it("提案的 digest 由 Mock 生成（64 位小写十六进制），改动列表来自真实差异", async () => {
    const client = new MockHavenClient()
    const { proposal } = await createProposal(client)

    expect(proposal.proposalId).toBeTruthy()
    expect(proposal.status).toBe("pending")
    expect(proposal.digest).toMatch(DIGEST_PATTERN)
    expect(proposal.targetLabel).toContain("阅读")
    expect(proposal.changes.map((change) => change.key)).toEqual([
      "reading.fontSize",
      "reading.lineHeight",
      "reading.contentWidth",
    ])
    expect(proposal.changes.every((change) => change.before !== change.after)).toBe(true)
  })

  it("stale 上下文 / 伪造 hash / 伪造 id / 无改动 patch 全部 fail-closed", async () => {
    const client = new MockHavenClient()
    const context = await client.agentSettingsContextGet()
    const base = {
      sessionId: "0196f0d2-0000-7000-8000-00000000e701",
      requestId: "0196f0d2-0000-7000-8000-00000000e702",
      contextId: context.contextId,
      contextHash: context.contextHash,
      baseRevision: context.revision,
      patch: { fontSize: "large" as const },
    }

    await expect(
      client.agentSettingsProposalCreate({ ...base, baseRevision: "rev-does-not-exist" }),
    ).rejects.toMatchObject({ code: "AGENT_SETTINGS_BASE_REVISION_MISMATCH" })
    await expect(
      client.agentSettingsProposalCreate({ ...base, contextHash: "b".repeat(64) }),
    ).rejects.toMatchObject({ code: "AGENT_SETTINGS_CONTEXT_STALE" })
    await expect(
      client.agentSettingsProposalCreate({
        ...base,
        contextId: "0196f0d2-0000-7000-8000-00000000e999",
      }),
    ).rejects.toMatchObject({ code: "AGENT_SETTINGS_CONTEXT_ID_MISMATCH" })
    // 与当前值相同的 patch 不产生提案。
    await expect(
      client.agentSettingsProposalCreate({
        ...base,
        patch: { fontSize: context.reading.fontSize },
      }),
    ).rejects.toMatchObject({ code: "AGENT_SETTINGS_PATCH_CONFLICT" })
  })

  it("批准只接受 UI 显示的那份 digest，并用它写出设置与回执", async () => {
    const client = new MockHavenClient()
    const { proposal } = await createProposal(client)

    await expect(
      client.agentSettingsProposalApprove({
        proposalId: proposal.proposalId,
        expectedDigest: "c".repeat(64),
      }),
    ).rejects.toMatchObject({ code: "SETTING_PROPOSAL_DIGEST_MISMATCH" })

    const result = await client.agentSettingsProposalApprove({
      proposalId: proposal.proposalId,
      expectedDigest: proposal.digest,
    })
    expect(result.proposal.status).toBe("applied")
    expect(result.receipt.status).toBe("applied")
    expect(result.receipt.proposalDigest).toBe(proposal.digest)
    expect(result.receipt.changed).toBe(true)
    expect(result.receipt.appliedRevision).toBeTruthy()
    expect(result.receipt.changes).toEqual(proposal.changes)

    // 回执读取与提案回读同源。
    const stored = await client.agentSettingChangeReceiptGet({ proposalId: proposal.proposalId })
    expect(stored?.receiptId).toBe(result.receipt.receiptId)

    const reread = await client.agentSettingsProposalGet({ proposalId: proposal.proposalId })
    expect(reread.proposal.status).toBe("applied")
    expect(reread.receipt?.appliedRevision).toBe(result.receipt.appliedRevision)
  })

  it("拒绝零写入，且已拒绝的提案不能再批准", async () => {
    const client = new MockHavenClient()
    const { proposal } = await createProposal(client)
    const before = await client.agentSettingsContextGet()

    const rejected = await client.agentSettingsProposalReject({
      proposalId: proposal.proposalId,
      expectedDigest: proposal.digest,
    })
    expect(rejected.proposal.status).toBe("rejected")
    // 零写入：上下文身份不变。
    const after = await client.agentSettingsContextGet()
    expect(after.contextHash).toBe(before.contextHash)

    await expect(
      client.agentSettingsProposalApprove({
        proposalId: proposal.proposalId,
        expectedDigest: proposal.digest,
      }),
    ).rejects.toMatchObject({ code: "SETTING_PROPOSAL_NOT_PENDING" })
    expect(
      await client.agentSettingChangeReceiptGet({ proposalId: proposal.proposalId }),
    ).toBeNull()
  })

  it("提案生成后设置被外部改写：批准冲突且零写入", async () => {
    const client = new MockHavenClient()
    const { proposal } = await createProposal(client)

    // 模拟别处（例如设置页表单）改动阅读分区，推进 revision。
    await client.settingsUpdate({
      section: "reading",
      expectedRevision: (await client.settingsGet("reading")).revision,
      patch: { section: "reading", theme: "slate" },
    })

    await expect(
      client.agentSettingsProposalApprove({
        proposalId: proposal.proposalId,
        expectedDigest: proposal.digest,
      }),
    ).rejects.toMatchObject({ code: "REVISION_CONFLICT" })
    // 冲突后提案仍未被应用，也没有回执。
    expect(
      await client.agentSettingChangeReceiptGet({ proposalId: proposal.proposalId }),
    ).toBeNull()
  })

  it("批准过期提案时先收敛为 expired，且零设置写入、零回执", async () => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date("2026-09-16T10:00:00.000Z"))
    const client = new MockHavenClient()
    const { proposal } = await createProposal(client)

    vi.setSystemTime(new Date(proposal.expiresAt).getTime() + 1)
    await expect(
      client.agentSettingsProposalApprove({
        proposalId: proposal.proposalId,
        expectedDigest: proposal.digest,
      }),
    ).rejects.toMatchObject({ code: "SETTING_PROPOSAL_EXPIRED" })

    const reread = await client.agentSettingsProposalGet({ proposalId: proposal.proposalId })
    expect(reread.proposal.status).toBe("expired")
    expect(reread.receipt).toBeNull()
    await expect(
      client.agentSettingChangeReceiptGet({ proposalId: proposal.proposalId }),
    ).resolves.toBeNull()
  })

  it("已批准的提案不能再次拒绝，未知 proposal 在回读和回执读取时都 fail-closed", async () => {
    const client = new MockHavenClient()
    const { proposal } = await createProposal(client)
    await client.agentSettingsProposalApprove({
      proposalId: proposal.proposalId,
      expectedDigest: proposal.digest,
    })

    await expect(
      client.agentSettingsProposalReject({
        proposalId: proposal.proposalId,
        expectedDigest: proposal.digest,
      }),
    ).rejects.toMatchObject({ code: "SETTING_PROPOSAL_NOT_PENDING" })

    const unknown = "mock-proposal-does-not-exist"
    await expect(client.agentSettingsProposalGet({ proposalId: unknown })).rejects.toMatchObject({
      code: "SETTING_PROPOSAL_NOT_FOUND",
    })
    await expect(client.agentSettingChangeReceiptGet({ proposalId: unknown })).rejects.toMatchObject({
      code: "SETTING_PROPOSAL_NOT_FOUND",
    })
  })
})
