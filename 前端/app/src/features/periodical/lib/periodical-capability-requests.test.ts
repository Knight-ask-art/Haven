import { describe, expect, it } from "vitest"
import { createArticleCapabilityRequestRegistry } from "./periodical-capability-requests"

const MEDIA_A = "66666666-6666-4666-8666-666666666601"
const MEDIA_B = "66666666-6666-4666-8666-666666666602"

describe("createArticleCapabilityRequestRegistry", () => {
  it("dedupes a re-check of the same article within one generation", () => {
    const registry = createArticleCapabilityRequestRegistry()

    const release = registry.begin(MEDIA_A, 1)
    expect(release).not.toBeNull()
    expect(registry.begin(MEDIA_A, 1)).toBeNull()

    // 收尾之后同一代可以再次重查：去重的是在途请求，不是这一篇本身。
    release?.()
    expect(registry.begin(MEDIA_A, 1)).not.toBeNull()
  })

  it("keeps different articles independent", () => {
    const registry = createArticleCapabilityRequestRegistry()

    expect(registry.begin(MEDIA_A, 1)).not.toBeNull()
    expect(registry.begin(MEDIA_B, 1)).not.toBeNull()
    expect(registry.begin(MEDIA_A, 1)).toBeNull()
    expect(registry.begin(MEDIA_B, 1)).toBeNull()
  })

  it("does not let an outstanding marker from an old generation block a rebuilt tree", () => {
    const registry = createArticleCapabilityRequestRegistry()

    registry.begin(MEDIA_A, 1)
    // 切换作品身份 / 重建层级后，新一代必须能重新登记同一篇，而不是被旧代的在途标记挡住。
    expect(registry.begin(MEDIA_A, 2)).not.toBeNull()
  })

  it("keeps the new generation deduped when the old request finishes afterwards", () => {
    const registry = createArticleCapabilityRequestRegistry()

    const staleRelease = registry.begin(MEDIA_A, 1)
    const currentRelease = registry.begin(MEDIA_A, 2)

    // 旧代请求此刻才收尾：它只能解除自己那一代的登记。
    staleRelease?.()

    // 新代的重查仍在途，同一篇必须继续去重。
    expect(registry.begin(MEDIA_A, 2)).toBeNull()
    // 只有登记它的那一代能解除它。
    currentRelease?.()
    expect(registry.begin(MEDIA_A, 2)).not.toBeNull()
  })

  it("does not resurrect a released marker when a stale handle fires late", () => {
    const registry = createArticleCapabilityRequestRegistry()

    const staleRelease = registry.begin(MEDIA_A, 1)
    const currentRelease = registry.begin(MEDIA_A, 2)
    currentRelease?.()

    staleRelease?.()
    // 已经被新代释放过的登记不能被旧句柄重新激活。
    expect(registry.begin(MEDIA_A, 2)).not.toBeNull()
  })

  it("releases only the generation that owns the marker", () => {
    const registry = createArticleCapabilityRequestRegistry()

    const firstRelease = registry.begin(MEDIA_A, 1)
    firstRelease?.()
    const secondRelease = registry.begin(MEDIA_A, 1)
    // 已经释放过的旧句柄再触发一次，不得解除后来那次登记。
    firstRelease?.()
    expect(registry.begin(MEDIA_A, 1)).toBeNull()
    secondRelease?.()
    expect(registry.begin(MEDIA_A, 1)).not.toBeNull()
  })
})
