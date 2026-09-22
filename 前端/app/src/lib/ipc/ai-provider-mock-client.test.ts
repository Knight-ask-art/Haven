import { describe, expect, it } from "vitest"

import { MockHavenClient } from "./mock-client"
import { HavenError } from "./errors.js"
import type { AiProviderProfileUpsertRequest } from "./generated/wire"
import {
  guardAiProviderModelsCatalog,
  guardAiProviderProfile,
} from "../../features/settings/ipc/ai-provider-gateway"

const PROFILE_ID = "gw-main"
const ENDPOINT = "https://gateway.example.invalid/v1"
const API_KEY = "sk-mock-not-a-real-key"

/** 递归收集投影中的全部字符串，用于断言没有夹带凭据材料。 */
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

function upsertRequest(
  overrides: Partial<AiProviderProfileUpsertRequest> = {},
): AiProviderProfileUpsertRequest {
  return {
    profileId: PROFILE_ID,
    displayName: "自建网关",
    kind: "openai_compatible",
    endpoint: ENDPOINT,
    enabled: true,
    selectedModelId: null,
    expectedRevision: null,
    ...overrides,
  }
}

function client(): MockHavenClient {
  return new MockHavenClient(false, { seedSettings: false })
}

describe("MockHavenClient AI Provider Profile", () => {
  it("starts with no profiles and therefore no available models", async () => {
    const listed = await client().aiProviderProfileList()
    expect(listed.schemaVersion).toBe(1)
    expect(listed.profiles).toEqual([])
  })

  it("never infers models without a credential and reports the honest state", async () => {
    const haven = client()
    await haven.aiProviderProfileUpsert(upsertRequest())

    const noCredential = await haven.aiProviderModelsList({ profileId: PROFILE_ID })
    expect(noCredential.state).toBe("no_credential")
    expect(noCredential.models).toEqual([])
    expect(guardAiProviderModelsCatalog(noCredential)).toBe(true)

    // 写入凭据后才出现来自共享 fixture 的目录；能力值原样来自 fixture 的显式声明。
    await haven.credentialSet({ provider: "ai", profileId: PROFILE_ID, secret: API_KEY })
    const ready = await haven.aiProviderModelsList({ profileId: PROFILE_ID })
    expect(ready.state).toBe("ready")
    expect(ready.models.map((model) => model.modelId)).toEqual([
      "gpt-4o-mini",
      "text-embedding-3-small",
    ])
    expect(ready.models[0].vision).toBe("unknown")
    expect(ready.models[1].embedding).toBe("supported")

    // 停用后回到空目录，且原因是 disabled。
    const current = await haven.aiProviderProfileGet({ profileId: PROFILE_ID })
    await haven.aiProviderProfileUpsert(upsertRequest({
      enabled: false,
      expectedRevision: current.revision,
    }))
    const disabled = await haven.aiProviderModelsList({ profileId: PROFILE_ID })
    expect(disabled.state).toBe("disabled")
    expect(disabled.models).toEqual([])
  })

  it("keeps credential material out of every AI Provider response", async () => {
    const haven = client()
    const created = await haven.aiProviderProfileUpsert(upsertRequest())
    await haven.credentialSet({ provider: "ai", profileId: PROFILE_ID, secret: API_KEY })

    const listed = await haven.aiProviderProfileList()
    const catalog = await haven.aiProviderModelsList({ profileId: PROFILE_ID })
    const status = await haven.credentialStatus({ provider: "ai", profileId: PROFILE_ID })

    expect(created.credentialConfigured).toBe(false)
    expect(listed.profiles[0].credentialConfigured).toBe(true)
    expect(status).toEqual({ configured: true, updatedAt: null })

    const strings = [
      ...collectStrings(listed),
      ...collectStrings(catalog),
      ...collectStrings(status),
    ]
    for (const text of strings) {
      for (const forbidden of [API_KEY, "haven:ai:", "secret", "apiKey", "Bearer"]) {
        expect(text).not.toContain(forbidden)
      }
    }
  })

  it("enforces CAS on upsert and preserves createdAt", async () => {
    const haven = client()
    const created = await haven.aiProviderProfileUpsert(upsertRequest())

    // expectedRevision=null 且行已存在 → 冲突。
    await expect(haven.aiProviderProfileUpsert(upsertRequest()))
      .rejects.toMatchObject({ code: "AI_PROVIDER_PROFILE_REVISION_CONFLICT" })

    // 过期版本 → 冲突，且内容不变。
    await expect(haven.aiProviderProfileUpsert(upsertRequest({
      displayName: "被覆盖的名字",
      expectedRevision: "stale",
    }))).rejects.toMatchObject({ code: "AI_PROVIDER_PROFILE_REVISION_CONFLICT" })
    await expect(haven.aiProviderProfileGet({ profileId: PROFILE_ID })).resolves.toEqual(created)

    // 正确版本 → 成功，revision 变化，createdAt 保留。
    const updated = await haven.aiProviderProfileUpsert(upsertRequest({
      displayName: "改名后的网关",
      expectedRevision: created.revision,
    }))
    expect(updated.revision).not.toBe(created.revision)
    expect(updated.createdAt).toBe(created.createdAt)
    expect(guardAiProviderProfile(updated)).toBe(true)
  })

  it("enforces CAS on delete and only then clears the API key", async () => {
    const haven = client()
    const created = await haven.aiProviderProfileUpsert(upsertRequest())
    await haven.credentialSet({ provider: "ai", profileId: PROFILE_ID, secret: API_KEY })

    // 过期版本：冲突，并且**不得**销毁凭据 —— 与后端同一取舍。
    await expect(haven.aiProviderProfileDelete({
      profileId: PROFILE_ID,
      expectedRevision: "stale",
    })).rejects.toMatchObject({ code: "AI_PROVIDER_PROFILE_REVISION_CONFLICT" })
    await expect(haven.credentialStatus({ provider: "ai", profileId: PROFILE_ID }))
      .resolves.toEqual({ configured: true, updatedAt: null })
    // 行本身不变；`credentialConfigured` 是凭据的实时事实（此时仍为 true）。
    await expect(haven.aiProviderProfileGet({ profileId: PROFILE_ID }))
      .resolves.toEqual({ ...created, credentialConfigured: true })

    // 正确版本：凭据与行一起清理。
    const deleted = await haven.aiProviderProfileDelete({
      profileId: PROFILE_ID,
      expectedRevision: created.revision,
    })
    expect(deleted).toEqual({ profileId: PROFILE_ID, credentialDeleted: true })
    await expect(haven.credentialStatus({ provider: "ai", profileId: PROFILE_ID }))
      .resolves.toEqual({ configured: false, updatedAt: null })
    await expect(haven.aiProviderProfileGet({ profileId: PROFILE_ID }))
      .rejects.toMatchObject({ code: "AI_PROVIDER_PROFILE_NOT_FOUND" })
  })

  it("rejects invalid profile input with stable error codes and zero writes", async () => {
    const haven = client()
    await expect(haven.aiProviderProfileUpsert(upsertRequest({ profileId: "bad/id" })))
      .rejects.toBeInstanceOf(HavenError)
    await expect(haven.aiProviderProfileUpsert(upsertRequest({ profileId: "bad/id" })))
      .rejects.toMatchObject({ code: "AI_PROVIDER_PROFILE_ID_INVALID" })
    await expect(haven.aiProviderProfileUpsert(upsertRequest({ endpoint: "C:\\gateway" })))
      .rejects.toMatchObject({ code: "AI_PROVIDER_ENDPOINT_INVALID" })
    await expect(haven.aiProviderProfileUpsert(upsertRequest({
      endpoint: "https://gateway.example.invalid/v1?key=secret",
    }))).rejects.toMatchObject({ code: "AI_PROVIDER_ENDPOINT_INVALID" })

    await expect(haven.aiProviderProfileList()).resolves.toEqual({
      schemaVersion: 1,
      profiles: [],
    })
  })
})
