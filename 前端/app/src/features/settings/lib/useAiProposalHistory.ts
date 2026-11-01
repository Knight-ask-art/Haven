import { useCallback, useEffect, useRef, useState } from "react"
import { toHavenError } from "@/lib/ipc/errors"
import type { AgentSettingsProposalGetResultDto } from "@/lib/ipc/generated/wire"
import { listAgentProposals, type AgentProposalHistoryClient } from "../ipc/agent-proposal-gateway"

/** History is reread from its backend owner; this hook keeps only request/pagination state. */
export function useAiProposalHistory(
  client: AgentProposalHistoryClient | null,
  enabled: boolean,
  onSnapshot: (items: AgentSettingsProposalGetResultDto[], append: boolean) => void,
) {
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [nextCursor, setNextCursor] = useState<string | null>(null)
  const [settled, setSettled] = useState(false)
  const requestId = useRef(0)
  const cursorRef = useRef<string | null>(null)
  const available = typeof client?.agentSettingsProposalList === "function"

  const invalidate = useCallback(() => { requestId.current += 1; setLoading(false); setSettled(false) }, [])
  const read = useCallback(async (before: string | null) => {
    if (!client?.agentSettingsProposalList) return
    const id = ++requestId.current
    setLoading(true)
    setError(null)
    if (before === null) {
      cursorRef.current = null
      setNextCursor(null)
    }
    try {
      const page = await listAgentProposals(client, before)
      if (id !== requestId.current) return
      cursorRef.current = page.nextCursor
      setNextCursor(page.nextCursor)
      onSnapshot(page.items, before !== null)
    } catch (failure) {
      if (id === requestId.current) setError(toHavenError(failure).message)
    } finally {
      if (id === requestId.current) { setLoading(false); setSettled(true) }
    }
  }, [client, onSnapshot])
  const reload = useCallback(() => read(null), [read])
  const loadMore = useCallback(() => {
    if (cursorRef.current !== null) return read(cursorRef.current)
  }, [read])

  useEffect(() => {
    if (enabled && available) void reload()
    else { setSettled(false); setLoading(false) }
    return () => { requestId.current += 1 }
  }, [enabled, available, reload])

  return { available, loading: enabled && available && (!settled || loading), error, nextCursor, reload, loadMore, invalidate }
}
