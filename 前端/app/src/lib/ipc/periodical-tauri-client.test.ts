import { beforeEach, describe, expect, it, vi } from "vitest"
import type { PeriodicalTreeDto, PeriodicalTreeGetRequest } from "./generated/wire"

const { invoke, check } = vi.hoisted(() => ({ invoke: vi.fn(), check: vi.fn() }))
vi.mock("@tauri-apps/api/core", () => ({
  Channel: class Channel<T> { onmessage?: (event: T) => void },
  invoke,
}))
vi.mock("@tauri-apps/plugin-updater", () => ({ check }))

import { TauriHavenClient } from "./tauri-client"

beforeEach(() => {
  invoke.mockReset()
})

const WORK_ID = "11111111-1111-4111-8111-111111111111"

function tree(): PeriodicalTreeDto {
  return {
    schemaVersion: 1,
    workId: WORK_ID,
    periodical: {
      id: "22222222-2222-4222-8222-222222222201",
      workId: WORK_ID,
      title: "Nature Communications",
      issnPrint: null,
      issnElectronic: null,
      publisher: null,
    },
    volumes: [],
  }
}

describe("TauriHavenClient periodical_tree_get", () => {
  it("maps only the typed work request under the command's request argument", async () => {
    const request: PeriodicalTreeGetRequest = { workId: WORK_ID }
    const payload = tree()
    invoke.mockResolvedValueOnce(payload)

    await expect(new TauriHavenClient().periodicalTreeGet(request)).resolves.toBe(payload)
    expect(invoke).toHaveBeenCalledWith("periodical_tree_get", { request })
  })

  it("passes the command result through without business validation", async () => {
    // 信任边界在 Gateway：Client 只做传输与错误归一，不能在这里偷偷兜底、
    // 修剪或补全投影，否则越界响应会被适配层洗白成"可用"数据。
    const untrusted = { schemaVersion: 1, workId: WORK_ID } as unknown as PeriodicalTreeDto
    invoke.mockResolvedValueOnce(untrusted)

    await expect(new TauriHavenClient().periodicalTreeGet({ workId: WORK_ID })).resolves.toBe(untrusted)
  })

  it("normalizes a contract error dto into a HavenError", async () => {
    invoke.mockRejectedValueOnce({
      code: "PERIODICAL_NOT_FOUND",
      userMessage: "该作品没有期刊层级",
      retryable: false,
    })

    await expect(new TauriHavenClient().periodicalTreeGet({ workId: WORK_ID }))
      .rejects.toMatchObject({ code: "PERIODICAL_NOT_FOUND", retryable: false })
  })

  it("normalizes an unknown rejection into INTERNAL_ERROR", async () => {
    invoke.mockRejectedValueOnce(new Error("ipc exploded"))

    await expect(new TauriHavenClient().periodicalTreeGet({ workId: WORK_ID }))
      .rejects.toMatchObject({ code: "INTERNAL_ERROR", retryable: false })
  })
})
