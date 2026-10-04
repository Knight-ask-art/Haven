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
import { afterEach, beforeEach, describe, expect, it } from "vitest"

import { SettingsPage } from "./SettingsPage"
import { getHavenClient } from "@/lib/ipc/runtime"

const PROFILE_ID = "gw-main"
const ENDPOINT = "https://gateway.example.invalid/v1"
const API_KEY = "sk-ui-not-a-real-key"

afterEach(() => {
  cleanup()
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

function renderAiSettings() {
  return render(
    <MemoryRouter initialEntries={["/settings/ai"]}>
      <Routes>
        <Route path="/settings/:section" element={<SettingsPage />} />
      </Routes>
    </MemoryRouter>,
  )
}

/** 通过界面新建一个 Provider 配置（不触碰任何密钥）。 */
async function createProfile() {
  fireEvent.click(await screen.findByRole("button", { name: "新建配置" }))
  fireEvent.change(screen.getByLabelText("配置 ID"), { target: { value: PROFILE_ID } })
  fireEvent.change(screen.getByLabelText("API 地址"), { target: { value: ENDPOINT } })
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
})

/** 定位「外部 Agent 接入」分组（SettingsGroup 渲染成 section）。 */
async function findAgentBrokerGroup(): Promise<HTMLElement> {
  const heading = await screen.findByText("外部 Agent 接入")
  const section = heading.closest("section")
  if (!section) throw new Error("未找到「外部 Agent 接入」分组")
  return section
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
    expect(within(group).queryByText(/浏览器预览/)).toBeNull()
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
    expect(within(group).getByText("浏览器预览：没有真实本地监听")).toBeTruthy()
    // 预览态不得出现复制入口、模板选择或原始 mock 标识。
    expect(within(group).queryByRole("button", { name: /复制/ })).toBeNull()
    expect(within(group).queryByLabelText("配置模板")).toBeNull()
    expect(document.body.textContent).not.toContain("mock://")
    // 也不提任何客户端：预览态说「Codex 可以连接」才是真正的误导。
    expect(group.textContent).not.toContain("Codex")
    expect(group.textContent).not.toContain("Claude Code")
    // 监听中才有停用入口。
    expect(within(group).getByRole("button", { name: "停用" })).toBeTruthy()
  })

  it("分组只描述权限边界：没有审批按钮，也不出现任何模型名", async () => {
    renderAiSettings()
    const group = await findAgentBrokerGroup()
    await within(group).findByText("已关闭")

    // 必须写明批准 / 拒绝 / 应用只能回到栖阅界面，且没有 SQL / 文件系统 / Secret 权限。
    for (const required of ["批准", "拒绝", "应用", "SQL", "文件系统", "Secret", "待审批 Proposal"]) {
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
