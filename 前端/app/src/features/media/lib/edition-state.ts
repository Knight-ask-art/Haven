import type { HavenError } from "@/lib/ipc/errors"

export type EditionListState = "loading" | "data" | "empty" | "retryable_error" | "terminal_error"

export function getEditionListState(
  production: boolean,
  loading: boolean,
  items: readonly unknown[] | null,
  error: HavenError | null,
): EditionListState {
  if (!production) return "data"
  if (loading) return "loading"
  if (error?.dto.retryable) return "retryable_error"
  if (error) return "terminal_error"
  if (items !== null && items.length === 0) return "empty"
  return "data"
}

export function canConsumeEdition(
  production: boolean,
  state: EditionListState,
  hasPrimaryAction: boolean,
): boolean {
  return !production || (state === "data" && hasPrimaryAction)
}
