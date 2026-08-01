import { useCallback, useRef } from "react";
import type { MouseEvent as ReactMouseEvent } from "react";
import { useNavigate } from "react-router";

/**
 * Anything that owns its own click. `label` is in the list because the
 * selection checkboxes are wrapped in one to reach WCAG 2.2's 24px target.
 *
 * A `closest()` test rather than `stopPropagation()` on each child: the
 * propagation style is opt-out, so a cell added later silently starts
 * swallowing the row's navigation, and it needs a wrapper element per control.
 */
const INTERACTIVE =
  "a,button,input,select,textarea,label,[role='button'],[data-row-nav-ignore]";

/** Pointer travel between press and release that reads as a drag, not a click. */
const DRAG_SLOP_PX = 4;

/**
 * Whole-row navigation for the `<table>` pages.
 *
 * This is a **pointer convenience only** — deliberately no `tabIndex`, no
 * `role`, no `onKeyDown` on the row. Every row that uses this already contains
 * a real `<Link>` in its first cell, which is the tab stop, the accessible
 * name, and the thing that makes ⌘-click and "open in new tab" work. Adding
 * keyboard handling to the row as well would put two tab stops on every row
 * and announce a `button` inside a `table` for no gain.
 *
 * That is what distinguishes this from `CaseGrid` / `CacheEntryGrid`, which do
 * set `role`/`tabIndex`/`onKeyDown`: their rows contain no link at all, so the
 * row is the only affordance and must be operable. Do not "unify" them.
 */
export function useRowNav() {
  const navigate = useNavigate();
  // Where the press started, so a release far away can be read as a drag.
  // One ref for all rows is safe: a press and release on different rows fires
  // `click` on their common ancestor, never on a row.
  const origin = useRef<{ x: number; y: number } | null>(null);

  return useCallback(
    (to: string) => ({
      onMouseDown: (e: ReactMouseEvent) => {
        origin.current = { x: e.clientX, y: e.clientY };
      },
      onClick: (e: ReactMouseEvent) => {
        const start = origin.current;
        origin.current = null;

        if (e.defaultPrevented) return;

        // Modified clicks mean "open elsewhere". Bail rather than opening a
        // tab ourselves: navigating the current tab when someone ⌘-clicked
        // would lose their place, and `window.open` here would both trip
        // popup blockers and double-fire when the press landed on the cell
        // link, which already does this natively.
        if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;

        // Let any real control — including the row's own link — have its click.
        if ((e.target as HTMLElement).closest(INTERACTIVE)) return;

        // Two different gestures, both of which must not navigate: dragging to
        // select text or to scroll the table sideways (travel > slop), and
        // double-clicking to select a word (travel of zero, but a selection).
        if (
          start &&
          Math.hypot(e.clientX - start.x, e.clientY - start.y) > DRAG_SLOP_PX
        ) {
          return;
        }
        if (window.getSelection()?.toString()) return;

        void navigate(to);
      },
    }),
    [navigate],
  );
}
