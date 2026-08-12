import type { ReactNode } from "react";
import { cn } from "@/lib/cn";
import { ColumnResizer } from "@/components/ui/ColumnResizer";
import { SortArrow } from "@/components/ui/SortArrow";
import { type ColumnDef, effectiveWidth, type TablePrefs } from "@/lib/tableColumns";
import {
  resetColumnWidth,
  setColumnWidth,
} from "@/lib/useColumnPrefs";

/**
 * A `<th>` carrying a resize handle, and optionally a sort toggle.
 *
 * Deliberately not a `DataTable` component: each page keeps its own `<thead>`,
 * `<tbody>` and row rendering, which is where all the interesting differences
 * live. This adds the one thing they all need and nothing else.
 */
export function ResizableTh({
  def,
  tableId,
  prefs,
  className,
  scope = "col",
  isLast = false,
  sort,
  children,
  ...rest
}: {
  def: ColumnDef;
  tableId: string;
  prefs: TablePrefs;
  className?: string;
  scope?: "col" | "colgroup";
  /** Keeps the trailing handle inside the table — see `ColumnResizer`'s `edge`. */
  isLast?: boolean;
  /**
   * Present only on sortable columns. A column without it renders an inert
   * header with no button and no `aria-sort` at all — announcing "none" on a
   * column that cannot be sorted would be a lie (the cache grid's convention).
   */
  sort?: { active: false | "asc" | "desc"; onToggle: () => void };
  children: ReactNode;
} & Omit<React.ThHTMLAttributes<HTMLTableCellElement>, "scope" | "className">) {
  const labelId = `${tableId}-h-${def.id}`;
  return (
    <th
      scope={scope}
      // `relative` so the handle can straddle this cell's right edge.
      className={cn("relative", className)}
      aria-sort={
        sort
          ? sort.active === "asc"
            ? "ascending"
            : sort.active === "desc"
              ? "descending"
              : "none"
          : undefined
      }
      {...rest}
    >
      {sort ? (
        <button
          type="button"
          onClick={sort.onToggle}
          className={cn(
            "inline-flex w-full min-w-0 items-center gap-1 rounded uppercase tracking-wide transition-colors hover:text-fg focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
            def.numeric ? "justify-end" : "justify-start",
          )}
        >
          <span id={labelId} className="truncate">{children}</span>
          <SortArrow dir={sort.active} />
        </button>
      ) : (
        <span id={labelId}>{children}</span>
      )}
      {/* A SIBLING of the sort button, never inside it: a pointerdown that
          bubbles into the sort toggle is the classic table-resizer bug. */}
      <ColumnResizer
        def={def}
        width={effectiveWidth(def, prefs)}
        headerId={labelId}
        edge={isLast}
        onResize={(px) => setColumnWidth(tableId, def.id, px)}
        onReset={() => resetColumnWidth(tableId, def.id)}
      />
    </th>
  );
}
