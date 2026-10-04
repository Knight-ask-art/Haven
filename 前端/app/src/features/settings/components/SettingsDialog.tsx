import { useEffect, useId, useLayoutEffect, useRef } from "react"
import type { ReactNode } from "react"
import { createPortal } from "react-dom"
import { X } from "lucide-react"
import "./settings-dialog.css"

/** 设置管理的大表单窗口：不持有业务数据，统一主题、视窗边界与键盘焦点。 */
export function SettingsDialog({ title, titleId, description, closeLabel = "取消", busy = false, initialFocusSelector, onClose, children }: {
  title: string
  titleId?: string
  description?: string
  closeLabel?: string
  busy?: boolean
  initialFocusSelector?: string
  onClose: () => void
  children: ReactNode
}) {
  const generatedId = useId()
  const panelRef = useRef<HTMLDialogElement>(null)
  const initialFocusRef = useRef(initialFocusSelector)
  const closeRef = useRef(onClose)
  const busyRef = useRef(busy)
  useEffect(() => { closeRef.current = onClose; busyRef.current = busy }, [onClose, busy])

  useLayoutEffect(() => {
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null
    const panel = panelRef.current
    // 原生模态窗口进入 top layer，并由浏览器隔离背景的焦点、点击和辅助技术访问。
    panel?.showModal()
    // 初始焦点归模态窗口所有；每次挂载都在 showModal 之后定位，包含 StrictMode 重放。
    const initialFocus = initialFocusRef.current ? panel?.querySelector<HTMLElement>(initialFocusRef.current) : null
    const focusTarget = initialFocus ?? panel
    focusTarget?.focus({ preventScroll: true })
    initialFocus?.scrollIntoView?.({ block: "center", behavior: "auto" })
    const handleKey = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
        event.preventDefault()
        event.stopPropagation()
        return
      }
      if (event.key !== "Tab" || !panel) return
      const controls = Array.from(panel.querySelectorAll<HTMLElement>(
        'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), a[href], [tabindex="0"]',
      )).filter((element) => !element.closest('[hidden], [aria-hidden="true"]') && (element.checkVisibility?.() ?? true))
      const first = controls[0]
      const last = controls[controls.length - 1]
      if (!first) { event.preventDefault(); panel.focus(); return }
      if (event.shiftKey && (document.activeElement === first || document.activeElement === panel)) {
        event.preventDefault(); last.focus()
      } else if (!event.shiftKey && (document.activeElement === last || document.activeElement === panel)) {
        event.preventDefault(); first.focus()
      }
    }
    document.addEventListener("keydown", handleKey, true)
    return () => {
      document.removeEventListener("keydown", handleKey, true)
      panel?.close()
      if (previousFocus?.isConnected) previousFocus.focus()
    }
  }, [])

  return createPortal(
    <dialog ref={panelRef} aria-modal="true" aria-labelledby={titleId ?? generatedId}
      aria-describedby={description ? `${generatedId}-description` : undefined} tabIndex={-1}
      className="settings-dialog-backdrop" onCancel={(event) => {
        event.preventDefault()
        if (!busyRef.current) closeRef.current()
      }} onMouseDown={(event) => {
      if (event.target === event.currentTarget && !busy) onClose()
    }}>
      <section className="settings-dialog settings-page">
        <header className="settings-dialog__header">
          <div>
            <h2 id={titleId ?? generatedId}>{title}</h2>
            {description && <p id={`${generatedId}-description`}>{description}</p>}
          </div>
          <button type="button" aria-label={closeLabel} onClick={onClose} disabled={busy} className="settings-dialog__close">
            <X size={16} aria-hidden="true" />{closeLabel === "取消" ? "取消" : "关闭"}
          </button>
        </header>
        <div className="settings-dialog__body">{children}</div>
      </section>
    </dialog>, document.body,
  )
}
