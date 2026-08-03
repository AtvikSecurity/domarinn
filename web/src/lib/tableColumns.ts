/**
 * Which columns a table shows, and how wide they are.
 *
 * The case grid already proved the need: a matrix-shaped run with six assertion
 * types wants ~1980px, which fits no laptop, and the columns that fall off the
 * right edge are the numeric ones people scrolled to the grid to read. Every
 * other table in the app has the same problem to a lesser degree — the runs
 * list carries thirteen columns at a 1080px floor.
 *
 * Pure functions here, storage effects in `useColumnPrefs`, so the rules can be
 * tested without a DOM — the split `drawerWidth.ts` uses.
 *
 * Preferences live in localStorage rather than the URL: they are a per-person
 * viewing habit, not part of what a shared link should mean. One key rather
 * than one per table, so the cross-tab `storage` listener stays an exact-key
 * match; per-table isolation comes from parsing each entry independently.
 */

import { log } from "@/lib/logger";

export const COLUMNS_KEY = "domarinn.columns";

/** The case grid's pre-generalization key, still read once for migration. */
export const LEGACY_GRID_COLUMNS_KEY = "domarinn.grid.columns";

/** Wide enough for any real content; narrow enough to stay draggable back. */
export const MAX_COLUMN_WIDTH = 800;

const SCHEMA_VERSION = 1;

/** Everything a table needs to lay out, offer, and resize one column. */
export interface ColumnDef {
  id: string;
  /** Human label for the picker. */
  label: string;
  /** Picker section; undefined groups into the unlabelled default section. */
  group?: string;
  /** CSS grid track (or `<col>` width) when the user has not resized it. */
  track: string;
  /** Intrinsic px floor. Also the resize floor — see `clampWidth`. */
  min: number;
  /** Resize ceiling; defaults to `MAX_COLUMN_WIDTH`. */
  max?: number;
  numeric?: boolean;
  sticky?: boolean;
  /** Structural: never offered to the picker, never hidden. */
  alwaysVisible?: boolean;
  /** Visible when the user has no stored opinion. Defaults to true. */
  defaultVisible?: boolean;
  /** Defaults to true. */
  resizable?: boolean;
}

/**
 * One table's stored state. Both maps are SPARSE — only ids the user actually
 * touched appear — which is what lets a column added in a later release get
 * its own default rather than inheriting a stale `false`.
 */
export interface TablePrefs {
  visible: Record<string, boolean>;
  width: Record<string, number>;
}

export type AllPrefs = Record<string, TablePrefs>;

export const EMPTY_PREFS: TablePrefs = { visible: {}, width: {} };

export function isVisible(def: ColumnDef, prefs: TablePrefs): boolean {
  if (def.alwaysVisible) return true;
  return prefs.visible[def.id] ?? def.defaultVisible ?? true;
}

/**
 * The columns to render, in declaration order.
 *
 * Never empty: a table with every column hidden has no way back, because the
 * picker that would undo it is anchored to a table that no longer renders.
 * `useColumnPrefs` refuses to hide the last one, and this is the second guard
 * for a blob that arrived hidden-everything some other way.
 */
export function visibleColumns(
  defs: readonly ColumnDef[],
  prefs: TablePrefs,
): ColumnDef[] {
  const shown = defs.filter((d) => isVisible(d, prefs));
  if (shown.length > 0) return shown;
  return defs.length > 0 ? [defs[0]!] : [];
}

/** How many columns the picker could bring back. */
export function hiddenCount(
  defs: readonly ColumnDef[],
  prefs: TablePrefs,
): number {
  return defs.filter((d) => !d.alwaysVisible && !isVisible(d, prefs)).length;
}

export function clampWidth(def: ColumnDef, px: number): number {
  const max = def.max ?? MAX_COLUMN_WIDTH;
  // A stored value can be anything; a non-finite one means "no usable
  // preference", not "as narrow as possible".
  if (!Number.isFinite(px)) return def.min;
  return Math.min(Math.max(Math.round(px), def.min), max);
}

/** The `<n>fr` share of a track, if it has one. */
function flexShare(track: string): string | null {
  return /(\d*\.?\d+fr)/.exec(track)?.[1] ?? null;
}

/**
 * The CSS track for one column, honouring a stored width.
 *
 * A resized flexible column keeps its `fr` share rather than becoming a bare
 * `Npx`. Of the case grid's eleven columns only two are flexible, so pinning
 * them to pixels takes the whole flex pool away on the second drag and the
 * grid stops filling its container at any viewport — permanently, because the
 * preference persists. `minmax(userPx, share)` honours the floor the user
 * dragged to and still lets the column take up slack.
 */
export function trackFor(def: ColumnDef, prefs: TablePrefs): string {
  const stored = prefs.width[def.id];
  if (stored === undefined) return def.track;
  const px = clampWidth(def, stored);
  const share = flexShare(def.track);
  return share ? `minmax(${px}px, ${share})` : `${px}px`;
}

export function gridTemplateFor(
  defs: readonly ColumnDef[],
  prefs: TablePrefs,
): string {
  return visibleColumns(defs, prefs)
    .map((d) => trackFor(d, prefs))
    .join(" ");
}

/**
 * The intrinsic width of the visible set, plus the container's own inset.
 * This is what makes the horizontal scrollbar appear rather than the last
 * column being clipped.
 */
export function minWidthFor(
  defs: readonly ColumnDef[],
  prefs: TablePrefs,
  inset = 0,
): number {
  return (
    visibleColumns(defs, prefs).reduce(
      (sum, d) => sum + clampWidth(d, prefs.width[d.id] ?? d.min),
      0,
    ) + inset
  );
}

/**
 * Column widths as custom properties for the scroll container.
 *
 * Put on the common ancestor of the header and the rows, a drag updates one
 * style object and every cell reflows in CSS — no React re-render of any row,
 * which is what makes live resizing affordable over a virtualized body.
 */
export function cssVarsFor(
  defs: readonly ColumnDef[],
  prefs: TablePrefs,
): Record<string, string> {
  const vars: Record<string, string> = {};
  for (const d of visibleColumns(defs, prefs)) {
    vars[`--col-${d.id}-size`] = trackFor(d, prefs);
  }
  return vars;
}

function readTable(value: unknown): TablePrefs | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return null;
  }
  const raw = value as { visible?: unknown; width?: unknown };
  const visible: Record<string, boolean> = {};
  const width: Record<string, number> = {};
  if (typeof raw.visible === "object" && raw.visible !== null) {
    for (const [k, v] of Object.entries(raw.visible as Record<string, unknown>)) {
      if (typeof v === "boolean") visible[k] = v;
    }
  }
  if (typeof raw.width === "object" && raw.width !== null) {
    for (const [k, v] of Object.entries(raw.width as Record<string, unknown>)) {
      if (typeof v === "number" && Number.isFinite(v)) width[k] = v;
    }
  }
  return { visible, width };
}

/**
 * Parse the stored blob, degrading to defaults rather than throwing.
 *
 * A corrupted value must never hide a table — that would break exactly the
 * thing it was meant to configure — so unusable entries are dropped and
 * everything else survives.
 */
export function parsePrefs(raw: string | null): AllPrefs {
  if (!raw) return {};
  try {
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
      return {};
    }
    const tables = (parsed as { tables?: unknown }).tables ?? parsed;
    if (typeof tables !== "object" || tables === null || Array.isArray(tables)) {
      return {};
    }
    const out: AllPrefs = {};
    for (const [id, value] of Object.entries(tables as Record<string, unknown>)) {
      if (id === "v") continue;
      const table = readTable(value);
      if (table) out[id] = table;
    }
    return out;
  } catch (err) {
    log.warn("could not read column preferences", err);
    return {};
  }
}

export function serializePrefs(all: AllPrefs): string {
  return JSON.stringify({ v: SCHEMA_VERSION, tables: all });
}

/**
 * Fold the case grid's old flat visibility map into the new shape.
 *
 * The new key wins outright when present: a user who has configured columns
 * since upgrading must not have a stale pre-upgrade blob reapplied over the
 * top. The legacy key is deliberately left in storage by the caller, so a
 * rollback — or a write that failed on quota — does not discard the setting.
 */
export function migrateLegacyPrefs(all: AllPrefs, legacyRaw: string | null): AllPrefs {
  if (all.cases || !legacyRaw) return all;
  try {
    const parsed: unknown = JSON.parse(legacyRaw);
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
      return all;
    }
    const visible: Record<string, boolean> = {};
    for (const [k, v] of Object.entries(parsed as Record<string, unknown>)) {
      if (typeof v === "boolean") visible[k] = v;
    }
    if (Object.keys(visible).length === 0) return all;
    return { ...all, cases: { visible, width: {} } };
  } catch {
    // A legacy value we cannot read is not worth a warning; the user simply
    // starts from defaults.
    return all;
  }
}
