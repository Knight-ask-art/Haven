export function videoSecondsToMilliseconds(seconds: number): number {
  if (!Number.isFinite(seconds)) return 0
  return Math.max(0, Math.round(seconds * 1000))
}
