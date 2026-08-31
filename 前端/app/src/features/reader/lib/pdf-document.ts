import {
  GlobalWorkerOptions,
  PDFDataRangeTransport,
  getDocument,
} from "pdfjs-dist/legacy/build/pdf.mjs"
import type { PDFDocumentLoadingTask, PDFDocumentProxy } from "pdfjs-dist/legacy/build/pdf.mjs"
import pdfWorkerUrl from "pdfjs-dist/legacy/build/pdf.worker.min.mjs?url"
import { isPdfMimeType } from "./pdf-reader-state"
import { fetchSessionResource } from "@/features/session/ipc/resource-fetch"

export const MAX_PDF_BYTES = 32 * 1024 * 1024
/**
 * Range-backed documents are not assembled in the WebView, so their total
 * size may exceed the single-response cap. Keep a bounded document ceiling so
 * a corrupt server-side length cannot turn the reader into an unbounded client.
 */
export const MAX_PDF_DOCUMENT_BYTES = 512 * 1024 * 1024
export const PDF_RANGE_CHUNK_SIZE = 64 * 1024

export type PdfReaderErrorCode =
  | "PDF_INVALID_INPUT"
  | "PDF_UNSUPPORTED_MIME"
  | "PDF_TOO_LARGE"
  | "PDF_CANCELLED"
  | "PDF_RANGE_UNSUPPORTED"
  | "PDF_RANGE_FAILED"
  | "PDF_LOAD_FAILED"

export class PdfReaderError extends Error {
  readonly code: PdfReaderErrorCode

  constructor(code: PdfReaderErrorCode, message: string) {
    super(message)
    this.name = "PdfReaderError"
    this.code = code
  }
}

export interface PdfDocumentLoadOptions {
  mimeType?: string | null
  signal?: AbortSignal
}

export interface PdfSessionSource {
  /** Canonical `haven-resource://session/<uuid>` URI, never an arbitrary URL. */
  contentUri: string
  /** Total byte length obtained from a bounded range probe. */
  totalBytes: number
  /** Optional bounded prefix already fetched during the MIME/range probe. */
  initialData?: ArrayBuffer
}

export type PdfDocumentSource = ArrayBuffer | PdfSessionSource

let workerConfigured = false

function configurePdfWorker(): void {
  if (workerConfigured) return
  // Vite resolves this local asset into the application bundle; no remote worker URL is used.
  GlobalWorkerOptions.workerSrc = pdfWorkerUrl
  workerConfigured = true
}

function createCancelledError(): PdfReaderError {
  return new PdfReaderError("PDF_CANCELLED", "PDF loading was cancelled")
}

function assertPdfInput(bytes: ArrayBuffer, options: PdfDocumentLoadOptions): void {
  if (!(bytes instanceof ArrayBuffer)) {
    throw new PdfReaderError("PDF_INVALID_INPUT", "PDF input must be an ArrayBuffer")
  }
  if (options.mimeType !== undefined && options.mimeType !== null && !isPdfMimeType(options.mimeType)) {
    throw new PdfReaderError("PDF_UNSUPPORTED_MIME", "Only application/pdf resources can be opened")
  }
  if (bytes.byteLength === 0) {
    throw new PdfReaderError("PDF_INVALID_INPUT", "PDF input is empty")
  }
  if (bytes.byteLength > MAX_PDF_BYTES) {
    throw new PdfReaderError("PDF_TOO_LARGE", "PDF exceeds the supported size limit")
  }
}

function assertPdfSessionSource(source: PdfSessionSource): void {
  if (
    typeof source.contentUri !== "string"
    || !/^haven-resource:\/\/session\/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(source.contentUri)
  ) {
    throw new PdfReaderError("PDF_INVALID_INPUT", "PDF resource URI is invalid")
  }
  if (!Number.isSafeInteger(source.totalBytes) || source.totalBytes <= 0) {
    throw new PdfReaderError("PDF_INVALID_INPUT", "PDF resource length is invalid")
  }
  if (source.totalBytes > MAX_PDF_DOCUMENT_BYTES) {
    throw new PdfReaderError("PDF_TOO_LARGE", "PDF exceeds the supported size limit")
  }
}

/**
 * PDF.js' built-in URL loader starts with an unbounded GET before it knows
 * whether a server supports ranges. That is unsafe for a custom protocol,
 * where the handler would already have materialised the response. This
 * transport gives PDF.js only the exact chunks it asks for and keeps every
 * request behind the existing session resource fetch boundary.
 */
class SessionPdfRangeTransport extends PDFDataRangeTransport {
  private readonly contentUri: string
  private readonly parentSignal?: AbortSignal
  private readonly controllers = new Set<AbortController>()
  private aborted = false

  constructor(length: number, contentUri: string, initialData?: ArrayBuffer, parentSignal?: AbortSignal) {
    super(length, initialData ? new Uint8Array(initialData) : null, false)
    this.contentUri = contentUri
    this.parentSignal = parentSignal
    if (parentSignal) {
      parentSignal.addEventListener("abort", () => this.abort(), { once: true })
    }
  }

  requestDataRange(begin: number, end: number): void {
    if (this.aborted) return
    if (
      !Number.isSafeInteger(begin)
      || !Number.isSafeInteger(end)
      || begin < 0
      || end <= begin
      || end > this.length
    ) {
      this.onDataRange(begin, new Uint8Array())
      return
    }

    const controller = new AbortController()
    this.controllers.add(controller)
    const abortFromParent = () => controller.abort()
    this.parentSignal?.addEventListener("abort", abortFromParent, { once: true })
    void fetchSessionResource(this.contentUri, {
      range: `bytes=${begin}-${end - 1}`,
      signal: controller.signal,
    })
      .then((payload) => {
        if (this.aborted || controller.signal.aborted) return
        const range = payload.contentRange
        if (
          payload.contentType !== "application/pdf"
          || !payload.partial
          || !range
          || range.start !== begin
          || range.end < begin
          || range.end >= end
          || range.total !== this.length
          || payload.bytes.byteLength !== range.end - range.start + 1
        ) {
          throw new PdfReaderError("PDF_RANGE_FAILED", "PDF 分段读取返回了无效范围")
        }
        this.onDataRange(begin, new Uint8Array(payload.bytes))
        this.onDataProgress(range.end + 1, range.total)
      })
      .catch((error: unknown) => {
        if (this.aborted || controller.signal.aborted) return
        // PDFDataRangeTransport has no error callback. A zero-length range
        // makes the worker fail closed instead of waiting forever; the reader
        // maps the resulting parser error to a safe retryable state.
        void error
        this.onDataRange(begin, new Uint8Array())
      })
      .finally(() => {
        this.parentSignal?.removeEventListener("abort", abortFromParent)
        this.controllers.delete(controller)
      })
  }

  abort(): void {
    if (this.aborted) return
    this.aborted = true
    for (const controller of this.controllers) controller.abort()
    this.controllers.clear()
  }
}

/**
 * Load a PDF from bytes already held by the caller. The loader deliberately does
 * not accept URL/path sources, keeping resource authorization outside this feature.
 */
export async function loadPdfDocument(
  source: PdfDocumentSource,
  options: PdfDocumentLoadOptions = {},
): Promise<PDFDocumentProxy> {
  if (options.signal?.aborted) throw createCancelledError()
  configurePdfWorker()

  const isBytesSource = source instanceof ArrayBuffer
  if (isBytesSource) assertPdfInput(source, options)
  else assertPdfSessionSource(source)

  const rangeTransport = isBytesSource
    ? null
    : new SessionPdfRangeTransport(source.totalBytes, source.contentUri, source.initialData, options.signal)
  let loadingTask: PDFDocumentLoadingTask | null = null
  let abortHandler: (() => void) | null = null
  let documentLoaded = false
  try {
    loadingTask = isBytesSource
      ? getDocument({ data: new Uint8Array(source) })
      : getDocument({
          range: rangeTransport ?? undefined,
          disableStream: true,
          disableAutoFetch: true,
          rangeChunkSize: PDF_RANGE_CHUNK_SIZE,
        })
    abortHandler = () => {
      void loadingTask?.destroy()
    }
    options.signal?.addEventListener("abort", abortHandler, { once: true })
    const document = await loadingTask.promise
    documentLoaded = true
    if (options.signal?.aborted) {
      await document.destroy()
      throw createCancelledError()
    }
    return document
  } catch (error: unknown) {
    if (options.signal?.aborted) throw createCancelledError()
    if (error instanceof PdfReaderError) throw error
    throw new PdfReaderError("PDF_LOAD_FAILED", "Unable to open this PDF")
  } finally {
    if (abortHandler) options.signal?.removeEventListener("abort", abortHandler)
    if (!documentLoaded || options.signal?.aborted) rangeTransport?.abort()
  }
}

export async function destroyPdfDocument(document: PDFDocumentProxy | null | undefined): Promise<void> {
  if (!document) return
  await document.destroy()
}
