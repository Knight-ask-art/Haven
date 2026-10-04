/** Merge a local layout draft onto the latest saved snapshot after a revision conflict. */
type LayoutSetting = { module: string; visible: boolean; size: string }

export interface LayoutRebaseResult<T> {
  modules: T[]
  hasOverlappingChanges: boolean
}

function sameOrder(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((module, index) => module === right[index])
}

export function rebaseLayoutModules<T extends LayoutSetting>(
  base: readonly T[],
  draft: readonly T[],
  latest: readonly T[],
): LayoutRebaseResult<T> {
  const baseByModule = new Map(base.map((entry) => [entry.module, entry]))
  const draftByModule = new Map(draft.map((entry) => [entry.module, entry]))
  const baseOrder = base.map((entry) => entry.module)
  const draftOrder = draft.map((entry) => entry.module)
  const latestOrder = latest.map((entry) => entry.module)
  const localOrderChanged = !sameOrder(draftOrder, baseOrder)
  const latestOrderChanged = !sameOrder(latestOrder, baseOrder)
  let hasOverlappingChanges = localOrderChanged
    && latestOrderChanged
    && !sameOrder(draftOrder, latestOrder)

  const mergedByModule = new Map<string, T>()
  for (const saved of latest) {
    const previous = baseByModule.get(saved.module)
    const local = draftByModule.get(saved.module)
    if (!previous || !local) {
      mergedByModule.set(saved.module, { ...saved })
      continue
    }

    const mergeField = <V extends string | boolean>(baseValue: V, localValue: V, savedValue: V): V => {
      if (localValue === baseValue) return savedValue
      if (savedValue === baseValue || localValue === savedValue) return localValue
      hasOverlappingChanges = true
      // Conflicting edits preserve the user's local draft; the UI will ask them to review it.
      return localValue
    }

    mergedByModule.set(saved.module, {
      ...saved,
      visible: mergeField(previous.visible, local.visible, saved.visible),
      size: mergeField(previous.size, local.size, saved.size),
    })
  }

  const chosenOrder = localOrderChanged ? draftOrder : latestOrder
  const order = [...chosenOrder, ...latestOrder.filter((module) => !chosenOrder.includes(module))]
  const modules = order.flatMap((module) => {
    const entry = mergedByModule.get(module)
    return entry ? [entry] : []
  })

  return { modules, hasOverlappingChanges }
}
