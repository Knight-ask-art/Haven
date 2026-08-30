import { REPRESENTATIVE_ITEMS } from "@/features/library/components/LibraryGrid"
import type { LibraryMediaItemData } from "@/features/library/components/MediaItem"

export const STORAGE_KEYS = {
  favorite: (id: string) => `haven:favorite:${id}`,
  download: (id: string) => `haven:download:${id}`,
  marker: (mode: string, mediaItemId: string) => `haven:marker:${mode}:${mediaItemId}`,
  history: (id: string) => `haven:history:${id}`,
  hiddenHistory: "haven:hidden-history",
  searchHistory: "haven:search-history",
} as const

export function getStoredFavoriteIds(): string[] {
  const ids: string[] = []
  for (let i = 0; i < localStorage.length; i++) {
    const key = localStorage.key(i)
    if (key?.startsWith("haven:favorite:") && localStorage.getItem(key) === "1") {
      ids.push(key.slice("haven:favorite:".length))
    }
  }
  return ids
}

export function getStoredMarkers(): Array<{ mode: string; mediaItemId: string }> {
  const markers: Array<{ mode: string; mediaItemId: string }> = []
  for (let i = 0; i < localStorage.length; i++) {
    const key = localStorage.key(i)
    if (key?.startsWith("haven:marker:") && localStorage.getItem(key) === "1") {
      const rest = key.slice("haven:marker:".length)
      const sep = rest.indexOf(":")
      if (sep > 0) {
        markers.push({ mode: rest.slice(0, sep), mediaItemId: rest.slice(sep + 1) })
      }
    }
  }
  return markers
}

export function getStoredDownloadIds(status: "queued" | "downloaded"): string[] {
  const ids: string[] = []
  for (let i = 0; i < localStorage.length; i++) {
    const key = localStorage.key(i)
    if (key?.startsWith("haven:download:") && localStorage.getItem(key) === status) {
      ids.push(key.slice("haven:download:".length))
    }
  }
  return ids
}

export function removeStoredDownload(id: string) {
  localStorage.removeItem(STORAGE_KEYS.download(id))
}

export function removeStoredMarker(mode: string, mediaItemId: string) {
  localStorage.removeItem(STORAGE_KEYS.marker(mode, mediaItemId))
}

export function recordHistory(id: string) {
  localStorage.setItem(STORAGE_KEYS.history(id), String(Date.now()))
}

export function getCatalogItem(id: string): LibraryMediaItemData | undefined {
  return REPRESENTATIVE_ITEMS.find((item) => item.id === id)
}

export function getCatalogItems(ids: string[]): LibraryMediaItemData[] {
  return ids
    .map((id) => getCatalogItem(id))
    .filter((item): item is LibraryMediaItemData => Boolean(item))
}
