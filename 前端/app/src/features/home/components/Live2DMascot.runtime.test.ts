import { describe, expect, it } from "vitest"
import {
  loadOptionalOml2dRuntime,
  type Oml2dRuntimeModule,
} from "./live2d-runtime"

function createRuntimeModule(): Oml2dRuntimeModule {
  return {
    loadOml2d: (() => ({})) as unknown as Oml2dRuntimeModule["loadOml2d"],
  }
}

describe("loadOptionalOml2dRuntime", () => {
  it("loads the local Cubism runtime before evaluating the component module", async () => {
    const calls: string[] = []
    const module = createRuntimeModule()
    const runtime = await loadOptionalOml2dRuntime(
      async () => {
        calls.push("module")
        return module
      },
      async () => {
        calls.push("cubism")
      },
      async () => {
        calls.push("stylesheet")
      },
    )

    expect(runtime).toBe(module)
    expect(calls).toEqual(["stylesheet", "cubism", "module"])
  })

  it("does not evaluate the component module after stylesheet loading is cancelled", async () => {
    const controller = new AbortController()
    let moduleLoads = 0
    const runtime = await loadOptionalOml2dRuntime(
      async () => {
        moduleLoads += 1
        return createRuntimeModule()
      },
      async () => undefined,
      async () => {
        controller.abort()
      },
      controller.signal,
    )

    expect(runtime).toBeNull()
    expect(moduleLoads).toBe(0)
  })

  it("converts a local Cubism runtime failure into an unavailable component", async () => {
    let moduleLoads = 0
    const runtime = await loadOptionalOml2dRuntime(
      async () => {
        moduleLoads += 1
        return createRuntimeModule()
      },
      async () => {
        throw new Error("CSP rejected the local runtime")
      },
      async () => undefined,
    )

    expect(runtime).toBeNull()
    expect(moduleLoads).toBe(0)
  })

  it("converts a third-party module evaluation failure into an unavailable runtime", async () => {
    const runtime = await loadOptionalOml2dRuntime(async () => {
      throw new Error("CSP rejected the Cubism runtime script")
    }, async () => undefined, async () => undefined)

    expect(runtime).toBeNull()
  })
})
