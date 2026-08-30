import { describe, expect, it } from "vitest"
import { MAX_PDF_BYTES, loadPdfDocument, PdfReaderError } from "./pdf-document"

function expectedError(code: PdfReaderError["code"]): (error: unknown) => boolean {
  return (error) => error instanceof PdfReaderError && error.code === code
}

describe("PDF document input boundary", () => {
  it("matches the resource protocol single-response limit", () => {
    expect(MAX_PDF_BYTES).toBe(32 * 1024 * 1024)
  })

  it("rejects non-PDF MIME before invoking the parser", async () => {
    await expect(loadPdfDocument(new ArrayBuffer(4), { mimeType: "text/plain" }))
      .rejects.toSatisfy(expectedError("PDF_UNSUPPORTED_MIME"))
  })

  it("rejects empty and oversized payloads before parsing", async () => {
    await expect(loadPdfDocument(new ArrayBuffer(0), { mimeType: "application/pdf" }))
      .rejects.toSatisfy(expectedError("PDF_INVALID_INPUT"))
    await expect(loadPdfDocument(new ArrayBuffer(MAX_PDF_BYTES + 1), { mimeType: "application/pdf" }))
      .rejects.toSatisfy(expectedError("PDF_TOO_LARGE"))
  })

  it("honors a cancelled request before parsing", async () => {
    const abortController = new AbortController()
    abortController.abort()
    await expect(loadPdfDocument(new ArrayBuffer(4), {
      mimeType: "application/pdf",
      signal: abortController.signal,
    })).rejects.toSatisfy(expectedError("PDF_CANCELLED"))
  })
})
