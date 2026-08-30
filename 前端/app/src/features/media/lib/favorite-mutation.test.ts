import { describe, expect, it } from "vitest"
import { HavenError } from "@/lib/ipc/errors"
import { settleFavoriteMutation } from "./favorite-mutation"

describe("favorite optimistic mutation", () => {
  it("rolls back to the previous value when the IPC write fails", async () => {
    const error = new HavenError({
      code: "DATABASE_ERROR",
      userMessage: "数据库暂时不可用",
      retryable: true,
    })
    const outcome = await settleFavoriteMutation(true, async () => {
      throw error
    })

    expect(outcome.favorite).toBe(true)
    expect(outcome.error).toBe(error)
  })

  it("uses the authoritative server value after a successful write", async () => {
    const outcome = await settleFavoriteMutation(false, async () => ({ favorite: true }))
    expect(outcome).toEqual({ favorite: true, error: null })
  })
})
