// Pure helpers for client-side table sorts, encoded in the `?sort=` URL param
// (or local state for modal/drawer tables). Kept dependency-light and
// side-effect free so both the table components and unit tests can share them.

import type { SortingState } from "@tanstack/react-table";
import type { CaseStatus } from "@/api";

/**
 * Decode the `?sort=` param into a table `SortingState`. A single column is
 * carried: `?sort=<col>` ascending, `?sort=-<col>` descending. Absent, blank,
 * or malformed input (including a bare `-`) yields an empty state (no sort).
 */
export function parseSort(param: string | null): SortingState {
  const raw = param?.trim();
  if (!raw) return [];
  const desc = raw.startsWith("-");
  const id = (desc ? raw.slice(1) : raw).trim();
  if (!id) return [];
  return [{ id, desc }];
}

/**
 * Encode a table `SortingState` back into the `?sort=` param value, or `null`
 * to clear it. Only the primary (first) column is represented — the grid is
 * single-column sorted.
 */
export function serializeSort(sorting: SortingState): string | null {
  const first = sorting[0];
  if (!first) return null;
  return first.desc ? `-${first.id}` : first.id;
}

/**
 * Advance a single-column `SortingState` through the asc → desc → clear cycle
 * for a clicked column (or desc → asc → clear with `descFirst`). Clicking a
 * different column starts its cycle fresh — the same behaviour react-table's
 * toggle handler gives the case grid.
 */
export function cycleSort(
  sorting: SortingState,
  id: string,
  descFirst = false,
): SortingState {
  const current = sorting[0];
  if (!current || current.id !== id) return [{ id, desc: descFirst }];
  if (current.desc === descFirst) return [{ id, desc: !descFirst }];
  return [];
}

/** What a column accessor may yield for a row. */
export type SortValue = number | string | null | undefined;
export type SortAccessor<T> = (row: T) => SortValue;

/**
 * Ascending comparator over non-null sort values: numbers numerically,
 * strings via localeCompare, and a mixed pair (a column should not produce
 * one) by string form so the order is at least total.
 */
export function compareValues(
  a: number | string,
  b: number | string,
): number {
  if (typeof a === "number" && typeof b === "number") return a - b;
  if (typeof a === "string" && typeof b === "string") return a.localeCompare(b);
  return String(a).localeCompare(String(b));
}

/**
 * Copy-sort `rows` by the single sorted column, looking the column's accessor
 * up in `fields` — whose keys are therefore the single source of truth for
 * which columns sort. An empty state or an unknown column id returns `rows`
 * unchanged (same reference), which is what makes "clear sort = the table's
 * default order" free at every call site.
 *
 * Rows whose accessor yields null/undefined sort LAST in both directions —
 * the flip below only applies to comparable pairs, so "sort by cost desc"
 * never floats the cost-less rows to the top.
 */
export function sortRows<T>(
  rows: readonly T[],
  sorting: SortingState,
  fields: Record<string, SortAccessor<T>>,
): readonly T[] {
  const primary = sorting[0];
  if (!primary) return rows;
  const accessor = fields[primary.id];
  if (!accessor) return rows;
  const dir = primary.desc ? -1 : 1;
  // `Array.prototype.sort` is stable, so equal-keyed rows keep their
  // incoming (default) order.
  return [...rows].sort((ra, rb) => {
    const a = accessor(ra);
    const b = accessor(rb);
    const aMissing = a === null || a === undefined;
    const bMissing = b === null || b === undefined;
    if (aMissing && bMissing) return 0;
    if (aMissing) return 1;
    if (bMissing) return -1;
    return compareValues(a, b) * dir;
  });
}

/**
 * Rank for the Status column's sort: fail > error > pass > skip. Descending
 * order therefore floats failures (then errors) to the top of the grid — the
 * cases an operator most wants to see first.
 */
export const STATUS_RANK: Record<CaseStatus, number> = {
  fail: 5,
  // Gate-failing, so it sorts beside fail; its own rank keeps the two
  // separable in a sorted grid.
  xpass: 4,
  error: 3,
  pass: 2,
  // Expected and unremarkable: below pass, above skip (it was graded).
  xfail: 1,
  skip: 0,
};

/**
 * Ascending comparator by status rank
 * (skip < xfail < pass < error < xpass < fail). react-table
 * reverses it for a descending sort, surfacing failures first.
 */
export function compareStatus(a: CaseStatus, b: CaseStatus): number {
  return STATUS_RANK[a] - STATUS_RANK[b];
}
