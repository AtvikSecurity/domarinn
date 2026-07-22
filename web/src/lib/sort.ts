// Pure helpers for the case grid's client-side sort, encoded in the `?sort=`
// URL param. Kept dependency-light and side-effect free so both the table
// component and unit tests can share them.

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
 * Rank for the Status column's sort: fail > error > pass > skip. Descending
 * order therefore floats failures (then errors) to the top of the grid — the
 * cases an operator most wants to see first.
 */
export const STATUS_RANK: Record<CaseStatus, number> = {
  fail: 3,
  error: 2,
  pass: 1,
  skip: 0,
};

/**
 * Ascending comparator by status rank (skip < pass < error < fail). react-table
 * reverses it for a descending sort, surfacing failures first.
 */
export function compareStatus(a: CaseStatus, b: CaseStatus): number {
  return STATUS_RANK[a] - STATUS_RANK[b];
}
