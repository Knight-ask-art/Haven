// @vitest-environment jsdom
//
// 设置页 AI Provider 分组的写入路径（A2 基础切片）。
//
// 与 `components/ai-assistant/AiAssistantDialog.test.tsx` 里的只读空态用例分开：
// 这些用例会真的写 Mock 的 Provider 状态，而 `getHavenClient()` 是进程内单例。
// Vitest 默认按文件隔离模块注册表，因此这里能拿到一个干净的客户端。
//
// 断言的安全边界：
// - 没有配置前不出现任何示例地址或模型名；
// - API Key 只单向提交：提交后 DOM 里不再有原文，界面只显示「已配置」；
// - 模型只能来自 Provider 目录，能力 「未声明」不与「不支持」混淆；
// - 删除配置回到空态。

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { useEffect } from "react"
import { MemoryRouter, Route, Routes, useNavigate } from "react-router"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { SettingsPage } from "./SettingsPage"
import { getHavenClient } from "@/lib/ipc/runtime"
import * as runtime from "@/lib/ipc/runtime"
import { HavenError } from "@/lib/ipc/errors"
import { NoticeContext, type NoticeInput } from "@/app/notice-center/notice-context"

const PROFILE_ID = "gw-main"
const ENDPOINT = "https://gateway.example.invalid/v1"
const API_KEY = "sk-ui-not-a-real-key"

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

/**
 * 每个用例都从「没有任何 Provider 配置」开始。
 *
 * `getHavenClient()` 是进程内单例（页面与 Hook 都依赖这一点），因此前一个用例写下的
 * Provider 状态会留给下一个用例。这里通过公开的客户端 API 清干净，而不是绕到内部状态，
 * 顺便也证明「删除配置」确实把行和凭据一起清掉了。
 */
beforeEach(async () => {
  const client = getHavenClient()
  const { profiles } = await client.aiProviderProfileList()
  for (const profile of profiles) {
    await client.aiProviderProfileDelete({
      profileId: profile.profileId,
      expectedRevision: profile.revision,
    })
  }
})

function renderAiWorkbench() {
  return render(
    <MemoryRouter initialEntries={["/settings/ai"]}>
      <Routes>
        <Route path="/settings/:section" element={<SettingsPage />} />
      </Routes>
    </MemoryRouter>,
  )
}

function renderAiSettings() {
  const rendered = renderAiWorkbench()
  fireEvent.click(screen.getByRole("button", { name: "AI 设置" }))
  return rendered
}

describe("AI 工作台导航入口", () => {
  it("默认把工作台作为页面主视图呈现，并提供历史、执行记录和设置入口", async () => {
    renderAiWorkbench()

    expect(await screen.findByRole("button", { name: "新会话" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "提案历史" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "执行记录" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "AI 设置" })).toBeTruthy()
    expect(screen.queryByRole("dialog")).toBeNull()
    expect(screen.queryByText("当前配置")).toBeNull()
    expect(screen.queryByRole("button", { name: /内置技能/ })).toBeNull()
  })
})

/** 通过界面新建一个 Provider 配置（不触碰任何密钥）。 */
async function createProfile() {
  await createProfileWith(PROFILE_ID, ENDPOINT)
}

/** 同上，但显式指定配置 id 与地址（用于造出"列表里不止一项"的现场）。 */
async function createProfileWith(profileId: string, endpoint: string) {
  fireEvent.click(await screen.findByRole("button", { name: "新建配置" }))
  fireEvent.change(screen.getByLabelText("配置 ID"), { target: { value: profileId } })
  fireEvent.change(screen.getByLabelText("API 地址"), { target: { value: endpoint } })
  fireEvent.click(screen.getByRole("button", { name: "保存" }))
  // 保存成功后草稿收起，配置行渲染出来。
  await waitFor(() => expect(screen.queryByLabelText("配置 ID")).toBeNull())
  // 列表回读的 loading 分支也会暂时隐藏 ID；等待真正的草稿收敛。
  await waitFor(() => expect(screen.queryByTestId("ai-profile-draft-notice")).toBeNull())
}

describe("设置页 AI Provider 写入路径", () => {
  it("新建配置后凭据显示未配置，且不伪造模型或示例地址", async () => {
    renderAiSettings()
    await createProfile()

    // 凭据事实只有「未配置」——没有密钥就没有模型目录。
    expect(await screen.findByText("未配置")).toBeTruthy()
    expect(screen.queryByText("已配置")).toBeNull()
    expect(document.body.textContent).not.toContain("api.example.com")

    const trigger = (await screen.findByRole("button", { name: "默认模型" })) as HTMLButtonElement
    expect(trigger.disabled).toBe(true)
    expect(trigger.textContent).toContain("无可用模型")
    for (const fakeModel of ["gpt-4o", "claude-compatible", "vision-compatible"]) {
      expect(document.body.textContent).not.toContain(fakeModel)
    }
  })

  it("API Key 单向提交：提交后显示已配置，DOM 里不再有原文", async () => {
    renderAiSettings()
    await createProfile()

    // 打开编辑器前，密钥输入框不存在，也没有任何回填。
    expect(screen.queryByLabelText("API Key")).toBeNull()

    fireEvent.click(await screen.findByRole("button", { name: "配置" }))
    const input = (await screen.findByLabelText("API Key")) as HTMLInputElement
    fireEvent.change(input, { target: { value: API_KEY } })
    expect(input.value).toBe(API_KEY)
    fireEvent.click(screen.getByRole("button", { name: "保存" }))

    // 提交后：只显示「已配置」，输入框销毁，原文不残留在 DOM 里。
    expect(await screen.findByText("已配置")).toBeTruthy()
    expect(screen.queryByLabelText("API Key")).toBeNull()
    expect(document.body.textContent).not.toContain(API_KEY)

    // 有密钥后才出现来自 Provider 目录的模型；能力值原样呈现，「未声明」不写成「不支持」。
    const trigger = (await screen.findByRole("button", { name: "默认模型" })) as HTMLButtonElement
    await waitFor(() => expect(trigger.disabled).toBe(false))
    // 目录非空但还没选过 → 「未选择」；这里显示「无可用模型」就是谎报。
    expect(trigger.textContent).toContain("未选择")
    expect(trigger.textContent).not.toContain("无可用模型")

    fireEvent.click(trigger)
    expect(await screen.findByRole("listbox")).toBeTruthy()
    expect(document.body.textContent).toContain("GPT-4o mini")
    expect(document.body.textContent).toContain("识图：未声明")
  })

  it("删除配置后凭据与模型一起清空，回到无可用模型", async () => {
    renderAiSettingsWithNotices([], true)
    await createProfile()

    fireEvent.click(await screen.findByRole("button", { name: "配置" }))
    fireEvent.change(await screen.findByLabelText("API Key"), { target: { value: API_KEY } })
    fireEvent.click(screen.getByRole("button", { name: "保存" }))
    expect(await screen.findByText("已配置")).toBeTruthy()

    fireEvent.click(screen.getByRole("button", { name: "删除配置" }))

    // 回到「没有 Provider」的空态：没有配置行、没有凭据、没有模型。
    expect(await screen.findByRole("button", { name: "新建配置" })).toBeTruthy()
    await waitFor(() => expect(screen.queryByText("已配置")).toBeNull())
    expect(screen.queryByLabelText("API 地址")).toBeNull()
    expect(document.body.textContent).not.toContain("GPT-4o mini")
    expect(document.body.textContent).not.toContain(API_KEY)
  })

  it("新建配置不能重用现有 ID 覆盖旧 Provider", async () => {
    renderAiSettings()
    await createProfile()

    fireEvent.click(screen.getByRole("button", { name: "新建配置" }))
    fireEvent.change(screen.getByLabelText("配置 ID"), { target: { value: PROFILE_ID } })
    fireEvent.change(screen.getByLabelText("API 地址"), {
      target: { value: "https://replacement.example.invalid/v1" },
    })
    fireEvent.click(screen.getByRole("button", { name: "保存" }))

    // 只用 DOM 自身的事实断言（`textContent`），不引入 jest-dom matcher：
    // 仓库没有安装 `@testing-library/jest-dom`，用 `toHaveTextContent` 只会得到
    // "matcher 不存在"，而不是一条关于界面的断言。
    const alert = await screen.findByRole("alert")
    expect(alert.textContent).toContain("该配置 ID 已存在")
    const { profiles } = await getHavenClient().aiProviderProfileList()
    expect(profiles).toHaveLength(1)
    expect(profiles[0]?.endpoint).toBe(ENDPOINT)
  })

  it("Provider 列表读取失败时显示错误并隐藏配置写入动作", async () => {
    const rendered = renderAiSettings()
    await createProfile()
    const client = getHavenClient()
    vi.spyOn(client, "aiProviderProfileList").mockRejectedValueOnce({
      code: "AI_PROVIDER_UNAVAILABLE",
      userMessage: "读取 Provider 失败",
      retryable: true,
    })

    rendered.unmount()
    renderAiSettings()

    expect((await screen.findAllByText("读取 Provider 失败")).length).toBeGreaterThan(0)
    expect(screen.getByRole("button", { name: "重试读取" })).toBeTruthy()
    expect(screen.queryByRole("button", { name: "删除配置" })).toBeNull()
    expect((screen.getByRole("button", { name: "刷新模型列表" }) as HTMLButtonElement).disabled).toBe(true)
  })
})

/**
 * 放弃草稿必须让它的失败重放一起失效。
 *
 * 复现自审计探针：编辑名称 → 保存失败（可重试）→ 点「放弃」（界面恢复已保存值）→
 * 再点错误横幅上的「重试」。旧实现只 `setDraft(null)`，重试把刚被放弃的名称写了回去。
 */
describe("设置页 AI 分组：放弃草稿后失败重试不得重放已放弃的输入", () => {
  it("编辑放弃后，错误横幅的重试不会写入被放弃的名称", async () => {
    renderAiSettings()
    await createProfile()

    fireEvent.change(screen.getByLabelText("配置名称"), { target: { value: "被放弃的名称" } })
    const client = getHavenClient()
    vi.spyOn(client, "aiProviderProfileUpsert").mockRejectedValueOnce(new HavenError({
      code: "AI_PROVIDER_TRANSIENT",
      userMessage: "临时写入失败",
      retryable: true,
    }))
    // 「保存」按钮位于独立的「保存配置」行，不在「配置名称」行内。
    const saveRow = screen.getByText("保存配置").closest("[data-settings-row]") as HTMLElement
    fireEvent.click(within(saveRow).getByRole("button", { name: "保存" }))
    expect(await screen.findByText("临时写入失败")).toBeTruthy()

    fireEvent.click(screen.getByRole("button", { name: "放弃" }))
    await waitFor(() => expect((screen.getByLabelText("配置名称") as HTMLInputElement).value).toBe("自建网关"))

    // 失败提示随草稿一起作废：横幅里连「重试」都不该再出现。
    expect(screen.queryByText("临时写入失败")).toBeNull()
    expect(screen.queryByRole("button", { name: "重试" })).toBeNull()
    const { profiles } = await client.aiProviderProfileList()
    expect(profiles[0]?.displayName).toBe("自建网关")
  })

  it("新建放弃后，被放弃的配置 id 不会被任何重试写进本机", async () => {
    renderAiSettings()
    await createProfile()
    fireEvent.click(screen.getByRole("button", { name: "新建配置" }))
    fireEvent.change(screen.getByLabelText("配置 ID"), { target: { value: "abandoned-provider" } })
    const client = getHavenClient()
    vi.spyOn(client, "aiProviderProfileUpsert").mockRejectedValueOnce(new HavenError({
      code: "AI_PROVIDER_TRANSIENT",
      userMessage: "临时写入失败",
      retryable: true,
    }))
    fireEvent.click(screen.getByRole("button", { name: "保存" }))
    expect(await screen.findByText("临时写入失败")).toBeTruthy()

    fireEvent.click(screen.getByRole("button", { name: "放弃" }))
    await waitFor(() => expect(screen.queryByLabelText("配置 ID")).toBeNull())

    const { profiles } = await client.aiProviderProfileList()
    expect(profiles.map((profile) => profile.profileId)).toEqual([PROFILE_ID])
  })
})

/**
 * 已保存的默认模型不在当前目录里，与"从未选择"是两件事。
 *
 * 保存时保留的 `selectedModelId` 仍在生效（后端据此校验并报告模型不可用）。界面把它
 * 显示成「未选择」，等于告诉用户本机没有选择——而用户明明选过。
 */
describe("设置页 AI 分组：已选模型不在当前目录", () => {
  it("保留已保存的模型 id 并给出重新选择指引，不静默改写或删除它", async () => {
    const client = getHavenClient()
    // 预先写入一份**已选模型不在 fixture 目录里**的配置；凭据也要存在，否则目录是 no_credential。
    await client.aiProviderProfileUpsert({
      profileId: PROFILE_ID,
      displayName: "自建网关",
      kind: "openai_compatible",
      endpoint: ENDPOINT,
      enabled: true,
      selectedModelId: "retired-model",
      expectedRevision: null,
    })
    await client.credentialSet({ provider: "ai", profileId: PROFILE_ID, secret: API_KEY })

    renderAiSettings()
    const trigger = (await screen.findByRole("button", { name: "默认模型" })) as HTMLButtonElement
    await waitFor(() => expect(trigger.disabled).toBe(false))

    // 不说「未选择」，也不说「无可用模型」：说清"保存的模型不在目录里"。
    expect(trigger.textContent).not.toContain("未选择")
    expect(trigger.textContent).not.toContain("无可用模型")
    expect(trigger.textContent).toContain("已选模型")
    const modelRow = trigger.closest("[data-settings-row]") as HTMLElement
    expect(modelRow.textContent).toContain("retired-model")
    expect(modelRow.textContent).toContain("替换")

    // 真实模型仍然可选；选项里**只有**目录模型，"已选模型不可用"不是一项可点的假模型。
    fireEvent.click(trigger)
    await screen.findByRole("listbox")
    expect(screen.queryByRole("option", { name: "已选模型已不在当前目录" })).toBeNull()
    const options = screen.getAllByRole("option").map((option) => option.textContent ?? "")
    expect(options.some((option) => option.includes("已选模型"))).toBe(false)
    expect(options.some((option) => option.includes("retired-model"))).toBe(false)
    expect(options.some((option) => option.includes("GPT-4o mini (gpt-4o-mini)"))).toBe(true)

    // 选中真实模型后写入的是目录里的 id，而不是被自动替换。
    fireEvent.click(screen.getByRole("option", { name: "GPT-4o mini (gpt-4o-mini)" }))
    await waitFor(async () => {
      const listed = await client.aiProviderProfileList()
      expect(listed.profiles[0]?.selectedModelId).toBe("gpt-4o-mini")
    })
  })

  it("从未选择时仍然显示「未选择」，不被失效分支吞掉", async () => {
    renderAiSettings()
    await createProfile()
    fireEvent.click(await screen.findByRole("button", { name: "配置" }))
    fireEvent.change(await screen.findByLabelText("API Key"), { target: { value: API_KEY } })
    fireEvent.click(screen.getByRole("button", { name: "保存" }))
    expect(await screen.findByText("已配置")).toBeTruthy()

    const trigger = (await screen.findByRole("button", { name: "默认模型" })) as HTMLButtonElement
    await waitFor(() => expect(trigger.disabled).toBe(false))
    expect(trigger.textContent).toContain("未选择")
    expect(trigger.textContent).not.toContain("已选模型")
  })
})

/**
 * 未保存的草稿不能让「已保存的那一份」继续在背后干活。
 *
 * 面板里的名称 / 地址是草稿，而"读取模型目录 / 切换启用 / 选择默认模型"三条动作读写的是
 * 本机**已保存**的配置。草稿在屏幕上、请求打到旧地址，用户拿到的是另一份配置的结果却
 * 没有任何提示——这比"点了没反应"更糟。因此这些入口在草稿保存前停用，并把原因写在界面上。
 */
describe("设置页 AI 分组：草稿未保存时不得按已保存的地址动作", () => {
  /** 建一份配置并写入密钥，让模型目录非空（默认模型选择器才会是可交互的）。 */
  async function createProfileWithModels() {
    await createProfile()
    fireEvent.click(await screen.findByRole("button", { name: "配置" }))
    fireEvent.change(await screen.findByLabelText("API Key"), { target: { value: API_KEY } })
    fireEvent.click(screen.getByRole("button", { name: "保存" }))
    expect(await screen.findByText("已配置")).toBeTruthy()
    const trigger = (await screen.findByRole("button", { name: "默认模型" })) as HTMLButtonElement
    await waitFor(() => expect(trigger.disabled).toBe(false))
  }

  function enabledToggle(): HTMLButtonElement {
    return screen.getByRole("switch", { name: "启用智能功能" }) as HTMLButtonElement
  }

  /** 密钥编辑器所在的那一条设置行：里面「保存」与面板上的「保存配置」重名，必须落到行上取。 */
  function apiKeyRow(): HTMLElement {
    const row = (screen.getByLabelText("API Key") as HTMLElement).closest("[data-settings-row]")
    if (!row) throw new Error("未找到 API Key 所属的设置行")
    return row as HTMLElement
  }

  it("改地址后停用会打到旧地址的动作，放弃草稿后恢复", async () => {
    renderAiSettings()
    await createProfileWithModels()
    expect(enabledToggle().disabled).toBe(false)

    fireEvent.change(screen.getByLabelText("API 地址"), {
      target: { value: "https://edited.example.invalid/v1" },
    })

    expect((screen.getByRole("button", { name: "刷新模型列表" }) as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByRole("button", { name: "拉取模型" }) as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByRole("button", { name: "默认模型" }) as HTMLButtonElement).disabled).toBe(true)
    expect(enabledToggle().disabled).toBe(true)
    // 停用必须说明原因：只把按钮变灰等于给用户一条死路。
    expect(screen.getByTestId("ai-profile-draft-notice").textContent).toContain("请先保存配置")

    // 放弃草稿：不写任何东西，动作全部恢复，输入框回到已保存的地址。
    fireEvent.click(screen.getByRole("button", { name: "放弃" }))
    await waitFor(() =>
      expect((screen.getByRole("button", { name: "刷新模型列表" }) as HTMLButtonElement).disabled).toBe(false),
    )
    expect((screen.getByRole("button", { name: "拉取模型" }) as HTMLButtonElement).disabled).toBe(false)
    expect((screen.getByRole("button", { name: "默认模型" }) as HTMLButtonElement).disabled).toBe(false)
    expect(enabledToggle().disabled).toBe(false)
    expect(screen.queryByTestId("ai-profile-draft-notice")).toBeNull()
    expect((screen.getByLabelText("API 地址") as HTMLInputElement).value).toBe(ENDPOINT)
  })

  it("地址改脏后既不能提交新密钥，也不能清除已配置的密钥", async () => {
    renderAiSettings()
    await createProfileWithModels()

    // 没有草稿时密钥编辑器照常可达：密钥框里有字本身不构成阻塞。
    fireEvent.click(screen.getByRole("button", { name: "配置" }))
    const NEW_KEY = "sk-second-not-a-real-key"
    fireEvent.change(await screen.findByLabelText("API Key"), { target: { value: NEW_KEY } })
    expect((within(apiKeyRow()).getByRole("button", { name: "保存" }) as HTMLButtonElement).disabled).toBe(false)

    // 地址一改脏，凭据写入绑定的就不再是屏幕上这一份：保存与清除都必须停手。
    fireEvent.change(screen.getByLabelText("API 地址"), {
      target: { value: "https://edited.example.invalid/v1" },
    })
    expect((within(apiKeyRow()).getByRole("button", { name: "保存" }) as HTMLButtonElement).disabled).toBe(true)

    // 停用不只是"变灰"：这次点击没有写走任何东西，密钥原文还留在框里。
    fireEvent.click(within(apiKeyRow()).getByRole("button", { name: "保存" }))
    expect((screen.getByLabelText("API Key") as HTMLInputElement).value).toBe(NEW_KEY)

    // 编辑器关掉后「配置」入口同样停用；放弃草稿后一次性恢复单向保存。
    fireEvent.click(within(apiKeyRow()).getByRole("button", { name: "取消" }))
    expect((screen.getByRole("button", { name: "配置" }) as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByRole("button", { name: "清除" }) as HTMLButtonElement).disabled).toBe(true)

    fireEvent.click(screen.getByRole("button", { name: "放弃" }))
    await waitFor(() =>
      expect((screen.getByRole("button", { name: "配置" }) as HTMLButtonElement).disabled).toBe(false),
    )
    expect((screen.getByLabelText("API 地址") as HTMLInputElement).value).toBe(ENDPOINT)
  })

  it("模型目录动作不写成「测试连接」", async () => {
    renderAiSettings()
    await createProfileWithModels()

    // 这个按钮只读一次模型目录，一次推理请求都不发：名字里不能出现"测试连接"这种探测承诺。
    expect(screen.queryByRole("button", { name: /测试连接/ })).toBeNull()
    expect(screen.getByRole("button", { name: "刷新模型列表" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "拉取模型" })).toBeTruthy()
  })
})

/**
 * 关闭「AI 设置」面板不得吞掉草稿：三条关闭入口（关闭按钮 / Escape / 点击遮罩）走同一条
 * 守卫，有草稿先确认（只有"继续编辑"与"显式放弃"两个出口），写入在途时连确认都不弹。
 *
 * 确认框是模态内部的一枚面板：通知中心渲染在 `document.body` 的普通层里，原生模态下点不到。
 */
describe("设置页 AI 面板：关闭未保存的草稿要确认", () => {
  const CONFIRM_TITLE = "有未保存的 AI 配置"

  function aiDialog(): HTMLElement {
    return screen.getByRole("dialog", { name: "AI 设置" })
  }

  /** 打开面板、建一份配置，再把名称改脏。 */
  async function openDirtyEditDraft() {
    renderAiSettings()
    await createProfile()
    fireEvent.change(screen.getByLabelText("配置名称"), { target: { value: "改过的名字" } })
    expect(screen.getByTestId("ai-profile-draft-notice")).toBeTruthy()
  }

  it("没有草稿时关闭不需要确认", async () => {
    renderAiSettings()
    await createProfile()

    fireEvent.click(screen.getByRole("button", { name: "关闭 AI 设置" }))

    await waitFor(() => expect(screen.queryByRole("dialog", { name: "AI 设置" })).toBeNull())
    expect(screen.queryByText(CONFIRM_TITLE)).toBeNull()
  })

  it("关闭按钮先确认；继续编辑保留草稿与面板", async () => {
    await openDirtyEditDraft()

    fireEvent.click(screen.getByRole("button", { name: "关闭 AI 设置" }))
    expect(screen.getByText(CONFIRM_TITLE)).toBeTruthy()

    fireEvent.click(screen.getByRole("button", { name: "继续编辑" }))

    expect(aiDialog()).toBeTruthy()
    expect(screen.queryByText(CONFIRM_TITLE)).toBeNull()
    expect((screen.getByLabelText("配置名称") as HTMLInputElement).value).toBe("改过的名字")
  })

  it("Escape 与点击遮罩走同一条确认，取消后草稿仍在", async () => {
    await openDirtyEditDraft()

    // 原生 <dialog> 的 Escape 由浏览器派发 cancel 事件，`SettingsDialog` 在 onCancel 里收口。
    fireEvent(aiDialog(), new Event("cancel", { cancelable: true }))
    expect(screen.getByText(CONFIRM_TITLE)).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: "继续编辑" }))

    // 遮罩点击：只有 target 就是 <dialog> 本身时才算是点到了遮罩。
    fireEvent.mouseDown(aiDialog())
    expect(screen.getByText(CONFIRM_TITLE)).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: "继续编辑" }))

    expect(aiDialog()).toBeTruthy()
    expect((screen.getByLabelText("配置名称") as HTMLInputElement).value).toBe("改过的名字")
  })

  it("显式放弃才关闭：草稿没有写进任何地方，重新打开回到已保存状态", async () => {
    await openDirtyEditDraft()

    fireEvent.click(screen.getByRole("button", { name: "关闭 AI 设置" }))
    fireEvent.click(screen.getByRole("button", { name: "放弃修改" }))

    await waitFor(() => expect(screen.queryByRole("dialog", { name: "AI 设置" })).toBeNull())
    const { profiles } = await getHavenClient().aiProviderProfileList()
    expect(profiles).toHaveLength(1)
    expect(profiles[0]?.displayName).toBe("自建网关")

    fireEvent.click(screen.getByRole("button", { name: "AI 设置" }))
    const name = (await screen.findByLabelText("配置名称")) as HTMLInputElement
    expect(name.value).toBe("自建网关")
  })

  it("未提交的 API Key 输入同样算未保存：只在显式放弃后随面板一起消失", async () => {
    renderAiSettings()
    await createProfile()

    fireEvent.click(await screen.findByRole("button", { name: "配置" }))
    fireEvent.change(await screen.findByLabelText("API Key"), { target: { value: API_KEY } })

    fireEvent.click(screen.getByRole("button", { name: "关闭 AI 设置" }))
    expect(screen.getByText(CONFIRM_TITLE)).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: "继续编辑" }))
    expect((screen.getByLabelText("API Key") as HTMLInputElement).value).toBe(API_KEY)

    fireEvent.click(screen.getByRole("button", { name: "关闭 AI 设置" }))
    fireEvent.click(screen.getByRole("button", { name: "放弃修改" }))

    await waitFor(() => expect(screen.queryByRole("dialog", { name: "AI 设置" })).toBeNull())
    expect(document.body.textContent).not.toContain(API_KEY)
    // 只粘过、没提交的密钥不会被任何一次关闭顺手写下去。
    const { profiles } = await getHavenClient().aiProviderProfileList()
    expect(profiles[0]?.credentialConfigured).toBe(false)
  })

  it("写入在途时不关闭，也不弹确认", async () => {
    await openDirtyEditDraft()

    // 一次已经发出去、结果还没回来的写入。
    vi.spyOn(getHavenClient(), "aiProviderProfileUpsert").mockReturnValue(new Promise(() => {}))
    fireEvent.click(screen.getByRole("button", { name: "保存" }))

    await waitFor(() =>
      expect((screen.getByRole("button", { name: "关闭 AI 设置" }) as HTMLButtonElement).disabled).toBe(true),
    )
    const dialog = aiDialog()
    fireEvent(dialog, new Event("cancel", { cancelable: true }))
    fireEvent.mouseDown(dialog)

    expect(aiDialog()).toBeTruthy()
    expect(screen.queryByText(CONFIRM_TITLE)).toBeNull()
  })

  it("写入在途时冻结草稿输入：那一次保存不会把期间的新编辑悄悄吃掉", async () => {
    renderAiSettings()
    await createProfile()

    fireEvent.change(screen.getByLabelText("配置名称"), { target: { value: "改过的名字" } })
    // 一次已经发出去、结果还没回来的写入。
    vi.spyOn(getHavenClient(), "aiProviderProfileUpsert").mockReturnValue(new Promise(() => {}))
    fireEvent.click(screen.getByRole("button", { name: "保存" }))

    // 保存成功后草稿会被收起，所以这期间敲进去的字符只会随草稿一起消失：输入必须冻住。
    await waitFor(() =>
      expect((screen.getByLabelText("配置名称") as HTMLInputElement).disabled).toBe(true),
    )
    expect((screen.getByLabelText("API 地址") as HTMLInputElement).disabled).toBe(true)
    expect((screen.getByRole("button", { name: "保存" }) as HTMLButtonElement).disabled).toBe(true)
  })

  /**
   * 面板关闭会销毁持有草稿的 `AiSettings`，控制器却活在分区之上。
   *
   * 不做这条生命周期边界时："保存失败 → 关闭 → 确认放弃修改 → 重新打开 → 点重试"仍会
   * 把随面板一起消失的那份草稿写回本机。取消关闭（继续编辑）不是放弃，重放必须保留。
   */
  it("保存失败后关闭并确认放弃：重新打开面板时没有任何可重放的失败草稿", async () => {
    await openDirtyEditDraft()

    vi.spyOn(getHavenClient(), "aiProviderProfileUpsert").mockRejectedValueOnce(new HavenError({
      code: "AI_PROVIDER_TRANSIENT",
      userMessage: "临时写入失败",
      retryable: true,
    }))
    fireEvent.click(screen.getByRole("button", { name: "保存" }))
    expect(await screen.findByText("临时写入失败")).toBeTruthy()

    // 先取消一次关闭：这不是放弃，失败提示与重试都该原样留着。
    fireEvent.click(screen.getByRole("button", { name: "关闭 AI 设置" }))
    fireEvent.click(screen.getByRole("button", { name: "继续编辑" }))
    expect(screen.getByText("临时写入失败")).toBeTruthy()
    expect(screen.getByRole("button", { name: "重试" })).toBeTruthy()

    // 再次关闭并确认放弃：草稿随面板一起消失，它的失败重放也必须作废。
    fireEvent.click(screen.getByRole("button", { name: "关闭 AI 设置" }))
    fireEvent.click(screen.getByRole("button", { name: "放弃修改" }))
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "AI 设置" })).toBeNull())

    fireEvent.click(screen.getByRole("button", { name: "AI 设置" }))
    expect(await screen.findByLabelText("配置名称")).toBeTruthy()
    expect(screen.queryByText("临时写入失败")).toBeNull()
    expect(screen.queryByRole("button", { name: "重试" })).toBeNull()

    const { profiles } = await getHavenClient().aiProviderProfileList()
    expect(profiles).toHaveLength(1)
    expect(profiles[0]?.displayName).toBe("自建网关")
  })
})

/**
 * 浏览器历史不受关闭守卫约束。
 *
 * 普通「关闭」在写入在途时会被拦住（那一次写入还没有结论）；后退 / 前进却会直接把面板
 * 连同草稿卸载掉，控制器仍活在页面之上。若卸载边界只走"在途就拒绝"的放弃入口，迟到的
 * 失败会带着 `lastSaveRequest` 回到 Hook 里——重新打开面板就能点「重试」，把随面板一起
 * 消失的那份输入写回本机。
 */
describe("设置页 AI 面板：历史离开后不复活已销毁的草稿", () => {
  /** 从一个历史条目进来，这样"后退离开 → 前进返回"才是真的浏览器历史。 */
  function renderWithHistory(): ReturnType<typeof useNavigate> {
    let navigate!: ReturnType<typeof useNavigate>
    function Driver() {
      const routerNavigate = useNavigate()
      // 在 effect 里取值：渲染期给外部变量赋值是副作用（react(globals) 会告警）。
      useEffect(() => {
        navigate = routerNavigate
      }, [routerNavigate])
      return <SettingsPage />
    }
    render(
      <MemoryRouter initialEntries={["/settings/privacy", "/settings/ai"]} initialIndex={1}>
        <Routes>
          <Route path="/settings/:section" element={<Driver />} />
        </Routes>
      </MemoryRouter>,
    )
    return navigate
  }

  it("写入在途时后退离开：迟到的失败不提供重试，也不写回那份草稿", async () => {
    const navigate = renderWithHistory()
    fireEvent.click(screen.getByRole("button", { name: "AI 设置" }))
    await createProfile()
    fireEvent.change(await screen.findByLabelText("配置名称"), { target: { value: "随面板消失的名字" } })

    const client = getHavenClient()
    let rejectWrite!: (error: unknown) => void
    const pending = new Promise<never>((_resolve, reject) => { rejectWrite = reject })
    const upsert = vi.spyOn(client, "aiProviderProfileUpsert").mockReturnValueOnce(pending)
    fireEvent.click(screen.getByRole("button", { name: "保存" }))
    await waitFor(() => expect((screen.getByLabelText("配置名称") as HTMLInputElement).disabled).toBe(true))

    // 后退不是「放弃修改」：关闭守卫拦不住它，面板与草稿一起消失。
    await act(async () => { await navigate(-1) })
    await waitFor(() => expect(screen.queryByLabelText("配置名称")).toBeNull())

    // 写入这时才给出结论：失败如实落在状态里，但那份输入已经不在了。
    await act(async () => {
      rejectWrite(new HavenError({
        code: "AI_PROVIDER_TRANSIENT",
        userMessage: "迟到的写入失败",
        retryable: true,
      }))
      await pending.catch(() => undefined)
    })

    // 前进回到 AI 分区，重新打开面板。
    await act(async () => { await navigate(1) })
    const open = screen.queryByRole("button", { name: "AI 设置" })
    if (open) fireEvent.click(open)
    expect(await screen.findByLabelText("配置名称")).toBeTruthy()
    // 迟到的失败本身如实呈现（用户确实提交过这次写入）……
    expect(await screen.findByText("迟到的写入失败")).toBeTruthy()

    // ……但没有可重放的输入，就没有「重试」：不给出一个按下去什么也不做的按钮。
    expect(screen.queryByRole("button", { name: "重试" })).toBeNull()
    // 只有最初那次被拒绝的写入，迟到的失败没有重放被放弃的名称。
    expect(upsert).toHaveBeenCalledTimes(1)
    const { profiles } = await client.aiProviderProfileList()
    expect(profiles).toHaveLength(1)
    expect(profiles[0]?.displayName).toBe("自建网关")
  })
})

/** 定位「外部 Agent 接入」分组（SettingsGroup 渲染成 section）。 */
async function findAgentBrokerGroup(): Promise<HTMLElement> {  const heading = await screen.findByText("外部 Agent 接入")
  const section = heading.closest("section")
  if (!section) throw new Error("未找到「外部 Agent 接入」分组")
  return section
}

/**
 * 定位一段文案所属的那一条 `SettingRow`（渲染成 `div.group/row`）。
 *
 * 「外部 Agent 接入」分组里现在住着**两个**功能：Broker 与「一键配置外部 MCP 客户端」。
 * 断言"这一行没有写 X"时必须先落到行上；按整组做否定断言会把另一个功能的正常文案
 * 判成失败，那样修掉的不是界面错误，而是把闸门拆了。
 */
function settingRowOf(element: HTMLElement): HTMLElement {
  const row = element.closest('[class~="group/row"]')
  if (!row) throw new Error("未找到该文案所属的设置行")
  return row as HTMLElement
}

/**
 * 外部 Agent 接入分组（A5 接线切片）。
 *
 * 浏览器里没有 Rust Broker：这里只覆盖「默认关闭 → 显式开启 → 只得到预览标记」这一条
 * 与 Mock 客户端一致的状态机。真实端点的形态、模板转义与 busy / unavailable 分支由
 * `ipc/agent-broker-gateway.test.ts` 与 Rust 侧测试覆盖——页面测试拿不到真实端点。
 */
describe("设置页外部 Agent 接入分组", () => {
  // Broker 状态同样留在 Mock 单例里；每个用例都从「显式关闭」开始。
  beforeEach(async () => {
    await getHavenClient().agentBrokerDisable()
  })

  it("默认关闭：不预置监听状态，也没有端点或模板", async () => {
    renderAiSettings()
    const group = await findAgentBrokerGroup()

    // 事实来自客户端：读到 disabled 才显示「已关闭」，不是页面自己预设的。
    expect(await within(group).findByText("已关闭")).toBeTruthy()
    expect(within(group).getByRole("button", { name: "启用外部接入" })).toBeTruthy()
    expect(within(group).queryByText("监听中")).toBeNull()
    // 关闭态既没有端点行，也没有预览标记。这条只能钉在 Broker 自己的文案上：
    // 同一个分组里的「一键配置外部 MCP 客户端」在浏览器预览下本来就会写出「浏览器预览」
    // 这几个字，用 /浏览器预览/ 通配整个分组会把另一个功能的正常说明判成失败。
    expect(within(group).queryByText("本地端点")).toBeNull()
    expect(within(group).queryByText("浏览器预览：没有真实本地监听")).toBeNull()
    expect(within(group).queryByRole("button", { name: /复制/ })).toBeNull()
    expect(within(group).queryByLabelText("配置模板")).toBeNull()
    expect(document.body.textContent).not.toContain("mock://")
  })

  it("启用后只给出浏览器预览标记：没有可复制端点，也没有连接模板", async () => {
    renderAiSettings()
    const group = await findAgentBrokerGroup()
    await within(group).findByText("已关闭")

    fireEvent.click(within(group).getByRole("button", { name: "启用外部接入" }))

    expect(await within(group).findByText("监听中")).toBeTruthy()
    const previewMarker = within(group).getByText("浏览器预览：没有真实本地监听")
    expect(previewMarker).toBeTruthy()
    // 预览态不得出现复制入口、模板选择或原始 mock 标识。
    expect(within(group).queryByRole("button", { name: /复制/ })).toBeNull()
    expect(within(group).queryByLabelText("配置模板")).toBeNull()
    expect(document.body.textContent).not.toContain("mock://")
    // Broker 的预览行自己不得提任何客户端：预览态说「Codex 可以连接」才是真正的误导。
    // 这条只能钉在**那一行**上——同一个分组里的「一键配置外部 MCP 客户端」会如实写出它
    // 支持的两个客户端与两个固定文件，那是另一个功能的既定文案，不是这里的谎报。
    const previewRow = settingRowOf(previewMarker)
    expect(previewRow.textContent).not.toContain("Codex")
    expect(previewRow.textContent).not.toContain("Claude Code")
    // 反过来说清楚两个功能是分开的：一键配置行独立存在，并写明它的固定授权范围。
    expect(within(group).getByText("一键配置外部 MCP 客户端")).toBeTruthy()
    for (const fixedScope of ["config.toml", ".claude.json"]) {
      expect(group.textContent).toContain(fixedScope)
    }
    // 监听中才有停用入口。
    expect(within(group).getByRole("button", { name: "停用" })).toBeTruthy()
  })

  it("分组只描述权限边界：没有审批按钮，也不出现任何模型名", async () => {
    renderAiSettings()
    const group = await findAgentBrokerGroup()
    await within(group).findByText("已关闭")

    // 必须写明批准 / 拒绝 / 应用只能回到栖阅界面，且没有 SQL / 文件系统 / Secret 权限。
    for (const required of ["批准", "拒绝", "应用", "SQL", "文件系统", "Secret", "待审批建议", "需你在栖阅中操作"]) {
      expect(group.textContent).toContain(required)
    }
    for (const forbidden of [/批准/, /拒绝/, /应用/, /approve/i, /apply/i, /reject/i]) {
      expect(within(group).queryByRole("button", { name: forbidden })).toBeNull()
    }
    // 外部接入分组与模型无关：这里不该出现任何示例模型名。
    for (const modelName of ["gpt-4", "gpt-4o", "claude-3", "sonnet", "gemini", "text-embedding"]) {
      expect(group.textContent?.toLowerCase()).not.toContain(modelName)
    }
  })
})

/** 与 `renderAiSettings` 相同，但把通知中心换成一个可观测的 push；`confirmResult` 决定确认框结果。 */
function renderAiSettingsWithNotices(pushed: string[], confirmResult = false) {
  const rendered = render(
    <NoticeContext.Provider value={{
      notices: [],
      push: (input: NoticeInput) => {
        pushed.push(input.message)
        return ""
      },
      dismiss: () => undefined,
      clear: () => undefined,
      resolveConfirm: () => undefined,
      confirm: async () => confirmResult,
    }}>
      <MemoryRouter initialEntries={["/settings/ai"]}>
        <Routes>
          <Route path="/settings/:section" element={<SettingsPage />} />
        </Routes>
      </MemoryRouter>
    </NoticeContext.Provider>,
  )
  fireEvent.click(screen.getByRole("button", { name: "AI 设置" }))
  return rendered
}

/**
 * Mock 只把凭据记在浏览器内存里。
 *
 * 页面上的其它成功提示都区分了 Mock（保存配置、写入 API Key），但「清除 API Key」与
 * 「删除配置」曾经照抄生产文案："已清除 API Key" / "已删除配置并清除其 API Key" 会让
 * 用户以为系统凭据管理器里真的动过——那是一句关于**本机持久化**的假陈述，而界面之外
 * 没有任何东西可以纠正它。这两条用例把文案钉在"Mock/预览 + 没有触碰系统凭据管理器"上。
 */
describe("设置页 AI 分组的 Mock 诚实性", () => {
  it("Mock 下清除 API Key 不得声称改动了系统凭据管理器", async () => {
    const pushed: string[] = []
    renderAiSettingsWithNotices(pushed)
    await createProfile()

    fireEvent.click(await screen.findByRole("button", { name: "配置" }))
    fireEvent.change(await screen.findByLabelText("API Key"), { target: { value: API_KEY } })
    fireEvent.click(screen.getByRole("button", { name: "保存" }))
    expect(await screen.findByText("已配置")).toBeTruthy()

    fireEvent.click(screen.getByRole("button", { name: "清除" }))

    await waitFor(() => expect(pushed.some((message) => message.includes("没有改动系统凭据管理器"))).toBe(true))
    expect(pushed).not.toContain("已清除 API Key")
    // 写入与清除必须说同一件事：这里没有系统凭据管理器参与。
    for (const message of pushed) {
      expect(message).not.toContain("已清除 API Key")
    }
  })

  it("Mock 下删除配置不得声称删除了本机数据库记录", async () => {
    const pushed: string[] = []
    renderAiSettingsWithNotices(pushed, true)
    await createProfile()

    fireEvent.click(await screen.findByRole("button", { name: "删除配置" }))

    await waitFor(() => expect(pushed.some((message) => message.includes("没有删除本机数据库记录"))).toBe(true))
    expect(pushed).not.toContain("已删除配置并清除其 API Key")
    const deleteNotice = pushed.find((message) => message.includes("没有删除本机数据库记录"))
    expect(deleteNotice).toContain("Mock/预览")
    expect(deleteNotice).toContain("没有触碰系统凭据管理器")
  })

  /**
   * 写入那一步同样是**关于本机持久化**的陈述。
   *
   * 它曾经只有生产口径："API Key 已写入系统凭据管理器"。在浏览器预览里这句话是假的：
   * 密钥只进了内存，刷新即失效，系统凭据管理器里什么都没有。清除与删除两条已经改了口径，
   * 写入这条同样不能漏——它是用户最先看到、也最容易相信的那一句。
   */
  it("Mock 下写入 API Key 不得声称写进了系统凭据管理器", async () => {
    const pushed: string[] = []
    renderAiSettingsWithNotices(pushed)
    await createProfile()

    fireEvent.click(await screen.findByRole("button", { name: "配置" }))
    fireEvent.change(await screen.findByLabelText("API Key"), { target: { value: API_KEY } })
    fireEvent.click(screen.getByRole("button", { name: "保存" }))

    await waitFor(() =>
      expect(pushed.some((message) => message.includes("没有写入系统凭据管理器"))).toBe(true),
    )
    expect(pushed).not.toContain("API Key 已写入系统凭据管理器")
  })

  /**
   * 模型列表的来源同样要说清楚。
   *
   * 预览里的目录是契约 fixture，**不是**对某个真实 Provider 发过一次请求拿到的结果。
   * 照抄生产口径（"模型与能力都来自 Provider 的模型列表"）会让用户以为本机真的探测到了模型。
   */
  it("Mock 下模型列表必须标注来自契约 fixture，而不是真实 Provider 的响应", async () => {
    renderAiSettings()
    await createProfile()
    fireEvent.click(await screen.findByRole("button", { name: "配置" }))
    fireEvent.change(await screen.findByLabelText("API Key"), { target: { value: API_KEY } })
    fireEvent.click(screen.getByRole("button", { name: "保存" }))
    expect(await screen.findByText("已配置")).toBeTruthy()

    // 这一行只在目录非空时渲染，因此先确认目录确实非空（否则下面的断言会"空过"）。
    // fixture 里的模型**显示名**只出现在「默认模型」下拉展开后的选项里（触发器折叠时
    // 显示的是「未选择」，而「识图模型」那一行写的是 modelId「gpt-4o-mini」）。
    // 因此必须像上一条 API Key 用例那样先把它打开，再断言模型确实在。
    const trigger = (await screen.findByRole("button", { name: "默认模型" })) as HTMLButtonElement
    await waitFor(() => expect(trigger.disabled).toBe(false))
    expect(document.body.textContent).toContain("不是真实 Provider 的 /models 响应")
    expect(document.body.textContent).not.toContain("模型与能力都来自 Provider 的模型列表。")

    fireEvent.click(trigger)
    expect(await screen.findByRole("listbox")).toBeTruthy()
    expect(document.body.textContent).toContain("GPT-4o mini")
  })
})

/**
 * 「一键配置外部 MCP 客户端」现在是 `implemented`
 * （`settings-registry.ts` 的 `ai.clientAutoConfig` / `ai.client-auto-config`，
 * 绑定 `mcpClientConfigStatus` / `mcpClientConfigApply`）。
 *
 * 因此这道闸换成了三条**必须同时成立**的断言——它挡的是这一类错误：控件接上了，
 * 但用户看不到自己会被写到哪里，或者浏览器预览里冒出一个"已配置"的假成功：
 *
 * 1. **授权范围是可见事实**：两个固定文件与两个键名都写在界面上，不藏在文档里；
 * 2. **没有"自定义路径"入口**：目标只能是闭合枚举里的两个客户端；
 * 3. **Mock 里不谎报**：没有随包分发的运行时，因此既没有可写的按钮，也不显示"已配置"。
 */
describe("设置页如实呈现「一键配置外部客户端」", () => {
  it("写出两个授权位置的键名，不提供自定义路径，也不在预览里谎报已配置", async () => {
    renderAiSettings()
    await screen.findByText("外部 Agent 接入")

    const body = document.body.textContent ?? ""
    // 授权范围必须逐字可见：用户要能自己核对"它到底会动我哪个文件"。
    for (const visible of ["config.toml", ".claude.json", "mcp_servers.haven", "mcpServers.haven"]) {
      expect(body, `界面必须写出授权位置 ${visible}`).toContain(visible)
    }
    // 没有"自定义路径 / 任意客户端 / 上传配置片段"这类入口。
    for (const name of [/自定义路径/, /选择文件/, /浏览/, /导入配置/]) {
      expect(screen.queryByRole("button", { name })).toBeNull()
    }
    // 浏览器预览没有随包运行时：不得显示「已配置」，也不得给出写入按钮。
    expect(screen.queryByText("已配置")).toBeNull()
    expect(screen.queryByRole("button", { name: /写入配置/ })).toBeNull()
    expect(screen.queryByRole("button", { name: /重新写入/ })).toBeNull()
  })
})

/**
 * 分区切换不得重置用户选中的 Provider。
 *
 * 设置页的每个分区各自挂载自己的设置组件：切到别的分区再切回来，`AiSettings` 会连同
 * `useAiProviderSettings` 一起重新挂载。选择只留在那个组件里时，重挂载会静默回落到列表
 * 第一项——用户选的是第二个配置，回来变成第一个。而「打开栖伴」取的正是这个值
 * （`onOpenAssistant(selected?.profileId ?? null)`），于是外部模型请求被发给了另一个
 * Provider，界面上没有任何提示。
 *
 * 断言落在**可见事实**上（「当前服务」这一项是否还是用户选的那个）：它是重挂载后唯一
 * 能看出改投了的地方，而栖伴拿到的 profileId 与它同源。
 */
describe("设置页分区切换不重置选中的 Provider", () => {
  const SECOND_ID = "gw-second"
  const SECOND_ENDPOINT = "https://second.example.invalid/v1"

  it("切到别的分区再回来，仍然停在用户选中的那个配置上", async () => {
    renderAiSettings()
    // 先建第二个，再建 PROFILE_ID：列表按 profileId 排序，因此 PROFILE_ID 是第一项，
    // 用户显式选中的是第二项——正是"回落"会选错的那一个。
    await createProfileWith(SECOND_ID, SECOND_ENDPOINT)
    await createProfile()

    fireEvent.click(await screen.findByLabelText("当前 AI 服务"))
    const menu = screen.getByRole("listbox", { name: "当前 AI 服务" })
    expect(menu.closest("dialog")).toBe(screen.getByRole("dialog", { name: "AI 设置" }))
    expect(within(menu).getByRole("option", { selected: true }).className).toContain("text-[var(--haven-settings-primary-foreground)]")
    fireEvent.click(await screen.findByRole("option", { name: `自建网关 (${SECOND_ID})` }))
    expect(screen.getByLabelText("当前 AI 服务").textContent).toContain(SECOND_ID)

    // 切走：AI 分区卸载，Hook 与它内部的选择一起消失。
    fireEvent.click(screen.getByRole("button", { name: /^阅读/ }))
    await waitFor(() => expect(screen.queryByLabelText("当前 AI 服务")).toBeNull())

    // 切回来：AI 工作台是常驻导航入口，设置面板仍由工作台齿轮按钮打开。
    fireEvent.click(screen.getByRole("button", { name: /^AI 工作台/ }))
    fireEvent.click(screen.getByRole("button", { name: "AI 设置" }))
    const select = await screen.findByLabelText("当前 AI 服务")
    await waitFor(() => expect(select.textContent).toContain(SECOND_ID))
    expect(select.textContent, "重挂载后不得回落到列表第一项").not.toContain(PROFILE_ID)
  })
})

describe("工作台直接读取持久化 Provider", () => {
  it("配置读取完成前禁用输入和快捷操作", async () => {
    vi.spyOn(getHavenClient(), "aiProviderProfileList").mockReturnValue(new Promise(() => {}))
    renderAiWorkbench()
    const input = await screen.findByLabelText("描述你想要的设置改动")
    expect((input as HTMLTextAreaElement).disabled).toBe(true)
    expect((screen.getByRole("button", { name: "优化阅读" }) as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByRole("button", { name: "发送" }) as HTMLButtonElement).disabled).toBe(true)
  })

  it("不打开设置面板，首次进入和重新挂载都提交已保存的 profileId", async () => {
    const client = getHavenClient()
    await client.aiProviderProfileUpsert({
      profileId: PROFILE_ID,
      displayName: "自建网关",
      kind: "openai_compatible",
      endpoint: ENDPOINT,
      enabled: true,
      selectedModelId: "saved-model",
      expectedRevision: null,
    })
    const generate = vi.spyOn(client, "aiSettingsRecommendationGenerate")
      .mockRejectedValue(new Error("synthetic provider failure"))
    // 保留真实客户端方法的 this，同时走 Dialog 的生产请求分支。
    const facade = new Proxy(client, {
      getPrototypeOf: () => Object.prototype,
      get: (target, key) => {
        const value = Reflect.get(target, key, target)
        return typeof value === "function" ? value.bind(target) : value
      },
    })
    vi.spyOn(runtime, "getHavenClient").mockReturnValue(facade)

    for (let attempt = 1; attempt <= 2; attempt += 1) {
      const rendered = renderAiWorkbench()
      const input = await screen.findByLabelText("描述你想要的设置改动")
      await waitFor(() => expect((input as HTMLTextAreaElement).disabled).toBe(false))
      expect(screen.queryByRole("dialog")).toBeNull()
      fireEvent.change(input, { target: { value: "优化阅读" } })
      fireEvent.click(screen.getByRole("button", { name: "发送" }))
      await waitFor(() => expect(generate).toHaveBeenCalledTimes(attempt), { timeout: 6000 })
      expect(generate.mock.calls[attempt - 1][0].profileId).toBe(PROFILE_ID)
      rendered.unmount()
    }
  })

  it("非 AI 分区不自动读取 Provider", async () => {
    const client = getHavenClient()
    const list = vi.spyOn(client, "aiProviderProfileList")
    render(
      <MemoryRouter initialEntries={["/settings/reading"]}>
        <Routes><Route path="/settings/:section" element={<SettingsPage />} /></Routes>
      </MemoryRouter>,
    )
    await screen.findByRole("button", { name: /^AI 工作台/ })
    expect(list).not.toHaveBeenCalled()
  })
})
