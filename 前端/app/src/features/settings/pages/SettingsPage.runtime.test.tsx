// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { SettingsRuntimeStatusNotice } from "./SettingsPage"

afterEach(() => cleanup())

describe("SettingsRuntimeStatusNotice", () => {
  it("does not add noise while runtime settings are loading or ready", () => {
    const { rerender } = render(
      <SettingsRuntimeStatusNotice status="loading" onRetry={() => undefined} />,
    )
    expect(screen.queryByRole("alert")).toBeNull()

    rerender(<SettingsRuntimeStatusNotice status="ready" onRetry={() => undefined} />)
    expect(screen.queryByRole("alert")).toBeNull()
  })

  it("explains the safe default and exposes the single runtime reload action", () => {
    const onRetry = vi.fn()
    render(<SettingsRuntimeStatusNotice status="degraded" onRetry={onRetry} />)

    expect(screen.getByRole("alert")).toBeTruthy()
    expect(screen.getByText("外观配置读取不完整")).toBeTruthy()
    expect(screen.getByText(/当前页面使用安全默认值/)).toBeTruthy()

    fireEvent.click(screen.getByRole("button", { name: "重新读取" }))
    expect(onRetry).toHaveBeenCalledTimes(1)
  })
})
