import { log } from "@/lib/logger";

/**
 * Which case-grid columns the user wants to see.
 *
 * The grid can offer far more columns than fit: a matrix-shaped run with six
 * assertion types needs ~1980px, which does not fit any laptop, and the columns
 * that fall off the right edge are the numeric ones (tokens, cost, latency,
 * score) — the ones you scroll to the grid to read. The default set trades the
 * per-assertion columns (mostly em-dashes, since each test uses only one or two
 * of the run's assertion types) for a single combined strip, and the picker
 * lets anyone put the individual columns back.
 *
 * Preferences live in localStorage rather than the URL: they are a per-person
 * viewing habit, not part of what a shared run link should mean.
 */
export const GRID_COLUMNS_KEY = "domarinn.grid.columns";

/** Column id -> visible. Only ids the user has actually toggled are stored, so
 *  columns added later still get their own default. */
export type ColumnVisibility = Record<string, boolean>;

/** Ids that are structural and never offered to the picker. */
export const ALWAYS_VISIBLE = new Set(["status", "name"]);

/** Per-assertion columns are `assert:<label>`. */
export function isAssertColumn(id: string): boolean {
  return id.startsWith("assert:");
}

/**
 * Default visibility for a column id. Per-assertion columns start hidden — the
 * combined `asserts` strip covers the same information in a twelfth of the
 * width — and everything else starts visible.
 */
export function defaultVisible(id: string): boolean {
  return !isAssertColumn(id);
}

export function loadColumnVisibility(): ColumnVisibility {
  try {
    const raw = localStorage.getItem(GRID_COLUMNS_KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
      return {};
    }
    // Keep only boolean entries; a corrupted value should degrade to defaults
    // rather than hiding the grid.
    return Object.fromEntries(
      Object.entries(parsed as Record<string, unknown>).filter(
        ([, v]) => typeof v === "boolean",
      ),
    ) as ColumnVisibility;
  } catch (err) {
    log.warn("could not read grid column preferences", err);
    return {};
  }
}

export function saveColumnVisibility(value: ColumnVisibility): void {
  try {
    localStorage.setItem(GRID_COLUMNS_KEY, JSON.stringify(value));
  } catch (err) {
    log.warn("could not persist grid column preferences", err);
  }
}

/** Effective visibility for one column, honouring the stored override. */
export function isVisible(id: string, stored: ColumnVisibility): boolean {
  if (ALWAYS_VISIBLE.has(id)) return true;
  return stored[id] ?? defaultVisible(id);
}
