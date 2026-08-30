import fs from "node:fs"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"
import {
  injectPixiUnsafeEvalPatch,
  live2dCspPlugin,
} from "./live2d-csp-plugin"

const unsafeEvalModulePath = fileURLToPath(import.meta.resolve("@pixi/unsafe-eval"))
const unsafeEvalSource = fs.readFileSync(unsafeEvalModulePath, "utf8")

describe("live2dCspPlugin", () => {
  it("emits the pinned primary-color CSS and disables dynamic style injection", async () => {
    const plugin = live2dCspPlugin()
    const emittedAssets: Array<{ fileName?: string; source?: string | Uint8Array }> = []
    const generateBundle = plugin.generateBundle
    const transform = plugin.transform
    const modulePath = fileURLToPath(import.meta.resolve("oh-my-live2d"))

    expect(typeof generateBundle).toBe("function")
    expect(typeof transform).toBe("function")
    if (typeof generateBundle !== "function" || typeof transform !== "function") return

    await generateBundle.call({
      emitFile(asset: { fileName?: string; source?: string | Uint8Array }) {
        emittedAssets.push(asset)
        return "asset-id"
      },
    } as never, {} as never, {} as never, false)
    const stylesheet = emittedAssets.find(
      (asset) => asset.fileName === "live2d/vendor/oml2d.css",
    )
    const result = await transform.call({} as never, "", modulePath)
    const code = typeof result === "string" ? result : result?.code

    expect(stylesheet?.source).toContain("@keyframes oml2d-stage-slide-in")
    expect(stylesheet?.source).toContain("color: #007aff")
    expect(stylesheet?.source).toContain("#oml2d-menus")
    expect(code).not.toContain("this.globalStyle.initialize()")
  })

  it("installs the Pixi 6.5.10 CSP-safe ShaderSystem implementation into embedded Pixi", async () => {
    const plugin = live2dCspPlugin()
    const transform = plugin.transform
    const modulePath = fileURLToPath(import.meta.resolve("oh-my-live2d"))

    expect(typeof transform).toBe("function")
    if (typeof transform !== "function") return

    const result = await transform.call({} as never, "", modulePath)
    const code = typeof result === "string" ? result : result?.code

    expect(code).toContain("function syncUniforms(group, uniformData, ud, uv, renderer)")
    expect(code).toContain("install({ ShaderSystem: qu });")
  })

  it("fails closed when the embedded Pixi ShaderSystem layout changes", () => {
    expect(() => injectPixiUnsafeEvalPatch("const qu = {};", unsafeEvalSource)).toThrow(
      /embedded Pixi ShaderSystem layout changed/,
    )
  })

  it("fails closed when the pinned Pixi CSP implementation version changes", () => {
    const changedVersion = unsafeEvalSource.replace(
      "@pixi/unsafe-eval - v6.5.10",
      "@pixi/unsafe-eval - v6.5.11",
    )

    expect(() => injectPixiUnsafeEvalPatch("", changedVersion)).toThrow(
      /@pixi\/unsafe-eval version changed/,
    )
  })
})
