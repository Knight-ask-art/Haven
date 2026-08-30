export type WorkDetailState = "data" | "loading" | "retryable_error" | "terminal_error"

export function canConsumeDetail(production: boolean, state: WorkDetailState): boolean {
  return !production || state === "data"
}
