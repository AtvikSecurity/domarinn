/**
 * The rows either side of the open one, for a drawer's prev/next stepping.
 *
 * Both grids are paginated: `cases` and `entries` hold the pages fetched so
 * far, not the whole result set. So everything here is scoped to *loaded* rows,
 * deliberately:
 *
 * - `nextKey` is undefined at the last loaded row rather than firing
 *   `fetchNextPage`. Stepping is a keyboard-speed action and a fetch is not;
 *   a chevron that sometimes moves and sometimes spins is worse than one that
 *   stops at a boundary the grid itself can be scrolled past.
 * - `position` counts loaded rows, not the run's `case_count`. Reporting
 *   "12 of 4,318" while stepping can only reach 40 of them states something
 *   untrue about what the control does.
 *
 * A selection that is not among the loaded rows — a deep link into a filtered
 * page, or a row scrolled past the loaded window — yields no neighbours and no
 * position, which renders as a drawer with its nav disabled rather than one
 * claiming a place it cannot navigate from.
 *
 * Pure, so the boundary rules are testable without a grid or a DOM.
 */

export interface ListNeighbors {
  /** Key of the row before the selection, or undefined at the first row. */
  prevKey: string | undefined;
  /** Key of the row after the selection, or undefined at the last loaded row. */
  nextKey: string | undefined;
  /**
   * 1-based `{ index, total }` over loaded rows, or undefined when the
   * selection is not among them. Shaped to pass straight to `DetailDrawer`.
   */
  position: { index: number; total: number } | undefined;
}

const NONE: ListNeighbors = {
  prevKey: undefined,
  nextKey: undefined,
  position: undefined,
};

/** Locate `selectedKey` among `items` and describe its neighbours. */
export function listNeighbors<T>(
  items: readonly T[],
  selectedKey: string | undefined,
  keyOf: (item: T) => string,
): ListNeighbors {
  if (selectedKey === undefined) return NONE;
  const at = items.findIndex((item) => keyOf(item) === selectedKey);
  if (at === -1) return NONE;
  const prev = at > 0 ? items[at - 1] : undefined;
  const next = at < items.length - 1 ? items[at + 1] : undefined;
  return {
    prevKey: prev === undefined ? undefined : keyOf(prev),
    nextKey: next === undefined ? undefined : keyOf(next),
    position: { index: at + 1, total: items.length },
  };
}
