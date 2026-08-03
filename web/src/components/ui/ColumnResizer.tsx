import { useId, useRef } from "react";
import { cn } from "@/lib/cn";
import { clampWidth, type ColumnDef, MAX_COLUMN_WIDTH } from "@/lib/tableColumns";

/** Keyboard step. Fine enough to aim with, coarse enough to get somewhere. */
export const COLUMN_RESIZE_STEP = 16;

/**
 * The drag handle on a column header.
 *
 * Pointer handling is bespoke rather than TanStack's `getResizeHandler`: only
 * two of the app's tables use react-table at all, and that handler is
 * inseparable from react-table's own `columnSizing` state, which would become
 * a second source of truth beside the persisted store. One implementation that
 * works for both the virtualized CSS grids and the plain `<table>` pages beats
 * two that agree by hand.
 *
 * The accessible control is a visually-hidden `<input type="range">`, the
 * pattern React Aria settled on, rather than a focusable `role="separator"`.
 * A focusable separator is the only ARIA role that changes meaning based on
 * focusability, the working group has it flagged for deprecation, and it
 * cannot express a disabled state — which a non-resizable column needs. A
 * native range gives all of that for free, plus real value semantics: screen
 * readers announce "Resize, Latency, 250 pixels" rather than a bare number.
 *
 * These grids bind Enter/Space on rows but never arrow keys, so the arrows
 * drive the range directly. React Aria's Enter-to-enter-resize-mode exists to
 * stop a focusable child fighting full arrow-key cell navigation; adding it
 * here would be ceremony guarding against a conflict that does not exist.
 */
export function ColumnResizer({
  def,
  width,
  headerId,
  edge = false,
  onResize,
  onReset,
}: {
  def: ColumnDef;
  /** The column's current effective width in px. */
  width: number;
  /** Id of the header cell's label, so the announcement names the column. */
  headerId: string;
  /**
   * This is the last column, so the handle sits fully inside its cell rather
   * than straddling the edge. Half of a straddling handle hangs past the table
   * itself, and a scroller then reports content wider than it is — a scrollbar
   * under every table, promising columns that are not there.
   */
  edge?: boolean;
  onResize: (px: number) => void;
  /** Double-click / Home: back to the layout's own track. */
  onReset: () => void;
}) {
  const labelId = useId();
  const drag = useRef<{ startX: number; startWidth: number } | null>(null);

  if (def.resizable === false) return null;

  function onPointerDown(e: React.PointerEvent<HTMLDivElement>) {
    // Primary button only, and never let this reach the sort button that
    // shares the header cell — a pointerdown bubbling into the sort toggle is
    // the classic table-resizer bug.
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    drag.current = { startX: e.clientX, startWidth: width };
    // Pointer capture rather than window listeners, matching the drawer
    // splitter: the drag keeps tracking when the cursor leaves the handle.
    e.currentTarget.setPointerCapture(e.pointerId);
  }

  function onPointerMove(e: React.PointerEvent<HTMLDivElement>) {
    const state = drag.current;
    if (!state) return;
    onResize(clampWidth(def, state.startWidth + (e.clientX - state.startX)));
  }

  function endDrag(e: React.PointerEvent<HTMLDivElement>) {
    if (!drag.current) return;
    drag.current = null;
    e.currentTarget.releasePointerCapture(e.pointerId);
  }

  return (
    <div
      data-column-resizer={def.id}
      // A 1px seam is the right look and an unusable target, so the hit area
      // is wider and invisible, straddling the column boundary.
      //
      // On the last column it does not straddle — it tucks against the inside
      // of the edge, and clips: `sr-only` carries `margin: -1px`, so the
      // hidden label and range would each bleed a pixel past the table and the
      // scroller would report content wider than itself.
      className={cn(
        "group absolute inset-y-0 right-0 z-20 flex w-3 cursor-col-resize items-stretch",
        edge ? "justify-end overflow-hidden" : "translate-x-1/2 justify-center",
      )}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      onDoubleClick={(e) => {
        e.stopPropagation();
        onReset();
      }}
    >
      <span id={labelId} className="sr-only">
        Resize
      </span>
      <input
        type="range"
        className="peer sr-only"
        min={def.min}
        max={def.max ?? MAX_COLUMN_WIDTH}
        step={COLUMN_RESIZE_STEP}
        value={width}
        aria-labelledby={`${labelId} ${headerId}`}
        // Raw range numbers mean nothing out loud; pixels do.
        aria-valuetext={`${width} pixels`}
        onChange={(e) => onResize(clampWidth(def, Number(e.target.value)))}
        onKeyDown={(e) => {
          if (e.key === "Home") {
            e.preventDefault();
            onReset();
          }
        }}
      />
      <span
        aria-hidden
        className={cn(
          "w-px self-stretch bg-border transition-colors",
          "group-hover:bg-accent peer-focus-visible:w-0.5 peer-focus-visible:bg-accent",
        )}
      />
    </div>
  );
}
