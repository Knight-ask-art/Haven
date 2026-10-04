import { beforeEach, describe, expect, it, vi } from "vitest"

import profileListNormal from "../../../../../../contracts/ipc/v1/fixtures/ai-provider/profile.list.normal.json"
import profileListEmpty from "../../../../../../contracts/ipc/v1/fixtures/ai-provider/profile.list.empty.json"
import modelsReady from "../../../../../../contracts/ipc/v1/fixtures/ai-provider/models.catalog.normal.json"
import modelsNoCredential from "../../../../../../contracts/ipc/v1/fixtures/ai-provider/models.catalog.no-credential.json"
import modelsDisabled from "../../../../../../contracts/ipc/v1/fixtures/ai-provider/models.catalog.disabled.json"
import modelsEmpty from "../../../../../../contracts/ipc/v1/fixtures/ai-provider/models.catalog.empty.json"

// 身份边界用例需要替换 IPC 客户端；形状守卫用例本身不碰它。
const mocks = vi.hoisted(() => ({ getHavenClient: vi.fn() }))

vi.mock("@/lib/ipc/runtime", () => ({ getHavenClient: mocks.getHavenClient }))

import {
  NO_AVAILABLE_MODEL,
  NO_MODEL_SELECTED,
  catalogHasModels,
  catalogStateHint,
  capabilityLabel,
  deleteAiProviderProfile,
  getAiProviderProfile,
  guardAiProviderModel,
  guardAiProviderModelsCatalog,
  guardAiProviderProfile,
  guardAiProviderProfileList,
  guardCredentialStatus,
  toModelOptions,
  upsertAiProviderProfile,
} from "./ai-provider-gateway"

describe("AI Provider wire runtime guards", () => {
  // 目录守卫要求响应与**请求的那个 profile** 绑定；夹具里的 id 就是请求时用的那个。
  const PROFILE_ID = modelsReady.profileId
  const OTHER_PROFILE_ID = "another-provider"

  it("accepts the shared fixtures", () => {
    expect(guardAiProviderProfileList(profileListNormal)).toBe(true)
    expect(guardAiProviderProfileList(profileListEmpty)).toBe(true)
    expect(guardAiProviderModelsCatalog(modelsReady, PROFILE_ID)).toBe(true)
    expect(guardAiProviderModelsCatalog(modelsNoCredential, modelsNoCredential.profileId)).toBe(true)
    expect(guardAiProviderModelsCatalog(modelsDisabled, modelsDisabled.profileId)).toBe(true)
    expect(guardAiProviderModelsCatalog(modelsEmpty, modelsEmpty.profileId)).toBe(true)
  })

  it("rejects a catalog that belongs to a different profile", () => {
    // 身份边界：切换 profile 后的迟到响应会把另一个配置的模型目录当成本次请求的结果。
    expect(guardAiProviderModelsCatalog(modelsReady, OTHER_PROFILE_ID)).toBe(false)
    // 反向证据：同一份载荷在匹配的 id 下必须通过（否则上面的拒绝可以靠"一律拒绝"通过）。
    expect(guardAiProviderModelsCatalog(modelsReady, PROFILE_ID)).toBe(true)
  })

  it("rejects credential material and unknown keys anywhere in a profile", () => {
    const profile = profileListNormal.profiles[0]
    for (const forbidden of ["secret", "apiKey", "api_key", "credentialRef", "target", "token"]) {
      expect(guardAiProviderProfile({ ...profile, [forbidden]: "sk-not-a-real-key" })).toBe(false)
    }
    // 未知键一律拒绝，而不是"忽略"
    expect(guardAiProviderProfile({ ...profile, rootPath: "D:\\Private" })).toBe(false)
    expect(guardAiProviderProfile({ ...profile, kind: "anthropic" })).toBe(false)
    expect(
      guardAiProviderProfile({ ...profile, endpoint: "file:///etc/passwd" }),
      "端点必须是 http(s)",
    ).toBe(false)
    expect(guardAiProviderProfile({ ...profile, endpoint: "C:\\gateway" })).toBe(false)
    expect(guardAiProviderProfile({ ...profile, revision: "" })).toBe(false)
  })

  it("rejects symbol and non-enumerable keys that Object.keys would miss", () => {
    const profile = { ...profileListNormal.profiles[0] } as Record<string | symbol, unknown>
    profile[Symbol("secret")] = "sk-not-a-real-key"
    expect(guardAiProviderProfile(profile)).toBe(false)

    const hidden = { ...profileListNormal.profiles[0] }
    Object.defineProperty(hidden, "apiKey", {
      value: "sk-not-a-real-key",
      enumerable: false,
    })
    expect(guardAiProviderProfile(hidden)).toBe(false)
  })

  it("keeps capability tri-state distinct and never accepts a guessed value", () => {
    const model = modelsReady.models[0]
    expect(guardAiProviderModel(model)).toBe(true)
    expect(guardAiProviderModel({ ...model, vision: "likely" })).toBe(false)
    expect(guardAiProviderModel({ ...model, chat: true })).toBe(false)
    // unknown 必须与 unsupported 一样是合法值，且互不相等。
    expect(guardAiProviderModel({ ...model, vision: "unsupported" })).toBe(true)
    expect(capabilityLabel("unknown")).toBe("未声明")
    expect(capabilityLabel("unsupported")).toBe("已声明不支持")
    expect(capabilityLabel("supported")).toBe("已声明支持")
  })

  it("keeps catalog state and models mutually consistent", () => {
    // ready 但目录为空是自相矛盾的响应。
    expect(guardAiProviderModelsCatalog({ ...modelsReady, models: [] }, PROFILE_ID)).toBe(false)
    // 非 ready 却带模型同样是伪造。
    expect(
      guardAiProviderModelsCatalog(
        { ...modelsNoCredential, models: modelsReady.models },
        modelsNoCredential.profileId,
      ),
    ).toBe(false)
    expect(
      guardAiProviderModelsCatalog({ ...modelsReady, state: "unknown_state" }, PROFILE_ID),
    ).toBe(false)
    expect(
      guardAiProviderModelsCatalog(
        {
          ...modelsReady,
          models: [{ ...modelsReady.models[0], secret: "sk-not-a-real-key" }],
        },
        PROFILE_ID,
      ),
    ).toBe(false)
  })

  it("rejects credential status shapes that could carry a secret", () => {
    expect(guardCredentialStatus({ configured: true, updatedAt: null })).toBe(true)
    expect(guardCredentialStatus({ configured: false, updatedAt: "2026-09-18T00:00:00Z" })).toBe(true)
    expect(guardCredentialStatus({ configured: true, updatedAt: null, secret: "sk-x" })).toBe(false)
    expect(guardCredentialStatus({ configured: "yes", updatedAt: null })).toBe(false)
  })

  it("only treats a ready non-empty catalog as having available models", () => {
    expect(catalogHasModels(modelsReady as never)).toBe(true)
    for (const empty of [modelsNoCredential, modelsDisabled, modelsEmpty]) {
      expect(catalogHasModels(empty as never)).toBe(false)
    }
    expect(catalogHasModels(null)).toBe(false)
    expect(toModelOptions(modelsNoCredential as never)).toEqual([])
    expect(toModelOptions(modelsReady as never).map((option) => option.modelId))
      .toEqual(["gpt-4o-mini", "text-embedding-3-small"])
  })

  it("labels the empty state honestly for every reason", () => {
    expect(NO_AVAILABLE_MODEL).toBe("无可用模型")
    // 「还没选」必须与「无可用模型」是两个不同的文案。
    expect(NO_MODEL_SELECTED).toBe("未选择")
    expect(NO_MODEL_SELECTED).not.toBe(NO_AVAILABLE_MODEL)
    expect(catalogStateHint("no_credential")).toContain("API Key")
    expect(catalogStateHint("disabled")).toContain("停用")
    expect(catalogStateHint("empty")).toContain("空模型列表")
    expect(catalogStateHint(null)).toContain("尚未读取")
    // ready 不产生空态文案。
    expect(catalogStateHint("ready")).toBe("")
  })
})

/**
 * 身份边界：**响应的身份必须等于请求的身份**。
 *
 * 形状守卫只能回答"这是一份合法的 profile"，回答不了"这是我刚请求的那一份"。
 * 少了这条比对，一次错配的响应（迟到的旧请求、重复派发、服务端 id 混用）会被当成答案：
 * 写入路径据它把界面切到另一个 Provider（Hook 随即 `load(saved.profileId)`），读取路径把
 * 别人的 endpoint 与模型交回调用方——两条路径上界面的显示都没有任何区别。
 */
describe("AI Provider 网关的身份边界", () => {
  const PROFILE = profileListNormal.profiles[0]
  const OTHER_PROFILE_ID = "another-provider"

  beforeEach(() => {
    mocks.getHavenClient.mockReset()
  })

  it("参数读取拒绝属于另一个 profile 的响应", async () => {
    mocks.getHavenClient.mockReturnValue({
      aiProviderProfileGet: vi.fn(async () => ({ ...PROFILE, profileId: OTHER_PROFILE_ID })),
    })
    // 拒绝是稳定错误，且文案不回显被拒绝的载荷内容（这里连 id 都不出现）。
    const mismatch = await getAiProviderProfile(PROFILE.profileId).then(
      () => null,
      (error: unknown) => error as { code?: string; retryable?: boolean; message?: string },
    )
    expect(mismatch?.code).toBe("INTERNAL_ERROR")
    expect(mismatch?.retryable).toBe(false)
    expect(mismatch?.message ?? "").not.toContain(OTHER_PROFILE_ID)

    // 反向证据：同一份载荷在匹配的 id 下必须通过，否则上面的拒绝可以靠"一律拒绝"通过。
    mocks.getHavenClient.mockReturnValue({
      aiProviderProfileGet: vi.fn(async () => PROFILE),
    })
    await expect(getAiProviderProfile(PROFILE.profileId)).resolves.toMatchObject({
      profileId: PROFILE.profileId,
    })
  })

  it("写入拒绝身份不符的响应", async () => {
    const request = {
      profileId: PROFILE.profileId,
      displayName: PROFILE.displayName,
      kind: PROFILE.kind,
      endpoint: PROFILE.endpoint,
      enabled: PROFILE.enabled,
      selectedModelId: PROFILE.selectedModelId,
      expectedRevision: PROFILE.revision,
    }
    mocks.getHavenClient.mockReturnValue({
      aiProviderProfileUpsert: vi.fn(async () => ({ ...PROFILE, profileId: OTHER_PROFILE_ID })),
    })
    await expect(upsertAiProviderProfile(request as never)).rejects.toMatchObject({
      code: "INTERNAL_ERROR",
    })

    mocks.getHavenClient.mockReturnValue({
      aiProviderProfileUpsert: vi.fn(async () => PROFILE),
    })
    await expect(upsertAiProviderProfile(request as never)).resolves.toMatchObject({
      profileId: PROFILE.profileId,
    })
  })

  it("删除拒绝属于另一个 profile 的结果", async () => {
    mocks.getHavenClient.mockReturnValue({
      aiProviderProfileDelete: vi.fn(async () => ({
        profileId: OTHER_PROFILE_ID,
        credentialDeleted: true,
      })),
    })
    await expect(
      deleteAiProviderProfile(PROFILE.profileId, PROFILE.revision),
    ).rejects.toMatchObject({ code: "INTERNAL_ERROR" })

    mocks.getHavenClient.mockReturnValue({
      aiProviderProfileDelete: vi.fn(async () => ({
        profileId: PROFILE.profileId,
        credentialDeleted: true,
      })),
    })
    await expect(
      deleteAiProviderProfile(PROFILE.profileId, PROFILE.revision),
    ).resolves.toEqual({ profileId: PROFILE.profileId, credentialDeleted: true })
  })
})
