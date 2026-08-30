import { Link } from "react-router"
import type { ShelfDto } from "@/lib/ipc/generated/wire"
import { ContentShelf } from "./ContentShelf"
import { workCardToMediaCard } from "../ipc/home-gateway"

const SHELF_TITLES: Record<string, string> = {
  "shelf.favorites": "收藏",
}

function shelfTitle(titleKey: string): string {
  return SHELF_TITLES[titleKey] ?? "内容架"
}

export function HomeShelves({ shelves }: { shelves: ShelfDto[] }) {
  const visibleShelves = shelves.filter((shelf) => shelf.preview.length > 0)
  if (visibleShelves.length === 0) return null

  return visibleShelves.map((shelf) => (
    <ContentShelf
      key={shelf.shelfId}
      title={shelfTitle(shelf.titleKey)}
      items={shelf.preview.map(workCardToMediaCard)}
      className="pt-0"
      actionRight={shelf.shelfId === "shelf-favorites" ? (
        <Link to="/footprints" className="text-sm font-semibold text-muted-foreground transition-colors hover:text-foreground">
          查看更多
        </Link>
      ) : undefined}
    />
  ))
}
