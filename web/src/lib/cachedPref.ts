import { useSyncExternalStore } from "react";
import type { CachedFilter } from "@/api";
import { DEFAULT_CACHED, isCachedFilter } from "./cached";

/**
 * Whether fully-cached runs are noise, as a standing choice rather than a
 * per-page one.
 *
 * The cached filter used to live only in the runs list's URL, which meant the
 * answer to "is this noise?" had to be re-given on every surface that grew one
 * — and every surface that did not grow one silently picked its own. A single
 * store makes the choice once and lets the runs list, the suite pages and
 * search all read it.
 *
 * It is a store rather than component state because several of those surfaces
 * are on screen together (the runs list and its filter bar, a suite page and
 * its toggle), and a value read once at mount leaves them disagreeing until
 * something remounts — the same bug `output/prefs.ts` was written to fix.
 *
 * This is only the *default*. A `?cached=` in the URL beats it, so a shared
 * link still shows the same runs to everyone; see `resolveCached`.
 */
export const CACHED_PREF_KEY = "domarinn.cached.mode";

function read(): CachedFilter {
  try {
    const v = localStorage.getItem(CACHED_PREF_KEY);
    if (isCachedFilter(v)) return v;
  } catch {
    /* localStorage throws in private modes / sandboxed frames */
  }
  return DEFAULT_CACHED;
}

function write(value: CachedFilter): void {
  try {
    localStorage.setItem(CACHED_PREF_KEY, value);
  } catch {
    /* ignore: a preference is not worth breaking the page over */
  }
}

// Seeded once, then held in memory. `getSnapshot` returns a string, so it is
// stable by value and cannot loop `useSyncExternalStore`.
let state: CachedFilter = read();

const listeners = new Set<() => void>();

function emit() {
  for (const l of listeners) l();
}

export function subscribeCachedPref(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function getCachedPref(): CachedFilter {
  return state;
}

export function setCachedPref(next: CachedFilter): void {
  // A no-op write would still notify, re-rendering every subscribed surface
  // for nothing.
  if (state === next) return;
  state = next;
  write(next);
  emit();
}

/** Keeps two tabs of the same dashboard in agreement. */
if (typeof window !== "undefined") {
  window.addEventListener("storage", (e) => {
    if (e.key !== CACHED_PREF_KEY) return;
    const next = read();
    if (next === state) return;
    state = next;
    emit();
  });
}

export function useCachedPref(): CachedFilter {
  return useSyncExternalStore(subscribeCachedPref, getCachedPref, getCachedPref);
}

/** Test-only: re-seed module state from storage between cases. */
export function __resetCachedPref(): void {
  state = read();
  emit();
}
