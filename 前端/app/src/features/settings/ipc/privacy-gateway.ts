import type { CacheClearResultDto } from "@/lib/ipc/generated/wire"
import { getHavenClient } from "@/lib/ipc/runtime"
import type { HavenClient } from "@/lib/ipc/client"

/**
 * Privacy 页面只暴露两个已经冻结边界的本地数据操作：
 * - 搜索历史：只清理 search_history 表；
 * - Artwork Cache：只清理技术图片缓存，不触碰 Offline Resource 或业务事实。
 *
 * 页面不直接依赖 Tauri；调用统一经由 Typed HavenClient。
 */
export async function clearSearchHistory(client: HavenClient = getHavenClient()): Promise<void> {
  await client.searchHistoryClear()
}

export async function clearArtworkCache(
  client: HavenClient = getHavenClient(),
): Promise<CacheClearResultDto> {
  return client.cacheClear("artwork")
}
