// @vitest-environment jsdom

import { act, renderHook, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type {
  AiProviderModelsCatalogDto,
  AiProviderProfileDto,
} from "@/lib/ipc/generated/wire"

const gateway = vi.hoisted(() => ({
  clearAiProviderApiKey: vi.fn(),
  deleteAiProviderProfile: vi.fn(),
  listAiProviderModels: vi.fn(),
  listAiProviderProfiles: vi.fn(),
  setAiProviderApiKey: vi.fn(),
  toModelOptions: vi.fn(() => []),
  upsertAiProviderProfile: vi.fn(),
  catalogStateHint: vi.fn(() => "暂无模型"),
}))

// 用 `importOriginal` 保留真实的常量与纯函数（`isHttpsAiEndpoint`、
// `INSECURE_ENDPOINT_CODE` …），只替换真正需要打桩的 IO。
//
// 为什么不再手写整个模块：手写替身必须逐项复制模块的导出面，漏掉一个（这里曾经漏掉
// `isHttpsAiEndpoint`）就会让 Hook 在运行时拿到 `undefined is not a function`，
// 而失败现象是"未处理的异常"，与真正的断言失败混在一起，读起来完全不像 Jest 的报错。
vi.mock("../ipc/ai-provider-gateway", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc/ai-provider-gateway")>()
  return { ...actual, ...gateway }
})

import { useAiProviderSettings } from "./useAiProviderSettings"

const PROFILE: AiProviderProfileDto = {
  schemaVersion: 1,
  profileId: "provider-main",
  displayName: "主服务",
  kind: "openai_compatible",
  endpoint: "https://gateway.example.invalid/v1",
  enabled: true,
  selectedModelId: null,
  credentialConfigured: false,
  revision: "revision-1",
  createdAt: "2026-09-01T00:00:00Z",
  updatedAt: "2026-09-01T00:00:00Z",
}

const SECOND_PROFILE: AiProviderProfileDto = {
  ...PROFILE,
  profileId: "provider-second",
  displayName: "第二服务",
  revision: "revision-2",
}

const EMPTY_CATALOG: AiProviderModelsCatalogDto = {
  schemaVersion: 1,
  profileId: PROFILE.profileId,
  state: "empty",
  models: [],
}

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((complete, fail) => {
    resolve = complete
    reject = fail
  })
  return { promise, resolve, reject }
}

beforeEach(() => {
  for (const method of Object.values(gateway)) {
    if (typeof method === "function" && "mockReset" in method) method.mockReset()
  }
  gateway.toModelOptions.mockReturnValue([])
  gateway.catalogStateHint.mockReturnValue("暂无模型")
  gateway.listAiProviderProfiles.mockResolvedValue([PROFILE])
  gateway.listAiProviderModels.mockResolvedValue(EMPTY_CATALOG)
})

describe("useAiProviderSettings 并发与读取失败", () => {
  it("模型刷新不会使同时进行的 Provider 列表读取失效", async () => {
    const { result } = renderHook(() => useAiProviderSettings())
    await waitFor(() => expect(result.current.state.loading).toBe(false))

    const profiles = deferred<AiProviderProfileDto[]>()
    const catalog = deferred<AiProviderModelsCatalogDto>()
    gateway.listAiProviderProfiles.mockReturnValueOnce(profiles.promise)
    gateway.listAiProviderModels.mockReturnValueOnce(catalog.promise)

    let reload!: Promise<void>
    let refresh!: Promise<void>
    act(() => {
      reload = result.current.reload()
      refresh = result.current.refreshModels()
    })

    await act(async () => {
      profiles.resolve([PROFILE])
      await reload
    })
    expect(result.current.state.loading).toBe(false)
    expect(result.current.state.profiles).toEqual([PROFILE])

    await act(async () => {
      catalog.resolve(EMPTY_CATALOG)
      await refresh
    })
    expect(result.current.state.catalogLoading).toBe(false)
    expect(result.current.state.catalog).toEqual(EMPTY_CATALOG)
  })

  it("列表读取失败保留最后一次成功快照并报告错误，不伪装成空配置", async () => {
    const { result } = renderHook(() => useAiProviderSettings())
    await waitFor(() => expect(result.current.selectedProfile?.profileId).toBe(PROFILE.profileId))

    gateway.listAiProviderProfiles.mockRejectedValueOnce({
      code: "AI_PROVIDER_UNAVAILABLE",
      userMessage: "读取 Provider 失败",
      retryable: true,
    })
    await act(async () => result.current.reload())

    expect(result.current.state.loading).toBe(false)
    expect(result.current.state.loadError?.message).toBe("读取 Provider 失败")
    expect(result.current.state.profiles).toEqual([PROFILE])
    expect(result.current.selectedProfile?.profileId).toBe(PROFILE.profileId)
  })

  it("新建模式使用 create-only CAS，不把重复 ID 当成更新", async () => {
    const { result } = renderHook(() => useAiProviderSettings())
    await waitFor(() => expect(result.current.state.loading).toBe(false))

    await act(async () => {
      await expect(result.current.saveProfile({
        profileId: PROFILE.profileId,
        displayName: "替换服务",
        endpoint: "https://different.example.invalid/v1",
        enabled: true,
        selectedModelId: null,
        createOnly: true,
      })).resolves.toBe(false)
    })

    expect(gateway.upsertAiProviderProfile).not.toHaveBeenCalled()
    expect(result.current.state.saveError?.message).toContain("以免覆盖现有服务")
  })

  it("API Key 写入成功后回读凭据状态，不在前端乐观置为已配置", async () => {
    const configured = { ...PROFILE, credentialConfigured: true }
    gateway.listAiProviderProfiles
      .mockResolvedValueOnce([PROFILE])
      .mockResolvedValueOnce([configured])
    gateway.setAiProviderApiKey.mockResolvedValue(undefined)
    const { result } = renderHook(() => useAiProviderSettings())
    await waitFor(() => expect(result.current.selectedProfile?.profileId).toBe(PROFILE.profileId))

    await act(async () => {
      await expect(result.current.submitApiKey("secret-value")).resolves.toBe(true)
    })

    expect(gateway.listAiProviderProfiles).toHaveBeenCalledTimes(2)
    expect(result.current.selectedProfile?.credentialConfigured).toBe(true)
  })

  it("API Key 写入响应丢失时回读后以系统凭据管理器事实收敛", async () => {
    const configured = { ...PROFILE, credentialConfigured: true }
    gateway.listAiProviderProfiles
      .mockResolvedValueOnce([PROFILE])
      .mockResolvedValueOnce([configured])
    gateway.setAiProviderApiKey.mockRejectedValueOnce({
      code: "CREDENTIAL_ACCESS_FAILED",
      userMessage: "写入结果未确认",
      retryable: true,
    })
    const { result } = renderHook(() => useAiProviderSettings())
    await waitFor(() => expect(result.current.selectedProfile?.profileId).toBe(PROFILE.profileId))

    await act(async () => {
      await expect(result.current.submitApiKey("secret-value")).resolves.toBe(false)
    })

    expect(result.current.selectedProfile?.credentialConfigured).toBe(true)
    expect(result.current.state.saveError?.message).toBe("写入结果未确认")
  })

  it("凭据写入成功后重新拉取模型目录，不再沿用旧空目录", async () => {
    const configured = { ...PROFILE, credentialConfigured: true }
    gateway.listAiProviderProfiles
      .mockResolvedValueOnce([PROFILE])
      .mockResolvedValueOnce([configured])
    gateway.setAiProviderApiKey.mockResolvedValue(undefined)
    const { result } = renderHook(() => useAiProviderSettings())
    await waitFor(() => expect(result.current.selectedProfile?.profileId).toBe(PROFILE.profileId))
    await waitFor(() => expect(result.current.state.catalogLoading).toBe(false))
    const callsBeforeWrite = gateway.listAiProviderModels.mock.calls.length

    await act(async () => {
      await expect(result.current.submitApiKey("secret-value")).resolves.toBe(true)
    })

    // 凭据是否已配置是模型目录的输入之一：写入成功后必须重新拉取，而不是继续显示
    // 写入之前那份 `no_credential` 的空目录（模型选择器会因此一直禁用）。
    await waitFor(() =>
      expect(gateway.listAiProviderModels.mock.calls.length).toBeGreaterThan(callsBeforeWrite),
    )
    expect(gateway.listAiProviderModels).toHaveBeenLastCalledWith(PROFILE.profileId)
  })

  it("清空选择会同步结束旧模型请求的加载态", async () => {
    const { result } = renderHook(() => useAiProviderSettings())
    await waitFor(() => expect(result.current.selectedProfile?.profileId).toBe(PROFILE.profileId))

    const catalog = deferred<AiProviderModelsCatalogDto>()
    gateway.listAiProviderModels.mockReturnValueOnce(catalog.promise)
    let refresh!: Promise<void>
    act(() => { refresh = result.current.refreshModels() })
    expect(result.current.state.catalogLoading).toBe(true)

    act(() => result.current.selectProfile(null))
    expect(result.current.state.catalogLoading).toBe(false)
    await act(async () => {
      catalog.resolve(EMPTY_CATALOG)
      await refresh
    })
    expect(result.current.state.catalog).toBeNull()
  })
})

/**
 * 选中的 Provider 必须**跨分区切换**存活。
 *
 * 设置页的每个分区各自挂载自己的组件：从「智能功能」切到「阅读」再切回来，
 * `AiSettings` 与它内部的 Hook 会一起重新挂载。选择只留在 Hook 内部时，重挂载会静默
 * 回落到 `profiles[0]`——用户选过第二个配置，回来变成第一个，而「打开栖伴」取的正是
 * 这个值，于是外部模型请求被发给了另一个 Provider，界面上没有任何提示。
 *
 * 因此 Hook 必须接受一个"调用方记住的选择"，并在自己的选择变化时上报。
 */
describe("useAiProviderSettings 的选择跨重新挂载存活", () => {
  it("首次读取落在调用方记住的那个配置上，而不是列表第一项", async () => {
    // 列表第一项是 PROFILE，记住的是第二项：回落到第一项就是缺陷本身。
    gateway.listAiProviderProfiles.mockResolvedValue([PROFILE, SECOND_PROFILE])
    const { result } = renderHook(() =>
      useAiProviderSettings({ initialProfileId: SECOND_PROFILE.profileId }),
    )
    await waitFor(() => expect(result.current.state.loading).toBe(false))
    expect(result.current.selectedProfile?.profileId).toBe(SECOND_PROFILE.profileId)

    // 反向证据：没有记忆时就是列表第一项——否则上面可能只是"碰巧"。
    const plain = renderHook(() => useAiProviderSettings())
    await waitFor(() => expect(plain.result.current.state.loading).toBe(false))
    expect(plain.result.current.selectedProfile?.profileId).toBe(PROFILE.profileId)
  })

  it("记住的配置已经不存在时收敛到列表第一项，而不是选中一个空 id", async () => {
    gateway.listAiProviderProfiles.mockResolvedValue([PROFILE])
    const { result } = renderHook(() =>
      useAiProviderSettings({ initialProfileId: "provider-deleted" }),
    )
    await waitFor(() => expect(result.current.state.loading).toBe(false))
    expect(result.current.selectedProfile?.profileId).toBe(PROFILE.profileId)
  })

  it("显式选择与读取收敛都会上报给调用方", async () => {
    gateway.listAiProviderProfiles.mockResolvedValue([PROFILE, SECOND_PROFILE])
    const onSelectedProfileChange = vi.fn()
    const { result } = renderHook(() =>
      useAiProviderSettings({ onSelectedProfileChange }),
    )
    await waitFor(() => expect(result.current.state.loading).toBe(false))
    // 读取收敛的结果同样要上报：调用方记住的必须是"用户实际看到的那个"。
    expect(onSelectedProfileChange).toHaveBeenLastCalledWith(PROFILE.profileId)

    act(() => result.current.selectProfile(SECOND_PROFILE.profileId))
    expect(onSelectedProfileChange).toHaveBeenLastCalledWith(SECOND_PROFILE.profileId)

    // 清空也是一个明确的选择，同样要记住——否则重新挂载又会自作主张地选回第一项。
    act(() => result.current.selectProfile(null))
    expect(onSelectedProfileChange).toHaveBeenLastCalledWith(null)
  })
})

/**
 * 凭据的写入与清除**不能并发**。
 *
 * 两条写操作改的是同一个凭据：同时放行时，最终状态由两个请求的完成顺序决定，而界面显示的
 * 是最后一次回读——用户会看到"未配置"，而系统凭据管理器里其实写着密钥（或反过来）。
 * UI 也会禁用按钮，但 Hook 必须自守：双击、快速切换或别的调用方都不该绕过这道闸门。
 */
describe("useAiProviderSettings 凭据写入与清除互斥", () => {
  it("写入在途时清除被拒绝，且顺序反过来同样成立", async () => {
    gateway.listAiProviderProfiles.mockResolvedValue([PROFILE])
    const write = deferred<void>()
    gateway.setAiProviderApiKey.mockReturnValue(write.promise)
    const { result } = renderHook(() => useAiProviderSettings())
    await waitFor(() => expect(result.current.selectedProfile?.profileId).toBe(PROFILE.profileId))

    let setResult!: Promise<boolean>
    let clearResult!: Promise<boolean>
    act(() => {
      setResult = result.current.submitApiKey("secret-value")
      clearResult = result.current.clearApiKey()
    })

    // 清除被闸门拒绝，且**没有**发出任何请求。
    await expect(clearResult).resolves.toBe(false)
    expect(gateway.clearAiProviderApiKey).not.toHaveBeenCalled()

    await act(async () => {
      write.resolve()
      await expect(setResult).resolves.toBe(true)
    })
    expect(gateway.setAiProviderApiKey).toHaveBeenCalledTimes(1)

    // 闸门在写入结束后释放（不是永久卡死）：清除这时才能真的执行。
    gateway.clearAiProviderApiKey.mockResolvedValue(undefined)
    await act(async () => {
      await expect(result.current.clearApiKey()).resolves.toBe(true)
    })
    expect(gateway.clearAiProviderApiKey).toHaveBeenCalledTimes(1)
  })

  it("清除在途时写入被拒绝，凭据不会在清除之后又被写回去", async () => {
    gateway.listAiProviderProfiles.mockResolvedValue([PROFILE])
    const clear = deferred<void>()
    gateway.clearAiProviderApiKey.mockReturnValue(clear.promise)
    const { result } = renderHook(() => useAiProviderSettings())
    await waitFor(() => expect(result.current.selectedProfile?.profileId).toBe(PROFILE.profileId))

    let clearResult!: Promise<boolean>
    let setResult!: Promise<boolean>
    act(() => {
      clearResult = result.current.clearApiKey()
      setResult = result.current.submitApiKey("secret-value")
    })

    await expect(setResult).resolves.toBe(false)
    expect(gateway.setAiProviderApiKey).not.toHaveBeenCalled()

    await act(async () => {
      clear.resolve()
      await expect(clearResult).resolves.toBe(true)
    })
    expect(gateway.clearAiProviderApiKey).toHaveBeenCalledTimes(1)
  })
})

/**
 * 「重试」必须重做**失败的那件事**。
 *
 * 曾经的界面把保存失败的"重试"接到"拉取模型"上：用户点一次"重试"，看到的是一次网络请求
 * 和一份新目录，而真正失败的写入仍然没有发生——一次被误认为成功的失败，比直接报错更糟。
 */
describe("useAiProviderSettings 的重试语义", () => {
  it("保存失败后的重试重放同一份草稿，而不是去拉模型", async () => {
    gateway.listAiProviderProfiles.mockResolvedValue([PROFILE])
    gateway.upsertAiProviderProfile.mockRejectedValueOnce({
      code: "AI_PROVIDER_CAS_CONFLICT",
      userMessage: "配置已被改动",
      retryable: true,
    })
    const { result } = renderHook(() => useAiProviderSettings())
    await waitFor(() => expect(result.current.selectedProfile?.profileId).toBe(PROFILE.profileId))

    const draft = {
      profileId: PROFILE.profileId,
      displayName: "改过的名字",
      endpoint: PROFILE.endpoint,
      enabled: true,
      selectedModelId: null,
    }
    await act(async () => {
      await expect(result.current.saveProfile(draft)).resolves.toBe(false)
    })
    expect(result.current.state.saveErrorAction).toBe("profile")
    expect(result.current.canRetrySaveError).toBe(true)

    gateway.upsertAiProviderProfile.mockResolvedValueOnce({ ...PROFILE, displayName: "改过的名字" })
    await act(async () => {
      await expect(result.current.retrySaveError()).resolves.toBe(true)
    })

    // 关键是**重放了写入**：第二次调用与第一次逐字相同（同一份草稿，不是重新拼一份）。
    expect(gateway.upsertAiProviderProfile).toHaveBeenCalledTimes(2)
    expect(gateway.upsertAiProviderProfile.mock.calls[1]?.[0]).toEqual(
      gateway.upsertAiProviderProfile.mock.calls[0]?.[0],
    )
    expect(gateway.upsertAiProviderProfile.mock.calls[1]?.[0]).toMatchObject({
      displayName: "改过的名字",
    })
  })

  it("凭据写入失败不提供重试，也不会拿「拉取模型」冒充重试", async () => {
    gateway.listAiProviderProfiles.mockResolvedValue([PROFILE])
    gateway.setAiProviderApiKey.mockRejectedValueOnce({
      code: "CREDENTIAL_ACCESS_FAILED",
      userMessage: "写入结果未确认",
      retryable: true,
    })
    const { result } = renderHook(() => useAiProviderSettings())
    await waitFor(() => expect(result.current.selectedProfile?.profileId).toBe(PROFILE.profileId))

    await act(async () => {
      await expect(result.current.submitApiKey("secret-value")).resolves.toBe(false)
    })
    expect(result.current.state.saveErrorAction).toBe("credential")
    // 密钥原文按设计不留存，因此这条失败**无法重放**——即使后端说它可重试。
    expect(result.current.canRetrySaveError).toBe(false)

    const modelCallsBeforeRetry = gateway.listAiProviderModels.mock.calls.length
    await act(async () => {
      await expect(result.current.retrySaveError()).resolves.toBe(false)
    })
    // 它没有去做别的事（一次模型刷新正是最容易被误当成"重试成功了"的那种假动作）。
    expect(gateway.listAiProviderModels.mock.calls.length).toBe(modelCallsBeforeRetry)
    expect(gateway.setAiProviderApiKey).toHaveBeenCalledTimes(1)
  })

  /**
   * 删除的重试必须重放**失败的那一份**。
   *
   * 曾经的实现让 `retrySaveError` 直接调用 `deleteProfile()`，而后者读的是"此刻选中的
   * profile"。删除失败之后用户完全可以切到另一个 profile 去看别的配置——此时点「重试」，
   * 被删掉的就是他刚刚切过去的那一份，而界面上没有任何东西提示这件事。这是一次静默的
   * 数据丢失，也是"重试"这个词在这里唯一不能有的含义。
   */
  it("删除失败后的重试删的是原来那一份，即使中途切换了 profile", async () => {
    gateway.listAiProviderProfiles.mockResolvedValue([PROFILE, SECOND_PROFILE])
    gateway.deleteAiProviderProfile.mockRejectedValueOnce({
      code: "AI_PROVIDER_CAS_CONFLICT",
      userMessage: "配置已被改动",
      retryable: true,
    })
    const { result } = renderHook(() => useAiProviderSettings())
    await waitFor(() => expect(result.current.selectedProfile?.profileId).toBe(PROFILE.profileId))

    await act(async () => {
      await expect(result.current.deleteProfile()).resolves.toBe(false)
    })
    expect(result.current.state.saveErrorAction).toBe("delete")
    expect(result.current.canRetrySaveError).toBe(true)
    // 第一次删的是当时选中的那一份，用的是它当时的 revision（服务端 CAS 的依据）。
    expect(gateway.deleteAiProviderProfile).toHaveBeenCalledWith(PROFILE.profileId, PROFILE.revision)

    // 删除失败之后用户切到了第二个 profile——这正是"重试按当前选中项"会删错人的场景。
    act(() => result.current.selectProfile(SECOND_PROFILE.profileId))
    expect(result.current.selectedProfile?.profileId).toBe(SECOND_PROFILE.profileId)

    gateway.deleteAiProviderProfile.mockResolvedValueOnce(undefined)
    await act(async () => {
      await expect(result.current.retrySaveError()).resolves.toBe(true)
    })

    expect(gateway.deleteAiProviderProfile).toHaveBeenCalledTimes(2)
    // 关键断言：第二次删的仍然是失败的那一份（连同它当时的 revision），不是新选中的那一份。
    expect(gateway.deleteAiProviderProfile.mock.calls[1]).toEqual([
      PROFILE.profileId,
      PROFILE.revision,
    ])
    expect(gateway.deleteAiProviderProfile.mock.calls[1]?.[0]).not.toBe(SECOND_PROFILE.profileId)
  })

  it("关掉错误提示之后不再保留可重放的删除目标", async () => {
    gateway.listAiProviderProfiles.mockResolvedValue([PROFILE, SECOND_PROFILE])
    gateway.deleteAiProviderProfile.mockRejectedValueOnce({
      code: "AI_PROVIDER_CAS_CONFLICT",
      userMessage: "配置已被改动",
      retryable: true,
    })
    const { result } = renderHook(() => useAiProviderSettings())
    await waitFor(() => expect(result.current.selectedProfile?.profileId).toBe(PROFILE.profileId))

    await act(async () => {
      await expect(result.current.deleteProfile()).resolves.toBe(false)
    })
    act(() => result.current.dismissSaveError())
    expect(result.current.canRetrySaveError).toBe(false)

    act(() => result.current.selectProfile(SECOND_PROFILE.profileId))
    // 提示关掉之后就没有重试入口了；随后即便用户切了 profile，也不该再发出任何删除请求。
    await act(async () => {
      await expect(result.current.retrySaveError()).resolves.toBe(false)
    })
    expect(gateway.deleteAiProviderProfile).toHaveBeenCalledTimes(1)
  })
})
