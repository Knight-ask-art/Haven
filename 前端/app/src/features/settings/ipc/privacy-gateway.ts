import type { CacheClearResultDto } from "@/lib/ipc/generated/wire"
import { getHavenClient as getRuntimeHavenClient } from "@/lib/ipc/runtime"
import type { HavenClient } from "@/lib/ipc/client"
import { clearSearchHistory as clearSearchHistoryOwner } from "@/features/search/ipc/search-history-gateway"

/**
 * Privacy 页面只暴露两个已经冻结边界的本地数据操作：
 * - 搜索历史：清理 search_history 表与旧版本遗留的 localStorage 历史；
 * - Artwork Cache：只清理技术图片缓存，不触碰 Offline Resource 或业务事实。
 *
 * 页面不直接依赖 Tauri；调用统一经由 Typed HavenClient。
 *
 * 搜索历史清除必须走搜索历史网关的 owner：旧版本遗留的 `haven:search-history`
 * 与 SQLite 的迁移共用同一队列，只有这里才能保证"清除"之后旧词不会被再次迁移回来。
 * 默认客户端沿用页面原来的写法（调用时再取）；client 参数保留给测试注入，并透传给同一个 owner。
 */
export async function clearSearchHistory(
  client: HavenClient = getRuntimeHavenClient(),
): Promise<void> {
  await clearSearchHistoryOwner(client)
}

export async function clearArtworkCache(
  client: HavenClient = getRuntimeHavenClient(),
): Promise<CacheClearResultDto> {
  return client.cacheClear("artwork")
}
