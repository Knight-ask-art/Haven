import { primaryActionRoute } from "@/features/media/lib/primary-action-route"
import { getMediaItemDownloadInfo } from "@/features/downloads/ipc/download-gateway"
import type { FootprintActionCard } from "../ipc/footprints-gateway"

export type FootprintOpenResult =
  | { kind: "open"; route: string }
  | { kind: "message"; message: string }

export async function resolveFootprintOpen(
  card: Pick<FootprintActionCard, "primaryAction" | "mediaItemId">,
): Promise<FootprintOpenResult> {
  const route = primaryActionRoute(card.primaryAction)
  if (!route) return { kind: "message", message: "当前内容暂不可打开" }
  if (card.primaryAction?.kind === "open_edition") return { kind: "open", route }
  const mediaItemId = card.primaryAction?.mediaItemId ?? card.mediaItemId
  if (!mediaItemId) return { kind: "message", message: "当前内容缺少可用版本" }
  const info = await getMediaItemDownloadInfo(mediaItemId)
  if (info.canOnlineRead || info.hasOfflineResource) return { kind: "open", route }
  return { kind: "message", message: info.canDownload ? "该内容需要下载后阅读" : "当前内容暂不可用" }
}
