// @vitest-environment jsdom
//
// 连接面板的**禁用与文案**用例：没配置 OAuth 客户端时，唯一的主按钮必须禁用，
// 页面要如实说明"暂不能发起授权"，同时仍然只承诺只读范围——不能一边禁用，
// 一边让文案暗示可以上传、同步或备份。

import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const gateway = vi.hoisted(() => ({
  begin: vi.fn(),
  poll: vi.fn(),
  complete: vi.fn(),
  cancel: vi.fn(),
}))

// 面板与真实 useCloudAuthorization 只通过这个 IO 边界取数。
vi.mock("../ipc/cloud-storage-gateway", () => ({ cloudStorageGateway: gateway }))

import { CloudConnectPanel } from "./CloudConnectPanel"

beforeEach(() => {
  for (const method of Object.values(gateway)) method.mockReset()
})

afterEach(() => { cleanup() })

describe("CloudConnectPanel 的授权可用性", () => {
  it("未配置 OAuth 时禁用「打开 Google 授权页」，并如实说明范围与暂不可用", () => {
    render(<CloudConnectPanel available={false} onBack={vi.fn()} onAuthorized={vi.fn()} />)

    const authorize = screen.getByRole("button", { name: "打开 Google 授权页" }) as HTMLButtonElement
    expect(authorize.disabled, "未配置 OAuth 客户端时不得让用户发起授权").toBe(true)

    expect(screen.getByText("当前应用未配置 Google OAuth 客户端，暂不能发起授权。")).toBeTruthy()
    expect(screen.getByText("Google Drive 账户级只读权限")).toBeTruthy()
    expect(screen.getByText(/不上传、不删除云盘文件，不自动同步或备份。/)).toBeTruthy()
    expect(screen.queryByRole("button", { name: /上传|同步|备份/ }), "只读切片不得提供写入入口").toBeNull()
  })

  it("已配置 OAuth 时同一个按钮可用，禁用只由可用性决定", () => {
    render(<CloudConnectPanel available onBack={vi.fn()} onAuthorized={vi.fn()} />)
    expect((screen.getByRole("button", { name: "打开 Google 授权页" }) as HTMLButtonElement).disabled).toBe(false)
  })
})
