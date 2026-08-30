import { describe, expect, it } from "vitest"
import {
  MAX_RENDERED_PDF_PAGES,
  MAX_PDF_ZOOM,
  MIN_PDF_ZOOM,
  clampPdfPage,
  clampPdfZoom,
  isPdfMimeType,
  normalizePdfMimeType,
  resolvePdfInitialView,
} from "./pdf-reader-state"

describe("pdf reader state helpers", () => {
  it("accepts only the PDF MIME type and ignores parameters", () => {
    expect(isPdfMimeType("application/pdf")).toBe(true)
    expect(isPdfMimeType("Application/PDF; charset=binary")).toBe(true)
    expect(isPdfMimeType("application/octet-stream")).toBe(false)
    expect(isPdfMimeType(null)).toBe(false)
    expect(normalizePdfMimeType(" application/pdf ; charset=binary ")).toBe("application/pdf")
  })

  it("clamps page numbers to a valid document boundary", () => {
    expect(clampPdfPage(0, 4)).toBe(1)
    expect(clampPdfPage(2, 4)).toBe(2)
    expect(clampPdfPage(9, 4)).toBe(4)
    expect(clampPdfPage(1, 0)).toBe(0)
  })

  it("limits rendering to the active page regardless of document length", () => {
    expect(MAX_RENDERED_PDF_PAGES).toBe(1)
  })

  it("clamps zoom to the supported rendering range", () => {
    expect(clampPdfZoom(0)).toBe(MIN_PDF_ZOOM)
    expect(clampPdfZoom(1.25)).toBe(1.25)
    expect(clampPdfZoom(99)).toBe(MAX_PDF_ZOOM)
    expect(clampPdfZoom(Number.NaN)).toBe(1)
  })

  it("resolves a zero-based locator to the one-based PDF render page", () => {
    expect(resolvePdfInitialView(5, { pageIndex: 2, zoom: 1.25 })).toEqual({ page: 3, zoom: 1.25 })
  })

  it("rejects empty page counts and clamps restored page boundaries", () => {
    expect(resolvePdfInitialView(0, { pageIndex: 0, zoom: 1 })).toBeNull()
    expect(resolvePdfInitialView(1.5, { pageIndex: 0, zoom: 1 })).toBeNull()
    expect(resolvePdfInitialView(5, { pageIndex: -1, zoom: 1 })).toEqual({ page: 1, zoom: 1 })
    expect(resolvePdfInitialView(5, { pageIndex: 99, zoom: 1 })).toEqual({ page: 5, zoom: 1 })
  })

  it("defaults invalid zoom and clamps restored zoom boundaries", () => {
    expect(resolvePdfInitialView(5, { pageIndex: Number.NaN, zoom: null })).toEqual({ page: 1, zoom: 1 })
    expect(resolvePdfInitialView(5, { pageIndex: 0, zoom: Number.NaN })).toEqual({ page: 1, zoom: 1 })
    expect(resolvePdfInitialView(5, { pageIndex: 0, zoom: 0 })).toEqual({ page: 1, zoom: MIN_PDF_ZOOM })
    expect(resolvePdfInitialView(5, { pageIndex: 0, zoom: 99 })).toEqual({ page: 1, zoom: MAX_PDF_ZOOM })
  })

  it("resolves replacement Session inputs independently", () => {
    const first = resolvePdfInitialView(10, { pageIndex: 8, zoom: 2 })
    const replacement = resolvePdfInitialView(3, { pageIndex: 0, zoom: 0.75 })
    expect(first).toEqual({ page: 9, zoom: 2 })
    expect(replacement).toEqual({ page: 1, zoom: 0.75 })
  })
})
