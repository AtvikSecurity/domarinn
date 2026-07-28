/**
 * The case drawer's width: persisted, clamped, and shared by every run.
 *
 * A fixed 44rem drawer is wrong in both directions — too narrow to read a long
 * rendered prompt or a side-by-side diff, too wide when you only want to glance
 * at a verdict while keeping the grid visible. So it is user-controlled, and the
 * choice sticks: resizing it on every case would be worse than not having it.
 *
 * Pure functions here, storage effects in the hook, so the clamping rules can
 * be tested without a DOM.
 */

/** localStorage key, matching the `domarinn.grid.columns` convention. */
export const DRAWER_WIDTH_KEY = "domarinn.drawer.width";

/** Narrow enough to leave the grid usable behind it. */
export const MIN_WIDTH = 360;

/** Fraction of the viewport the drawer may occupy at most. */
const MAX_FRACTION = 0.95;

export const DEFAULT_WIDTH = 704; // 44rem, the previous fixed width.

/**
 * The widest the drawer may be at this viewport.
 *
 * A fraction rather than a constant, so it is nearly full-screen on a laptop
 * and still leaves a visible seam — a drawer covering 100% is a page, and the
 * overlay behind it stops being a way out. The minimum wins on a very narrow
 * viewport: an unusably thin drawer is worse than one that overflows slightly.
 */
export function maxWidth(viewport: number): number {
  return Math.max(MIN_WIDTH, Math.floor(viewport * MAX_FRACTION));
}

/** Clamp a width to something usable at the current viewport. */
export function clampWidth(width: number, viewport: number): number {
  const max = maxWidth(viewport);
  // A stored value can be anything; a non-finite one means "no usable
  // preference", not "as wide as possible".
  if (!Number.isFinite(width)) return Math.min(DEFAULT_WIDTH, max);
  return Math.min(Math.max(Math.round(width), MIN_WIDTH), max);
}

/** Parse a stored value, falling back to the default on anything unusable. */
export function parseStoredWidth(raw: string | null): number {
  if (raw === null) return DEFAULT_WIDTH;
  const n = Number.parseInt(raw, 10);
  return Number.isFinite(n) && n > 0 ? n : DEFAULT_WIDTH;
}

/**
 * The width a "toggle expand" should produce.
 *
 * Two states rather than a remembered pair: expanded is always the maximum, and
 * collapsing returns to the default. Storing a separate "restore" width would
 * make the button's result depend on invisible history.
 */
export function toggledWidth(current: number, viewport: number): number {
  const max = maxWidth(viewport);
  // Within a few pixels of the maximum counts as expanded, so a drag that
  // lands near the edge still collapses on the next click.
  return current >= max - 4 ? clampWidth(DEFAULT_WIDTH, viewport) : max;
}

/** Keyboard step for the resize handle; Shift takes bigger strides. */
export const RESIZE_STEP = 24;
export const RESIZE_STEP_LARGE = 96;
