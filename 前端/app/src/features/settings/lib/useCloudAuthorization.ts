import { useEffect, useRef, useState } from "react"
import type { CloudAccountDto, CloudConnectStatusDto } from "@/lib/ipc/generated/wire"
import { toHavenError, type HavenError } from "@/lib/ipc/errors"
import { cloudStorageGateway } from "../ipc/cloud-storage-gateway"

/** 单次授权会话；销毁或取消后，迟到 begin/poll/complete 不更新新页面。 */
export function useCloudAuthorization(accountId: string | undefined, onAuthorized: (account: CloudAccountDto) => void) {
  const [status, setStatus] = useState<CloudConnectStatusDto | "idle" | "starting">("idle")
  const [error, setError] = useState<HavenError | null>(null)
  const epoch = useRef(0)
  const attempt = useRef<string | null>(null)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const active = useRef(false)
  const beginLock = useRef(false)
  const cancelLock = useRef(false)
  const callback = useRef(onAuthorized)
  callback.current = onAuthorized

  useEffect(() => {
    active.current = true
    return () => {
      active.current = false; epoch.current++
      if (timer.current !== null) clearTimeout(timer.current)
      const id = attempt.current; attempt.current = null
      if (id) void cloudStorageGateway.cancel(id).catch(() => { /* TTL 为退出时取消失败兜底；不写假成功状态。 */ })
    }
  }, [])

  const begin = async () => {
    if (attempt.current || beginLock.current || cancelLock.current) return
    beginLock.current = true
    const current = ++epoch.current
    setError(null); setStatus("starting")
    try {
      const result = await cloudStorageGateway.begin(accountId ? { accountId } : {})
      if (!active.current || current !== epoch.current) {
        await cloudStorageGateway.cancel(result.attemptId)
        return
      }
      attempt.current = result.attemptId
      setStatus("pending")
      const poll = async () => {
        if (!active.current || current !== epoch.current) return
        try {
          const next = await cloudStorageGateway.poll(result.attemptId)
          if (!active.current || current !== epoch.current) return
          setStatus(next.status)
          if (next.status === "authorized") {
            setStatus("completing")
            const account = await cloudStorageGateway.complete(result.attemptId)
            if (!active.current || current !== epoch.current) return
            attempt.current = null
            setStatus("completed")
            callback.current(account)
          } else if (next.status === "pending" || next.status === "completing") {
            timer.current = setTimeout(() => void poll(), 1200)
          } else {
            attempt.current = null
          }
        } catch (failure) {
          if (!active.current || current !== epoch.current) return
          const id = attempt.current; attempt.current = null
          setError(toHavenError(failure)); setStatus("failed")
          if (id) void cloudStorageGateway.cancel(id).catch(() => {})
        }
      }
      void poll()
    } catch (failure) {
      if (active.current && current === epoch.current) { setError(toHavenError(failure)); setStatus("failed") }
    } finally {
      beginLock.current = false
    }
  }

  const cancel = async () => {
    if (cancelLock.current) return
    const current = ++epoch.current
    if (timer.current !== null) clearTimeout(timer.current)
    const id = attempt.current; attempt.current = null
    if (!id) { setStatus("cancelled"); return }
    cancelLock.current = true
    try {
      const final = await cloudStorageGateway.cancel(id)
      if (active.current && current === epoch.current) {
        setStatus(final)
        if (final === "completed") setError(toHavenError({ code: "CLOUD_OAUTH_ALREADY_COMPLETED", userMessage: "授权已完成，请返回存储重新读取账户状态。", retryable: false }))
      }
    } catch (failure) {
      if (active.current && current === epoch.current) { setError(toHavenError(failure)); setStatus("failed") }
    } finally {
      cancelLock.current = false
    }
  }
  const pending = status === "starting" || status === "pending" || status === "completing" || status === "authorized" || cancelLock.current
  return { status, error, pending, begin, cancel }
}
