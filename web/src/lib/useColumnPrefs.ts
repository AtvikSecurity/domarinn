import { useSyncExternalStore } from "react";
import {
  type AllPrefs,
  COLUMNS_KEY,
  EMPTY_PREFS,
  LEGACY_GRID_COLUMNS_KEY,
  migrateLegacyPrefs,
  parsePrefs,
  serializePrefs,
  type TablePrefs,
} from "./tableColumns";

/**
 * The live store behind every table's column preferences.
 *
 * A store rather than component state because each table has two consumers of
 * the same value on screen at once — the picker and the header — and a value
 * read once at mount leaves them disagreeing until something remounts. That is
 * the bug `output/prefs.ts` was written to fix, and the case grid still had it.
 */

function load(): AllPrefs {
  try {
    const migrated = migrateLegacyPrefs(
      parsePrefs(localStorage.getItem(COLUMNS_KEY)),
      localStorage.getItem(LEGACY_GRID_COLUMNS_KEY),
    );
    return migrated;
  } catch {
    /* localStorage throws in private modes / sandboxed frames */
    return {};
  }
}

function save(all: AllPrefs): void {
  try {
    localStorage.setItem(COLUMNS_KEY, serializePrefs(all));
  } catch {
    /* ignore: a viewing preference is not worth breaking the page over */
  }
}

let state: AllPrefs = load();

/**
 * Per-table snapshots, memoized so `getSnapshot` returns a stable reference.
 *
 * Returning a fresh object per call loops `useSyncExternalStore`; returning a
 * shared one for every table would re-render each table whenever any other
 * changed. An entry is replaced only when that table is actually mutated.
 */
const snapshots = new Map<string, TablePrefs>();

const listeners = new Set<() => void>();

function emit() {
  for (const l of listeners) l();
}

export function subscribeColumnPrefs(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function getColumnPrefs(tableId: string): TablePrefs {
  const cached = snapshots.get(tableId);
  if (cached) return cached;
  const next = state[tableId] ?? EMPTY_PREFS;
  snapshots.set(tableId, next);
  return next;
}

function update(tableId: string, next: TablePrefs): void {
  state = { ...state, [tableId]: next };
  snapshots.set(tableId, next);
  save(state);
  emit();
}

export function setColumnVisible(
  tableId: string,
  id: string,
  visible: boolean,
): void {
  const current = getColumnPrefs(tableId);
  // A no-op write would still notify, re-rendering the table for nothing.
  if (current.visible[id] === visible) return;
  update(tableId, {
    ...current,
    visible: { ...current.visible, [id]: visible },
  });
}

/** `px` is expected pre-clamped by the caller, which holds the `ColumnDef`. */
export function setColumnWidth(tableId: string, id: string, px: number): void {
  const current = getColumnPrefs(tableId);
  if (current.width[id] === px) return;
  update(tableId, { ...current, width: { ...current.width, [id]: px } });
}

/** Forget one column's width, returning it to the layout's own track. */
export function resetColumnWidth(tableId: string, id: string): void {
  const current = getColumnPrefs(tableId);
  if (current.width[id] === undefined) return;
  const { [id]: _dropped, ...width } = current.width;
  update(tableId, { ...current, width });
}

export function resetColumns(tableId: string): void {
  update(tableId, { visible: {}, width: {} });
}

/** Keeps two tabs of the same table in agreement. */
if (typeof window !== "undefined") {
  window.addEventListener("storage", (e) => {
    if (e.key !== COLUMNS_KEY) return;
    state = load();
    // Drop every memoized snapshot: each table re-seeds on next read, and the
    // reference it gets is then stable again.
    snapshots.clear();
    emit();
  });
}

export function useColumnPrefs(tableId: string): TablePrefs {
  return useSyncExternalStore(
    subscribeColumnPrefs,
    () => getColumnPrefs(tableId),
    () => getColumnPrefs(tableId),
  );
}

/** Test-only: re-seed module state from storage between cases. */
export function __resetColumnPrefs(): void {
  state = load();
  snapshots.clear();
  emit();
}
