import { describe, expect, it, vi } from "vitest"
import type { HavenClient } from "@/lib/ipc/client"
import type { WorkDetailHeaderDto } from "@/lib/ipc/generated/wire"

const workGet = vi.fn< HavenClient["workGet"] >()
const libraryList = vi.fn(() => {
  throw new Error("library_list must not be called")
})

vi.mock("@/lib/ipc/runtime", () => ({
  isTauriRuntime: () => true,
  getHavenClient: () => ({ workGet, libraryList }),
}))

import { getWorkDetail } from "./work-gateway"

const header = { workId: "work-1" } as WorkDetailHeaderDto

describe("work gateway", () => {
  it("uses work_get through HavenClient and never library_list in Tauri", async () => {
    workGet.mockResolvedValue(header)
    await expect(getWorkDetail("work-1")).resolves.toBe(header)
    expect(workGet).toHaveBeenCalledWith({ workId: "work-1" })
    expect(libraryList).not.toHaveBeenCalled()
  })
})
