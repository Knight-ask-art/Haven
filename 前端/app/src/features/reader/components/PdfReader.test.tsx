// @vitest-environment jsdom

import { cleanup, render, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { PdfReader } from "./PdfReader"

/**
 * PDF.js 6 hands the raster target to the renderer as `canvasContext`, and the
 * legacy build reads the element back off it (`this._canvas =
 * params.canvasContext.canvas`). The fake page below repeats that dereference,
 * so a render call shaped for the older `canvas` option fails here the same way
 * it fails in the real renderer instead of only failing the type checker.
 */
interface RenderParams {
  canvas: HTMLCanvasElement | null
  canvasContext?: CanvasRenderingContext2D
  viewport: { width: number; height: number }
  transform?: number[]
}

const pdfDocument = vi.hoisted(() => ({
  loadPdfDocument: vi.fn(),
  destroyPdfDocument: vi.fn(async () => undefined),
  PdfReaderError: class PdfReaderError extends Error {
    readonly code: string
    constructor(code: string, message: string) {
      super(message)
      this.name = "PdfReaderError"
      this.code = code
    }
  },
}))

vi.mock("../lib/pdf-document", () => pdfDocument)

vi.mock("pdfjs-dist/legacy/build/pdf.mjs", () => ({
  TextLayer: class TextLayer {
    constructor(_options: unknown) {}
    async render(): Promise<void> {}
    cancel(): void {}
  },
}))

function createCanvas2dContext(canvas: HTMLCanvasElement): CanvasRenderingContext2D {
  return { canvas, clearRect: vi.fn() } as unknown as CanvasRenderingContext2D
}

function createFakePage(renderCalls: RenderParams[]) {
  return {
    getViewport: ({ scale }: { scale: number }) => ({ width: 600 * scale, height: 800 * scale, scale }),
    streamTextContent: () => ({ items: [] }),
    getTextContent: async () => ({ items: [] }),
    cleanup: vi.fn(),
    render: (params: RenderParams) => {
      renderCalls.push(params)
      if (!params.canvasContext?.canvas) {
        throw new TypeError("Cannot read properties of undefined (reading 'canvas')")
      }
      return { promise: Promise.resolve(), cancel: vi.fn() }
    },
  }
}

function loadOnePageDocument(renderCalls: RenderParams[]) {
  const page = createFakePage(renderCalls)
  pdfDocument.loadPdfDocument.mockResolvedValue({ numPages: 1, getPage: async () => page })
}

describe("PdfReader canvas rendering", () => {
  beforeEach(() => {
    pdfDocument.loadPdfDocument.mockReset()
    pdfDocument.destroyPdfDocument.mockClear()
    // jsdom ships no 2D rasterizer, so the reader's canvas context is stubbed.
    // Only the context object matters here, since PDF.js dereferences its
    // `canvas` back-reference.
    const getContext = function (this: HTMLCanvasElement) {
      return createCanvas2dContext(this)
    }
    vi.spyOn(HTMLCanvasElement.prototype, "getContext")
      .mockImplementation(getContext as unknown as typeof HTMLCanvasElement.prototype.getContext)
  })

  afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
  })

  it("renders through the canvas 2D context rather than the canvas element", async () => {
    const renderCalls: RenderParams[] = []
    const onLocatorChange = vi.fn()
    loadOnePageDocument(renderCalls)

    const { container } = render(<PdfReader bytes={new ArrayBuffer(8)} onLocatorChange={onLocatorChange} />)

    await waitFor(() => expect(onLocatorChange).toHaveBeenCalled())
    expect(onLocatorChange.mock.calls[0][0]).toMatchObject({ pageIndex: 0, pageCount: 1, zoom: 1 })

    const canvas = container.querySelector("canvas")
    expect(canvas).not.toBeNull()
    expect(renderCalls).toHaveLength(1)
    expect(renderCalls[0].canvas).toBeNull()
    expect(renderCalls[0].canvasContext?.canvas).toBe(canvas)
    expect(renderCalls[0].viewport).toMatchObject({ width: 600, height: 800 })
  })

  it("keeps the device-pixel-ratio transform and backing store size", async () => {
    vi.stubGlobal("devicePixelRatio", 2)
    const renderCalls: RenderParams[] = []
    const onLocatorChange = vi.fn()
    loadOnePageDocument(renderCalls)

    const { container } = render(<PdfReader bytes={new ArrayBuffer(8)} onLocatorChange={onLocatorChange} />)

    await waitFor(() => expect(onLocatorChange).toHaveBeenCalled())
    expect(renderCalls[0].transform).toEqual([2, 0, 0, 2, 0, 0])
    const canvas = container.querySelector("canvas")
    expect(canvas?.width).toBe(1200)
    expect(canvas?.height).toBe(1600)
  })
})
