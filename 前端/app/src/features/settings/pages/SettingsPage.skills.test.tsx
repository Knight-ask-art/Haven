// @vitest-environment jsdom
//
// 设置页「内置技能」分组（浏览器 Mock 预览的 UI / 状态接线）。
//
// 断言的安全边界：
// - 状态来自客户端：读到之前不预置「已启用」，也不预置任何真实技能 id；
// - 浏览器预览用的是示例条目（真实目录是编译进二进制的构建产物，浏览器里没有它），
//   因此这里能证明的是**接线与状态机**，不是「技能真的生效了」；
// - 分组只描述权限边界：没有批准 / 应用 / 导入 / 编辑入口，也不出现任何技能正文或路径。

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { MemoryRouter, Route, Routes } from "react-router"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { SettingsPage } from "./SettingsPage"
import { getHavenClient } from "@/lib/ipc/runtime"

/** 与 MockHavenClient 同值的浏览器预览条目。 */
const MOCK_SKILL_ID = "preview-builtin-skill"
/** 真实内置技能的 id：预览环境里绝不能出现，否则会被读成「技能已在栖阅里生效」。 */
const REAL_SKILL_ID = "haven-agent-proposal"

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

/** 技能状态留在 Mock 单例里，因此每个用例都从「未启用」开始。 */
beforeEach(async () => {
  await getHavenClient().agentSkillSetEnabled({ skillId: MOCK_SKILL_ID, enabled: false })
})

function renderSkillsSettings() {
  return render(
    <MemoryRouter initialEntries={["/settings/skills"]}>
      <Routes>
        <Route path="/settings/:section" element={<SettingsPage />} />
      </Routes>
    </MemoryRouter>,
  )
}

/** 定位「本机内置技能」分组（SettingsGroup 渲染成 section）。 */
async function findSkillsGroup(): Promise<HTMLElement> {
  const heading = await screen.findByText("本机内置技能")
  const section = heading.closest("section")
  if (!section) throw new Error("未找到「本机内置技能」分组")
  return section
}

describe("设置页内置技能分组", () => {
  it("状态来自客户端：默认未启用，且不把预览条目伪装成真实内置技能", async () => {
    renderSkillsSettings()
    const group = await findSkillsGroup()

    expect(await within(group).findByText("未启用")).toBeTruthy()
    expect(within(group).queryByText("已启用")).toBeNull()
    // 预览条目必须自曝身份，并且绝不冒用真实技能 id。
    expect(within(group).getByText(MOCK_SKILL_ID)).toBeTruthy()
    expect(within(group).getByText(/浏览器预览项/)).toBeTruthy()
    expect(document.body.textContent).not.toContain(REAL_SKILL_ID)
    // 0 字如实呈现：浏览器里没有可注入正文，不说成「注入约 0 字」。
    expect(within(group).getByText("本次构建没有可注入正文")).toBeTruthy()
  })

  it("将过期授权明确显示为待重新确认，而不是已启用或未启用", async () => {
    const client = getHavenClient()
    vi.spyOn(client, "agentSkillList").mockResolvedValue({
      schemaVersion: 1,
      skills: [{
        schemaVersion: 1,
        skillId: MOCK_SKILL_ID,
        description: "浏览器预览项：桌面应用的内置技能目录由 Rust 侧提供",
        instructionsChars: 0,
        state: "stale",
      }],
    })

    renderSkillsSettings()
    const group = await findSkillsGroup()

    expect(await within(group).findByText("内容已更新，需重新确认")).toBeTruthy()
    expect(within(group).queryByText("已启用")).toBeNull()
    expect(within(group).queryByText("未启用")).toBeNull()
    expect(within(group).getByRole("button", { name: "启用" })).toBeTruthy()
  })

  it("启用走客户端 IPC：状态由返回值决定，按钮随之切换", async () => {
    const client = getHavenClient()
    const setEnabled = vi.spyOn(client, "agentSkillSetEnabled")
    renderSkillsSettings()
    const group = await findSkillsGroup()

    fireEvent.click(within(group).getByRole("button", { name: "启用" }))

    expect(await within(group).findByText("已启用")).toBeTruthy()
    expect(setEnabled).toHaveBeenCalledWith({ skillId: MOCK_SKILL_ID, enabled: true })
    expect(within(group).queryByText("未启用")).toBeNull()
    expect(within(group).getByRole("button", { name: "停用" })).toBeTruthy()

    fireEvent.click(within(group).getByRole("button", { name: "停用" }))
    expect(await within(group).findByText("未启用")).toBeTruthy()
    expect(setEnabled).toHaveBeenLastCalledWith({ skillId: MOCK_SKILL_ID, enabled: false })
    // 权威状态可回读：停用后重新读取仍是「未启用」。
    const listed = await getHavenClient().agentSkillList()
    expect(listed.skills.map((skill) => skill.state)).toEqual(["disabled"])
  })

  it("启停写入失败后明确显示错误，不把开关假装成成功", async () => {
    const client = getHavenClient()
    vi.spyOn(client, "agentSkillSetEnabled").mockRejectedValueOnce(new Error("write failed"))
    renderSkillsSettings()
    const group = await findSkillsGroup()

    fireEvent.click(await within(group).findByRole("button", { name: "启用" }))

    const alert = await within(group).findByRole("alert")
    expect(alert.textContent).toContain("读取或写入失败")
    expect(await within(group).findByText("未启用")).toBeTruthy()
    // 断言用 DOM 自身的事实（`HTMLButtonElement.disabled`），不引入 jest-dom matcher：
    // 仓库没有安装 `@testing-library/jest-dom`，`toBeDisabled` 只会得到"matcher 不存在"。
    const toggle = within(group).getByRole("button", { name: "启用" }) as HTMLButtonElement
    expect(toggle.disabled).toBe(true)
  })

  it("技能重读失败时隐藏旧行并禁用旧状态操作", async () => {
    const client = getHavenClient()
    const list = vi.spyOn(client, "agentSkillList")
    renderSkillsSettings()
    const group = await findSkillsGroup()
    await within(group).findByText("未启用")
    list.mockRejectedValueOnce(new Error("read failed"))

    fireEvent.click(within(group).getByRole("button", { name: "重新读取内置技能" }))

    expect(await within(group).findByRole("alert")).toBeTruthy()
    expect(within(group).queryByText(MOCK_SKILL_ID)).toBeNull()
    expect(within(group).queryByRole("button", { name: "启用" })).toBeNull()
  })

  it("写入失败之后「重新加载配置」成功，也不会把那次失败洗掉", async () => {
    // 回归点：一次成功的列表读取只说明"现在的状态是什么"，不说明"刚才那次启停为什么
    // 没生效"。若重读成功就清空错误，用户点完重新加载会看到界面一切正常，
    // 而他那次没生效的启停再没有任何解释。
    const client = getHavenClient()
    vi.spyOn(client, "agentSkillSetEnabled").mockRejectedValueOnce(new Error("write failed"))
    renderSkillsSettings()
    const group = await findSkillsGroup()

    fireEvent.click(await within(group).findByRole("button", { name: "启用" }))
    const failed = await within(group).findByRole("alert")
    expect(failed.textContent).toContain("读取或写入失败")

    const listSkills = vi.spyOn(client, "agentSkillList")
    const callsBeforeReload = listSkills.mock.calls.length
    fireEvent.click(within(group).getByRole("button", { name: "重新读取" }))
    await waitFor(() =>
      expect(listSkills.mock.calls.length).toBeGreaterThan(callsBeforeReload),
    )

    // 重读成功（默认 mock 返回一行未启用），但那一次写入失败必须继续可见。
    await within(group).findByText("未启用")
    const stillThere = within(group).getByRole("alert")
    expect(stillThere.textContent).toContain("读取或写入失败")
  })

  it("技能分组的重新读取会回读内置技能状态", async () => {
    const listSkills = vi.spyOn(getHavenClient(), "agentSkillList")
    renderSkillsSettings()
    const group = await findSkillsGroup()
    await within(group).findByText("未启用")

    const callsBeforeReload = listSkills.mock.calls.length
    fireEvent.click(within(group).getByRole("button", { name: "重新读取内置技能" }))

    await waitFor(() => expect(listSkills).toHaveBeenCalledTimes(callsBeforeReload + 1))
  })

  it("只描述权限边界：没有任何批准 / 应用 / 导入 / 编辑入口", async () => {
    renderSkillsSettings()
    const group = await findSkillsGroup()

    // 安全说明是分组上方的页面级 callout，不属于 SettingsGroup 的 section。
    const pageText = document.body.textContent ?? ""
    // 必须写明：技能不是权限，设置改动只有用户能在栖阅界面批准。
    for (const required of ["技能不是权限", "待批准提案", "批准"]) {
      expect(pageText).toContain(required)
    }
    for (const forbidden of [/批准/, /应用/, /导入/, /编辑/, /approve/i, /apply/i, /install/i]) {
      expect(within(group).queryByRole("button", { name: forbidden })).toBeNull()
    }
    // 正文与路径不经过前端：分组里不应出现任何形式的技能正文占位。
    expect(group.textContent).not.toContain("SKILL.md")
    expect(group.textContent).not.toContain("skills/")
  })
})

describe("内置技能页面路由", () => {
  it("浏览器预览直接打开独立分区；生产运行时白名单由 settings-runtime-state 测试验证", async () => {
    renderSkillsSettings()
    // 页面没有落到「当前版本不可用」的空态，而是渲染出真实分组。
    expect(await screen.findByText("本机内置技能")).toBeTruthy()
    expect(screen.queryByText("当前版本不可用")).toBeNull()
    // 工作台设置仍如实标明，技能只提供说明上下文，不额外授予工具或权限。
    expect(screen.getByText("技能不是权限")).toBeTruthy()
  })
})

// waitFor 只在这里用于兜底异步渲染（findBy* 已覆盖主要路径）。
describe("内置技能分组不会长时间停在读取中", () => {
  it("读取完成后不再显示「读取中」", async () => {
    renderSkillsSettings()
    await findSkillsGroup()
    await waitFor(() => expect(screen.queryByText("读取中")).toBeNull())
  })
})
