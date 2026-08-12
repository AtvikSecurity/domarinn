// One sort-state shape for the simple (non-react-table) tables, in two
// storages. Page-level tables keep it in `?sort=` — sort order is shareable
// meaning, per the doctrine in `tableColumns.ts`. Tables inside modals and
// drawers keep it in component state instead: their host pages own `?sort=`
// for their own tables, and an overlay's ordering is not something a shared
// URL should reproduce.

import { useCallback, useMemo, useState } from "react";
import { useSearchParams } from "react-router";
import type { SortingState } from "@tanstack/react-table";
import { mergeParams } from "@/lib/filters";
import { cycleSort, parseSort, serializeSort } from "@/lib/sort";

export interface TableSort {
  sorting: SortingState;
  /** The active direction for a column header's `aria-sort` / arrow. */
  sortFor(id: string): false | "asc" | "desc";
  /** Header click: cycle this column asc → desc → cleared. */
  toggle(id: string): void;
}

function sortForIn(sorting: SortingState, id: string): false | "asc" | "desc" {
  const current = sorting[0];
  if (!current || current.id !== id) return false;
  return current.desc ? "desc" : "asc";
}

/** URL-backed sort (`?sort=col` / `?sort=-col`) for page-level tables. */
export function useSortParam(): TableSort {
  const [params, setParams] = useSearchParams();
  const sorting = useMemo(() => parseSort(params.get("sort")), [params]);
  const toggle = useCallback(
    (id: string) => {
      // Read the param off `prev`, not the captured `params`: two clicks in
      // one frame must compound, and `replace` keeps the cycle out of history.
      setParams(
        (prev) =>
          mergeParams(prev, {
            sort: serializeSort(cycleSort(parseSort(prev.get("sort")), id)),
          }),
        { replace: true },
      );
    },
    [setParams],
  );
  return useMemo(
    () => ({ sorting, sortFor: (id) => sortForIn(sorting, id), toggle }),
    [sorting, toggle],
  );
}

/** Component-state sort, same shape, for tables inside modals/drawers. */
export function useLocalSort(): TableSort {
  const [sorting, setSorting] = useState<SortingState>([]);
  const toggle = useCallback((id: string) => {
    setSorting((prev) => cycleSort(prev, id));
  }, []);
  return useMemo(
    () => ({ sorting, sortFor: (id) => sortForIn(sorting, id), toggle }),
    [sorting, toggle],
  );
}
