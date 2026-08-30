import { createContext, useContext } from "react"

export type NoticeKind = "info" | "success" | "warning" | "error" | "announcement" | "progress" | "confirm"

export interface NoticeAction {
  label: string
  onClick: () => void | Promise<void>
  /** 默认点击后关闭通知；设置为 false 可在动作完成后继续保留。 */
  dismiss?: boolean
}

export interface NoticeInput {
  kind?: NoticeKind
  title?: string
  message: string
  code?: string | null
  retryable?: boolean
  action?: NoticeAction
  dedupeKey?: string
  /** null 表示持久显示，直到用户关闭或调用 dismiss。 */
  durationMs?: number | null
  /** 0..1；仅 progress 通知显示进度条。 */
  progress?: number | null
  confirmLabel?: string
  cancelLabel?: string
}

export interface Notice extends Omit<NoticeInput, "durationMs"> {
  id: string
  kind: NoticeKind
  title: string
  createdAt: number
  durationMs: number | null
}

export interface NoticeCenterApi {
  notices: Notice[]
  push: (input: NoticeInput) => string
  dismiss: (id: string) => void
  clear: () => void
  resolveConfirm: (id: string, confirmed: boolean) => void
  confirm: (input: Omit<NoticeInput, "kind" | "action"> & {
    confirmLabel?: string
    cancelLabel?: string
  }) => Promise<boolean>
}

const noopApi: NoticeCenterApi = {
  notices: [],
  push: () => "",
  dismiss: () => undefined,
  clear: () => undefined,
  resolveConfirm: () => undefined,
  confirm: async () => false,
}

export const NoticeContext = createContext<NoticeCenterApi>(noopApi)

export function useNotice(): NoticeCenterApi {
  return useContext(NoticeContext)
}
