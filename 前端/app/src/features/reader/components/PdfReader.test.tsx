// @vitest-environment jsdom

import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react"
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

/**
 * 渲染任务的所有权（审计 B02）。
 *
 * 第 1 页的光栅画完、文本提取还没返回时就切到第 2 页；第 1 页的文本稍后返回，它的
 * `finally` 曾经无条件把共享 ref 置空——第 2 页仍然在跑的光栅从此失去取消句柄，后续
 * 切页 / 缩放 / 卸载都停不下它。旧的 `finally` 只能碰自己那一份任务。
 */
describe("PdfReader render task ownership", () => {
  beforeEach(() => {
    pdfDocument.loadPdfDocument.mockReset()
    pdfDocument.destroyPdfDocument.mockClear()
    vi.spyOn(HTMLCanvasElement.prototype, "getContext")
      .mockImplementation((function (this: HTMLCanvasElement) {
        return createCanvas2dContext(this)
      }) as unknown as typeof HTMLCanvasElement.prototype.getContext)
  })

  afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
  })

  it("cancels the current raster task even if an older text extraction settles late", async () => {
    let finishOldText!: (value: { items: [] }) => void
    const oldText = new Promise<{ items: [] }>((resolve) => { finishOldText = resolve })
    const cancelSecond = vi.fn()
    const cancelThird = vi.fn()
    const renderSecond = vi.fn(() => ({ promise: new Promise<void>(() => {}), cancel: cancelSecond }))
    const renderThird = vi.fn(() => ({ promise: new Promise<void>(() => {}), cancel: cancelThird }))
    const viewport = ({ scale }: { scale: number }) => ({ width: 600 * scale, height: 800 * scale, scale })
    const pages = [
      { getViewport: viewport, render: vi.fn(() => ({ promise: Promise.resolve(), cancel: vi.fn() })), getTextContent: vi.fn(() => oldText), cleanup: vi.fn() },
      { getViewport: viewport, render: renderSecond, getTextContent: vi.fn(async () => ({ items: [] })), cleanup: vi.fn() },
      { getViewport: viewport, render: renderThird, getTextContent: vi.fn(async () => ({ items: [] })), cleanup: vi.fn() },
    ]
    pdfDocument.loadPdfDocument.mockResolvedValue({ numPages: 3, getPage: async (n: number) => pages[n - 1] })

    const view = render(<PdfReader bytes={new ArrayBuffer(8)} />)
    await waitFor(() => expect(pages[0].getTextContent).toHaveBeenCalled())
    fireEvent.click(view.getByRole("button", { name: "下一页" }))
    await waitFor(() => expect(renderSecond).toHaveBeenCalled())

    // 第 1 页的文本晚到：它只能清理自己的任务，不能抹掉第 2 页的取消句柄。
    await act(async () => { finishOldText({ items: [] }); await Promise.resolve() })
    expect(cancelSecond).not.toHaveBeenCalled()

    // 再一次切页：第 2 页的仍可能挂着的任务必须真的被取消。
    fireEvent.click(view.getByRole("button", { name: "下一页" }))
    await waitFor(() => expect(renderThird).toHaveBeenCalled())
    expect(cancelSecond).toHaveBeenCalled()
  })

  it("cancels its own raster task when the document unmounts after a late text settle", async () => {
    let finishOldText!: (value: { items: [] }) => void
    const oldText = new Promise<{ items: [] }>((resolve) => { finishOldText = resolve })
    const cancelSecond = vi.fn()
    const viewport = ({ scale }: { scale: number }) => ({ width: 600 * scale, height: 800 * scale, scale })
    const pages = [
      { getViewport: viewport, render: vi.fn(() => ({ promise: Promise.resolve(), cancel: vi.fn() })), getTextContent: vi.fn(() => oldText), cleanup: vi.fn() },
      { getViewport: viewport, render: vi.fn(() => ({ promise: new Promise<void>(() => {}), cancel: cancelSecond })), getTextContent: vi.fn(async () => ({ items: [] })), cleanup: vi.fn() },
    ]
    pdfDocument.loadPdfDocument.mockResolvedValue({ numPages: 2, getPage: async (n: number) => pages[n - 1] })

    const view = render(<PdfReader bytes={new ArrayBuffer(8)} />)
    await waitFor(() => expect(pages[0].getTextContent).toHaveBeenCalled())
    fireEvent.click(view.getByRole("button", { name: "下一页" }))
    await waitFor(() => expect(pages[1].render).toHaveBeenCalled())
    await act(async () => { finishOldText({ items: [] }); await Promise.resolve() })

    view.unmount()
    expect(cancelSecond).toHaveBeenCalled()
  })
})
