import type { ReactNode } from "react";
import { cn } from "@/lib/cn";
import { ColumnResizer } from "@/components/ui/ColumnResizer";
import { clampWidth, type ColumnDef, type TablePrefs } from "@/lib/tableColumns";
import {
  resetColumnWidth,
  setColumnWidth,
} from "@/lib/useColumnPrefs";

/**
 * A `<th>` carrying a resize handle.
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
  children,
  ...rest
}: {
  def: ColumnDef;
  tableId: string;
  prefs: TablePrefs;
  className?: string;
  scope?: "col" | "colgroup";
  children: ReactNode;
} & Omit<React.ThHTMLAttributes<HTMLTableCellElement>, "scope" | "className">) {
  const labelId = `${tableId}-h-${def.id}`;
  return (
    <th
      scope={scope}
      // `relative` so the handle can straddle this cell's right edge.
      className={cn("relative", className)}
      {...rest}
    >
      <span id={labelId}>{children}</span>
      <ColumnResizer
        def={def}
        width={clampWidth(def, prefs.width[def.id] ?? def.min)}
        headerId={labelId}
        onResize={(px) => setColumnWidth(tableId, def.id, px)}
        onReset={() => resetColumnWidth(tableId, def.id)}
      />
    </th>
  );
}
