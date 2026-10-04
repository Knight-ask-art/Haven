import { describe, expect, it, vi } from "vitest"
import { querySystemFontCatalog, searchSystemFonts, type LocalFontRecord } from "./system-font-catalog"

describe("system font catalog", () => {
  it("distinguishes unsupported environments from an empty font list", async () => {
    await expect(querySystemFontCatalog(undefined)).resolves.toEqual({ status: "unsupported" })
    await expect(querySystemFontCatalog(async () => [])).resolves.toEqual({ status: "ready", fonts: [] })
  })

  it("searches localized font names by pinyin and English aliases", async () => {
    const records: LocalFontRecord[] = [
      { family: "霞鹜文楷", fullName: "霞鹜文楷", postscriptName: "LXGWWenKai-Regular" },
      { family: "Consolas", fullName: "Consolas", postscriptName: "Consolas" },
    ]
    const result = await querySystemFontCatalog(async () => records)
    expect(result.status).toBe("ready")
    if (result.status !== "ready") throw new Error("font catalog did not load")

    expect(searchSystemFonts(result.fonts, "xiawuwen").map((font) => font.family)).toEqual(["霞鹜文楷"])
    expect(searchSystemFonts(result.fonts, "lxgw wen kai").map((font) => font.family)).toEqual(["霞鹜文楷"])
    expect(searchSystemFonts(result.fonts, "consolas").map((font) => font.family)).toEqual(["Consolas"])
  })

  it("deduplicates font families and rejects unsafe names before they reach CSS", async () => {
    const result = await querySystemFontCatalog(async () => [
      { family: "Segoe UI", fullName: "Segoe UI Regular" },
      { family: "Segoe  UI", fullName: "Segoe UI Variable" },
      { family: "bad); color:red", fullName: "Injected CSS" },
      { family: "../font.woff2", fullName: "Path" },
    ])

    expect(result.status).toBe("ready")
    if (result.status !== "ready") throw new Error("font catalog did not load")
    expect(result.fonts).toHaveLength(1)
    expect(result.fonts[0]?.fullName).toBe("Segoe UI Variable")
  })

  it("returns permission and generic failures as distinct user-actionable states", async () => {
    const denied = Object.assign(new Error("permission denied"), { name: "NotAllowedError" })
    await expect(querySystemFontCatalog(async () => { throw denied })).resolves.toEqual({ status: "denied" })

    const failure = vi.fn(async () => { throw new Error("font service failed") })
    await expect(querySystemFontCatalog(failure)).resolves.toEqual({ status: "error" })
    expect(failure).toHaveBeenCalledOnce()
  })
})
