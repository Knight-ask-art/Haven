// 单篇阅读能力重查的在途登记表。
//
// 页面按「代」重新读取整棵树（切换作品身份、重建层级）：新一代开始时，上一代发出的
// 重查可能还没回来。登记表因此按代记账，而不是一个裸的「在途 ID 集合」——旧代请求
// 收尾时只能解除它自己那一代的登记，绝不能顺手抹掉新代对同一篇的登记，否则新代仍在
// 途的重查会被当成「可以再次发起」，同一篇会被重复请求。
//
// 同一代内同一篇仍然只登记一次（去重），这正是登记表存在的理由。

/** 一次在途登记的释放句柄：只有登记它的那一代能解除它。 */
export type CapabilityRequestRelease = () => void

export interface ArticleCapabilityRequestRegistry {
  /**
   * 为 `mediaItemId` 在 `generation` 这一代登记一次重查。
   *
   * 同一代内同一篇已有在途请求时返回 `null`（调用方据此跳过重复请求）；否则返回
   * 释放句柄，调用方必须在这次请求收尾时（无论成功或失败）调用它。
   */
  begin(mediaItemId: string, generation: number): CapabilityRequestRelease | null
}

export function createArticleCapabilityRequestRegistry(): ArticleCapabilityRequestRegistry {
  // 每一篇最多只保留一条登记：更新的一代会覆盖旧一代的登记，因此条目数不超过文章数。
  const inFlight = new Map<string, { generation: number }>()

  return {
    begin(mediaItemId, generation) {
      const existing = inFlight.get(mediaItemId)
      if (existing !== undefined && existing.generation === generation) return null
      const lease = { generation }
      inFlight.set(mediaItemId, lease)
      return () => {
        // 按登记对象本身比对：这一篇可能已经被更新的一代重新登记过，此时旧句柄必须
        // 什么都不做，否则会解除新代仍然在途的标记。
        if (inFlight.get(mediaItemId) === lease) inFlight.delete(mediaItemId)
      }
    },
  }
}
