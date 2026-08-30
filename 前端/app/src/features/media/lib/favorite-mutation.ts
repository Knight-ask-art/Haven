/** Resolve a favorite write without allowing a failed optimistic update to stick. */
export async function settleFavoriteMutation(
  previous: boolean,
  save: () => Promise<{ favorite: boolean }>,
): Promise<{ favorite: boolean; error: unknown | null }> {
  try {
    const result = await save()
    return { favorite: result.favorite, error: null }
  } catch (error) {
    return { favorite: previous, error }
  }
}
