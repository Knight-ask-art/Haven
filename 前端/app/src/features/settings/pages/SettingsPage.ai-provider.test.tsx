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

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { MemoryRouter, Route, Routes } from "react-router"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { SettingsPage } from "./SettingsPage"
import { getHavenClient } from "@/lib/ipc/runtime"
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
    renderAiSettings()
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
    renderAiSettings()
    await createProfile()
    const client = getHavenClient()
    vi.spyOn(client, "aiProviderProfileList").mockRejectedValueOnce({
      code: "AI_PROVIDER_UNAVAILABLE",
      userMessage: "读取 Provider 失败",
      retryable: true,
    })

    fireEvent.click(screen.getByRole("button", { name: "关闭 AI 设置" }))
    fireEvent.click(screen.getByRole("button", { name: "AI 设置" }))

    expect(await screen.findByText("读取 Provider 失败")).toBeTruthy()
    expect(screen.getByRole("button", { name: "重试读取" })).toBeTruthy()
    expect(screen.queryByRole("button", { name: "删除配置" })).toBeNull()
    expect((screen.getByRole("button", { name: "测试连接" }) as HTMLButtonElement).disabled).toBe(true)
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

/** 与 `renderAiSettings` 相同，但把通知中心换成一个可观测的 push。 */
function renderAiSettingsWithNotices(pushed: string[]) {
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
      confirm: async () => false,
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
    renderAiSettingsWithNotices(pushed)
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
