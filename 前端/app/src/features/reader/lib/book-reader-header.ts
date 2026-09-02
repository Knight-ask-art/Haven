export type BookReaderContentStatus =
  | "idle"
  | "loading"
  | "ready"
  | "pdf_ready"
  | "empty"
  | "retryable_error"
  | "terminal_error"

export interface BookReaderHeaderContextInput {
  currentChapterTitle?: string | null
  contentStatus: BookReaderContentStatus
  contentErrorMessage?: string | null
}

/**
 * Keep the reader header truthful while content is being prepared or fails.
 * A missing chapter is not evidence that the reader is still loading: after
 * a terminal content error the header must stop saying "正在准备内容".
 */
export function resolveBookReaderHeaderContext({
  currentChapterTitle,
  contentStatus,
  contentErrorMessage,
}: BookReaderHeaderContextInput): string {
  if ((contentStatus === "retryable_error" || contentStatus === "terminal_error") && contentErrorMessage?.trim()) {
    return `读取失败 · ${contentErrorMessage.trim()}`
  }
  if (contentStatus === "loading") return "正在读取文本…"
  return currentChapterTitle?.trim() || "正在准备内容"
}
