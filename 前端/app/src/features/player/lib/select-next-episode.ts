import type { EpisodeItem } from "../components/EpisodeDrawer"

/**
 * 选择当前播放列表中的下一项。
 *
 * `episodes` 由 PlayerPage 按 canonical 顺序提供；抽屉的正序/倒序只是展示
 * 投影，自动播放不能因为用户切换了展示排序而反向跳集。未知当前项或已到
 * 列表末尾时返回 null，由调用方保持播放器结束状态。
 */
export function selectNextEpisodeId(
  episodes: readonly EpisodeItem[] | null | undefined,
  currentEpisodeId: string,
): string | null {
  if (!episodes || episodes.length === 0) return null
  const currentIndex = episodes.findIndex((episode) => episode.id === currentEpisodeId)
  if (currentIndex < 0 || currentIndex + 1 >= episodes.length) return null
  const nextId = episodes[currentIndex + 1]?.id
  return nextId && nextId !== currentEpisodeId ? nextId : null
}
