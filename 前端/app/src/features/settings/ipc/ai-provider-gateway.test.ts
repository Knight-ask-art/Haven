import { describe, expect, it } from "vitest"

import profileListNormal from "../../../../../../contracts/ipc/v1/fixtures/ai-provider/profile.list.normal.json"
import profileListEmpty from "../../../../../../contracts/ipc/v1/fixtures/ai-provider/profile.list.empty.json"
import modelsReady from "../../../../../../contracts/ipc/v1/fixtures/ai-provider/models.catalog.normal.json"
import modelsNoCredential from "../../../../../../contracts/ipc/v1/fixtures/ai-provider/models.catalog.no-credential.json"
import modelsDisabled from "../../../../../../contracts/ipc/v1/fixtures/ai-provider/models.catalog.disabled.json"
import modelsEmpty from "../../../../../../contracts/ipc/v1/fixtures/ai-provider/models.catalog.empty.json"
import {
  NO_AVAILABLE_MODEL,
  NO_MODEL_SELECTED,
  catalogHasModels,
  catalogStateHint,
  capabilityLabel,
  guardAiProviderModel,
  guardAiProviderModelsCatalog,
  guardAiProviderProfile,
  guardAiProviderProfileList,
  guardCredentialStatus,
  toModelOptions,
} from "./ai-provider-gateway"

describe("AI Provider wire runtime guards", () => {
  it("accepts the shared fixtures", () => {
    expect(guardAiProviderProfileList(profileListNormal)).toBe(true)
    expect(guardAiProviderProfileList(profileListEmpty)).toBe(true)
    expect(guardAiProviderModelsCatalog(modelsReady)).toBe(true)
    expect(guardAiProviderModelsCatalog(modelsNoCredential)).toBe(true)
    expect(guardAiProviderModelsCatalog(modelsDisabled)).toBe(true)
    expect(guardAiProviderModelsCatalog(modelsEmpty)).toBe(true)
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
    expect(guardAiProviderModelsCatalog({ ...modelsReady, models: [] })).toBe(false)
    // 非 ready 却带模型同样是伪造。
    expect(
      guardAiProviderModelsCatalog({ ...modelsNoCredential, models: modelsReady.models }),
    ).toBe(false)
    expect(guardAiProviderModelsCatalog({ ...modelsReady, state: "unknown_state" })).toBe(false)
    expect(
      guardAiProviderModelsCatalog({
        ...modelsReady,
        models: [{ ...modelsReady.models[0], secret: "sk-not-a-real-key" }],
      }),
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
