import {
  type ColumnDef,
  type TablePrefs,
  tableWidthFor,
  visibleColumns,
} from "@/lib/tableColumns";

/**
 * The `<colgroup>` that makes a real `<table>`'s column widths authoritative.
 *
 * Under the default `table-layout: auto` a `<th>`'s width is advisory — the
 * browser sizes columns from their content and quietly ignores what you asked
 * for, which is why resizing a plain table does nothing until the table is
 * `table-layout: fixed` and the widths come from here.
 *
 * The trade is real and worth stating: content-driven sizing disappears, so a
 * column that used to grow to fit now truncates. Every `sr-only` header column
 * therefore needs an explicit width, or it collapses to nothing — the `<col>`
 * wins over the `<th>`'s utility class once this element exists.
 */
export function ColumnGroup({
  columns,
  prefs,
}: {
  columns: readonly ColumnDef[];
  prefs: TablePrefs;
}) {
  return (
    <colgroup>
      {visibleColumns(columns, prefs).map((c) => {
        const width = tableWidthFor(c, prefs);
        return <col key={c.id} {...(width ? { style: { width } } : {})} />;
      })}
    </colgroup>
  );
}
