import { useSyncExternalStore } from "react";

/**
 * Shared Rendered/Raw and soft-wrap preferences for every `OutputViewer`.
 *
 * These were always meant to be global (they persist to localStorage), but each
 * viewer read the value once at mount into its own state. Several viewers are
 * routinely on screen at the same time — the case drawer renders one per prompt
 * message plus one for the output — so toggling "Raw" on one left the others
 * showing Rendered until they happened to remount. This makes the store the
 * single source of truth and subscribes every viewer to it.
 */

const RAW_KEY = "domarinn.output.raw";
const WRAP_KEY = "domarinn.output.wrap";

export interface OutputPrefs {
  raw: boolean;
  wrap: boolean;
}

function readBool(key: string, fallback: boolean): boolean {
  try {
    const v = localStorage.getItem(key);
    if (v === "1") return true;
    if (v === "0") return false;
  } catch {
    /* localStorage can throw in private modes / sandboxed frames */
  }
  return fallback;
}

function writeBool(key: string, value: boolean): void {
  try {
    localStorage.setItem(key, value ? "1" : "0");
  } catch {
    /* ignore */
  }
}

// Seeded once, then held in memory: `getSnapshot` must return a stable
// reference or useSyncExternalStore loops.
let state: OutputPrefs = {
  raw: readBool(RAW_KEY, false),
  wrap: readBool(WRAP_KEY, true),
};

const listeners = new Set<() => void>();

function emit() {
  for (const l of listeners) l();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function getSnapshot(): OutputPrefs {
  return state;
}

export function setRawMode(raw: boolean): void {
  if (state.raw === raw) return;
  state = { ...state, raw };
  writeBool(RAW_KEY, raw);
  emit();
}

export function setWrap(wrap: boolean): void {
  if (state.wrap === wrap) return;
  state = { ...state, wrap };
  writeBool(WRAP_KEY, wrap);
  emit();
}

/** Keeps two tabs of the same run in agreement. */
if (typeof window !== "undefined") {
  window.addEventListener("storage", (e) => {
    if (e.key !== RAW_KEY && e.key !== WRAP_KEY) return;
    state = {
      raw: readBool(RAW_KEY, state.raw),
      wrap: readBool(WRAP_KEY, state.wrap),
    };
    emit();
  });
}

export function useOutputPrefs(): OutputPrefs {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/** Test-only: restore module state between cases. */
export function __resetOutputPrefs(next: OutputPrefs = { raw: false, wrap: true }): void {
  state = next;
  emit();
}
