import { afterEach, describe, expect, it, vi } from "vitest"
import { fetchSessionResource } from "./resource-fetch"

const CONTENT_URI = "haven-resource://session/0196f0d2-0000-7000-8000-000000000001"

afterEach(() => {
  vi.unstubAllGlobals()
})

describe("fetchSessionResource", () => {
  it("rejects a non-session URI before fetch", async () => {
    const fetchMock = vi.fn()
    vi.stubGlobal("fetch", fetchMock)

    await expect(fetchSessionResource("https://example.test/private")).rejects.toMatchObject({
      code: "INVALID_ARGUMENT",
      retryable: false,
    })
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it("returns bytes and normalized MIME for an allowed resource", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(
      new Uint8Array([1, 2, 3]),
      { status: 200, headers: { "Content-Type": "video/mp4; charset=binary" } },
    )))

    const result = await fetchSessionResource(CONTENT_URI)

    expect(result).toMatchObject({ kind: "data", contentType: "video/mp4", partial: false })
    if (result.kind === "data") {
      expect(Array.from(new Uint8Array(result.bytes))).toEqual([1, 2, 3])
    }
  })

  it("sends a validated Range and marks a 206 response partial", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(
      new Uint8Array([4, 5]),
      {
        status: 206,
        headers: {
          "Content-Type": "application/pdf",
          "Content-Range": "bytes 10-11/100",
        },
      },
    ))
    vi.stubGlobal("fetch", fetchMock)

    const result = await fetchSessionResource(CONTENT_URI, { range: "bytes=10-19" })

    expect(fetchMock).toHaveBeenCalledWith(CONTENT_URI, {
      headers: { Range: "bytes=10-19" },
      signal: undefined,
    })
    expect(result).toMatchObject({
      kind: "data",
      contentType: "application/pdf",
      partial: true,
      totalBytes: 100,
      contentRange: { start: 10, end: 11, total: 100 },
    })
  })

  it("uses the WebView2 compatibility URL in a Windows Tauri runtime", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(
      new Uint8Array([1]),
      { status: 200, headers: { "Content-Type": "text/plain" } },
    ))
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} })
    vi.stubGlobal("navigator", { userAgent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64)" })
    vi.stubGlobal("fetch", fetchMock)

    await fetchSessionResource(CONTENT_URI)

    expect(fetchMock).toHaveBeenCalledWith(
      "http://haven-resource.session/0196f0d2-0000-7000-8000-000000000001",
      { headers: undefined, signal: undefined },
    )
  })

  it("accepts a suffix Range supported by the resource protocol", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(
      new Uint8Array([8, 9]),
      { status: 206, headers: { "Content-Type": "text/plain; charset=utf-8" } },
    ))
    vi.stubGlobal("fetch", fetchMock)

    await fetchSessionResource(CONTENT_URI, { range: "bytes=-2" })

    expect(fetchMock).toHaveBeenCalledWith(CONTENT_URI, {
      headers: { Range: "bytes=-2" },
      signal: undefined,
    })
  })

  it("returns empty for an allowed zero-byte response", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(
      new Uint8Array(),
      { status: 200, headers: { "Content-Type": "text/plain" } },
    )))

    await expect(fetchSessionResource(CONTENT_URI)).resolves.toMatchObject({
      kind: "empty",
      contentType: "text/plain",
      partial: false,
    })
  })

  it("rejects an invalid Range before fetch", async () => {
    const fetchMock = vi.fn()
    vi.stubGlobal("fetch", fetchMock)

    await expect(fetchSessionResource(CONTENT_URI, { range: "bytes=20-10" })).rejects.toMatchObject({
      code: "INVALID_ARGUMENT",
    })
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it("rejects missing, generic, and non-whitelisted MIME values", async () => {
    for (const contentType of [null, "application/octet-stream", "application/zip", "text/xml", "application/json"]) {
      const headers = contentType === null ? undefined : { "Content-Type": contentType }
      vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(new Uint8Array([1]), { status: 200, headers })))

      await expect(fetchSessionResource(CONTENT_URI)).rejects.toMatchObject({
        code: "FORMAT_UNSUPPORTED",
        retryable: false,
      })
    }
  })

  it("accepts Markdown and HTML document resources", async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(new Response("# Markdown", { status: 200, headers: { "Content-Type": "text/markdown" } }))
      .mockResolvedValueOnce(new Response("<p>HTML</p>", { status: 200, headers: { "Content-Type": "text/html; charset=utf-8" } }))
    vi.stubGlobal("fetch", fetchMock)

    await expect(fetchSessionResource(CONTENT_URI)).resolves.toMatchObject({ kind: "data", contentType: "text/markdown" })
    await expect(fetchSessionResource(CONTENT_URI)).resolves.toMatchObject({ kind: "data", contentType: "text/html" })
  })

  it("maps HTTP statuses to stable catalog codes without reading response bodies", async () => {
    const serverFailure = new Response("private body", { status: 503 })
    const serverBody = vi.spyOn(serverFailure, "arrayBuffer")
    vi.stubGlobal("fetch", vi.fn().mockResolvedValueOnce(serverFailure).mockResolvedValueOnce(
      new Response("private body", { status: 404 }),
    ))

    await expect(fetchSessionResource(CONTENT_URI)).rejects.toMatchObject({
      code: "INTERNAL_ERROR",
      retryable: false,
    })
    await expect(fetchSessionResource(CONTENT_URI)).rejects.toMatchObject({
      code: "RESOURCE_NOT_FOUND",
      retryable: false,
    })
    expect(serverBody).not.toHaveBeenCalled()
  })

  it("maps a valid range request rejected by the provider to an explicit download fallback", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(null, { status: 501 })))

    await expect(fetchSessionResource(CONTENT_URI, { range: "bytes=0-0" })).rejects.toMatchObject({
      code: "SOURCE_RANGE_UNSUPPORTED",
      message: "该远端正文不支持分段读取，请先下载到本地",
      retryable: false,
    })
  })

  it("maps an oversized response to a safe non-retryable catalog error without reading its body", async () => {
    const response = new Response("private body", { status: 413 })
    const responseBody = vi.spyOn(response, "arrayBuffer")
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(response))

    await expect(fetchSessionResource(CONTENT_URI)).rejects.toMatchObject({
      code: "FORMAT_UNSUPPORTED",
      message: "资源超过当前版本的 32 MiB 大小限制",
      retryable: false,
    })
    expect(responseBody).not.toHaveBeenCalled()
  })

  it("maps a network failure to a safe catalog HavenError", async () => {
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new Error("socket leaked details")))

    await expect(fetchSessionResource(CONTENT_URI)).rejects.toMatchObject({
      code: "RESOURCE_UNAVAILABLE",
      message: "资源暂时无法读取",
      retryable: false,
    })
  })

  it("maps an aborted request to OPERATION_CANCELLED", async () => {
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new DOMException("private", "AbortError")))

    await expect(fetchSessionResource(CONTENT_URI)).rejects.toMatchObject({
      code: "OPERATION_CANCELLED",
      retryable: false,
    })
  })
})
