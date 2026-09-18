import { beforeEach, describe, expect, it, vi } from "vitest"

const { invoke, check } = vi.hoisted(() => ({ invoke: vi.fn(), check: vi.fn() }))
vi.mock("@tauri-apps/api/core", () => ({
  Channel: class Channel<T> { onmessage?: (event: T) => void },
  invoke,
}))
vi.mock("@tauri-apps/plugin-updater", () => ({ check }))

import { TauriHavenClient } from "./tauri-client"

beforeEach(() => {
  invoke.mockReset()
})

const PROPOSAL_ID = "0196f0d2-0000-7000-8000-00000000e501"
const DIGEST = "a".repeat(64)

describe("TauriHavenClient Agent 全局设置 Typed IPC", () => {
  it("agent_settings_context_get 不接受入参，直接映射出脱敏上下文 DTO", async () => {
    const payload = { schemaVersion: 1, contextId: "ctx", contextHash: DIGEST }
    invoke.mockResolvedValueOnce(payload)

    await expect(new TauriHavenClient().agentSettingsContextGet()).resolves.toBe(payload)
    expect(invoke).toHaveBeenCalledWith("agent_settings_context_get")
  })

  it("能力清单命令无参数且不做业务兜底", async () => {
    const payload = {
      agentApiVersion: 1,
      capabilities: {
        settingsRead: true,
        settingsProposal: true,
        librarySummaryRead: false,
        metadataProposal: false,
        renameProposal: false,
        secretRead: false,
        filesystemWrite: false,
      },
    }
    invoke.mockResolvedValueOnce(payload)

    await expect(new TauriHavenClient().agentCapabilityManifestGet()).resolves.toBe(payload)
    expect(invoke).toHaveBeenCalledWith("agent_capability_manifest_get")
  })

  it("创建提案只传 typed request 对象（context id/hash + base revision + patch）", async () => {
    const request = {
      sessionId: "0196f0d2-0000-7000-8000-00000000e701",
      requestId: "0196f0d2-0000-7000-8000-00000000e702",
      contextId: "0196f0d2-0000-7000-8000-00000000e401",
      contextHash: DIGEST,
      baseRevision: "rev-1",
      patch: { fontSize: "large" as const },
    }
    invoke.mockResolvedValueOnce({ schemaVersion: 1, proposalId: PROPOSAL_ID })

    await new TauriHavenClient().agentSettingsProposalCreate(request)
    expect(invoke).toHaveBeenCalledWith("agent_settings_proposal_create", { request })
  })

  it("批准只提交提案 ID 与 UI 显示的那份 digest；请求里没有 token 字段", async () => {
    const request = { proposalId: PROPOSAL_ID, expectedDigest: DIGEST }
    invoke.mockResolvedValueOnce({ proposal: { proposalId: PROPOSAL_ID }, receipt: {} })

    await new TauriHavenClient().agentSettingsProposalApprove(request)
    expect(invoke).toHaveBeenCalledWith("agent_settings_proposal_approve", { request })
    // 传输层不得自行扩字段：token 由 Rust 内部生成并消费。
    const [, args] = invoke.mock.calls[0] as [string, { request: Record<string, unknown> }]
    expect(Object.keys(args.request).sort()).toEqual(["expectedDigest", "proposalId"])
  })

  it("拒绝与回执读取都走各自命令", async () => {
    invoke.mockResolvedValueOnce({ proposal: { proposalId: PROPOSAL_ID } })
    await new TauriHavenClient().agentSettingsProposalReject({
      proposalId: PROPOSAL_ID,
      expectedDigest: DIGEST,
    })
    expect(invoke).toHaveBeenCalledWith("agent_settings_proposal_reject", {
      request: { proposalId: PROPOSAL_ID, expectedDigest: DIGEST },
    })

    invoke.mockReset()
    invoke.mockResolvedValueOnce(null)
    await expect(
      new TauriHavenClient().agentSettingChangeReceiptGet({ proposalId: PROPOSAL_ID }),
    ).resolves.toBeNull()
    expect(invoke).toHaveBeenCalledWith("agent_setting_change_receipt_get", {
      request: { proposalId: PROPOSAL_ID },
    })
  })

  it("把契约 ErrorDto 归一成 HavenError（稳定 code 与可重试语义）", async () => {
    invoke.mockRejectedValueOnce({
      code: "SETTING_PROPOSAL_DIGEST_MISMATCH",
      userMessage: "确认的摘要与提案不一致",
      retryable: false,
    })

    await expect(
      new TauriHavenClient().agentSettingsProposalApprove({
        proposalId: PROPOSAL_ID,
        expectedDigest: DIGEST,
      }),
    ).rejects.toMatchObject({
      code: "SETTING_PROPOSAL_DIGEST_MISMATCH",
      message: "确认的摘要与提案不一致",
    })
  })
})
