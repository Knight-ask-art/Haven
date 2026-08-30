import {
  GlobalWorkerOptions,
  getDocument,
} from "pdfjs-dist/legacy/build/pdf.mjs"
import type { PDFDocumentLoadingTask, PDFDocumentProxy } from "pdfjs-dist/legacy/build/pdf.mjs"
import pdfWorkerUrl from "pdfjs-dist/legacy/build/pdf.worker.min.mjs?url"
import { isPdfMimeType } from "./pdf-reader-state"

export const MAX_PDF_BYTES = 32 * 1024 * 1024

export type PdfReaderErrorCode =
  | "PDF_INVALID_INPUT"
  | "PDF_UNSUPPORTED_MIME"
  | "PDF_TOO_LARGE"
  | "PDF_CANCELLED"
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

/**
 * Load a PDF from bytes already held by the caller. The loader deliberately does
 * not accept URL/path sources, keeping resource authorization outside this feature.
 */
export async function loadPdfDocument(
  bytes: ArrayBuffer,
  options: PdfDocumentLoadOptions = {},
): Promise<PDFDocumentProxy> {
  assertPdfInput(bytes, options)
  if (options.signal?.aborted) throw createCancelledError()
  configurePdfWorker()

  const data = new Uint8Array(bytes)
  let loadingTask: PDFDocumentLoadingTask | null = null
  let abortHandler: (() => void) | null = null
  try {
    loadingTask = getDocument({ data })
    abortHandler = () => {
      void loadingTask?.destroy()
    }
    options.signal?.addEventListener("abort", abortHandler, { once: true })
    const document = await loadingTask.promise
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
  }
}

export async function destroyPdfDocument(document: PDFDocumentProxy | null | undefined): Promise<void> {
  if (!document) return
  await document.destroy()
}
