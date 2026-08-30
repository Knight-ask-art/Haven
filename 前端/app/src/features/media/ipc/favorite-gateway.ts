// Favorite Gateway（SLICE-FAVORITE-001：详情页收藏的唯一数据通道，禁止散落 invoke）。
// 乐观更新与回滚由页面负责；本层封装 favoriteSet 调用，并透传事件订阅依赖。

import { getHavenClient, isTauriRuntime } from "@/lib/ipc/runtime"
import { onFavoriteChanged } from "@/lib/ipc/events"
import type { FavoriteSetRequest, FavoriteSetResult } from "@/lib/ipc/generated/wire"

/** 收藏/取消收藏（幂等；结果 revision 与 favorite-changed 事件同源）。 */
export async function setFavorite(request: FavoriteSetRequest): Promise<FavoriteSetResult> {
  return getHavenClient().favoriteSet(request)
}

export { isTauriRuntime, onFavoriteChanged }
