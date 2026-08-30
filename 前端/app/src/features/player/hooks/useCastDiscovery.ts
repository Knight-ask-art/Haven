import { useCallback, useEffect, useRef, useState } from "react"
import { discoverCastDevices } from "@/features/cast/ipc/cast-gateway"
import { toHavenError, HavenError } from "@/lib/ipc/errors"
import type { CastDeviceDto } from "@/lib/ipc/generated/wire"

export interface UseCastDiscoveryResult {
  devices: CastDeviceDto[]
  scanning: boolean
  error: HavenError | null
  refresh: () => void
}

export function useCastDiscovery(enabled: boolean, timeoutMs = 5000): UseCastDiscoveryResult {
  const [devices, setDevices] = useState<CastDeviceDto[]>([])
  const [scanning, setScanning] = useState(false)
  const [error, setError] = useState<HavenError | null>(null)
  const genRef = useRef(0)

  const refresh = useCallback(() => {
    if (!enabled) return
    const gen = ++genRef.current
    setScanning(true)
    setError(null)
    void discoverCastDevices(timeoutMs)
      .then((list) => {
        if (gen !== genRef.current) return
        setDevices(list)
        setScanning(false)
      })
      .catch((e: unknown) => {
        if (gen !== genRef.current) return
        setError(toHavenError(e))
        setScanning(false)
      })
  }, [enabled, timeoutMs])

  useEffect(() => {
    if (!enabled) return
    refresh()
    return () => {
      genRef.current += 1
    }
  }, [enabled, refresh])

  return { devices, scanning, error, refresh }
}
