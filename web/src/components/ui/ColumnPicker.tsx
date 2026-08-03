import { Popover } from "@/components/ui/Popover";
import {
  type ColumnDef,
  hiddenCount as countHidden,
  isVisible,
  type TablePrefs,
  visibleColumns,
} from "@/lib/tableColumns";

/**
 * Chooses which of a table's columns are shown.
 *
 * Every table in the app offers more columns than fit somewhere: the case grid
 * wants ~1980px with per-assertion columns expanded, the runs list carries
 * thirteen. The default sets favour what the table is scrolled to *for*, and
 * this is where anyone who disagrees puts the rest back.
 *
 * Sections come from each column's `group`, in first-appearance order, so a
 * table with one kind of column renders one unlabelled list and the case grid
 * still gets its assertions separated out.
 */
export function ColumnPicker({
  columns,
  prefs,
  onChange,
  onReset,
}: {
  columns: readonly ColumnDef[];
  prefs: TablePrefs;
  onChange: (id: string, visible: boolean) => void;
  onReset: () => void;
}) {
  // Structural columns are not offered, so they are not counted as hidden
  // either — the trigger would otherwise report a number the popover cannot
  // account for.
  const pickable = columns.filter((c) => !c.alwaysVisible);
  const hidden = countHidden(columns, prefs);
  // Unchecking the last one leaves a table with no columns and no way back,
  // since this popover is anchored to the table it just emptied.
  const lastVisible = visibleColumns(columns, prefs).length <= 1;

  const groups: { key: string; label: string | null; items: ColumnDef[] }[] = [];
  for (const col of pickable) {
    const key = col.group ?? "";
    const existing = groups.find((g) => g.key === key);
    if (existing) existing.items.push(col);
    else groups.push({ key, label: col.group ?? null, items: [col] });
  }

  return (
    <Popover
      align="end"
      trigger={
        <button
          type="button"
          className="inline-flex h-8 items-center gap-1.5 rounded-md border border-border bg-surface px-2.5 text-xs font-medium text-muted transition-colors hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        >
          <svg
            width="14"
            height="14"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            aria-hidden
          >
            <path d="M4 5h16M4 12h16M4 19h10" />
          </svg>
          Columns
          {hidden > 0 ? (
            <span className="tabular-nums text-muted/70">· {hidden} hidden</span>
          ) : null}
        </button>
      }
    >
      <div className="w-60">
        <div className="mb-2 flex items-center justify-between">
          <span className="text-xs font-semibold uppercase tracking-wide text-muted">
            Show columns
          </span>
          <button
            type="button"
            onClick={onReset}
            className="rounded px-1 text-[11px] font-medium text-accent hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            Reset
          </button>
        </div>
        <div className="max-h-72 space-y-3 overflow-y-auto overscroll-contain">
          {groups.map((g) => (
            <fieldset key={g.key}>
              {g.label ? (
                <legend className="mb-1 text-[11px] font-medium uppercase tracking-wide text-muted/80">
                  {g.label}
                </legend>
              ) : null}
              <div className="space-y-0.5">
                {g.items.map((c) => {
                  const checked = isVisible(c, prefs);
                  return (
                    <label
                      key={c.id}
                      className="flex cursor-pointer items-center gap-2 rounded px-1 py-1 text-sm hover:bg-surface-2"
                    >
                      <input
                        type="checkbox"
                        className="size-3.5 accent-[var(--color-accent)]"
                        checked={checked}
                        // Disabled rather than absent: the column is still a
                        // real choice, it just cannot be the one you remove.
                        disabled={checked && lastVisible}
                        onChange={(e) => onChange(c.id, e.target.checked)}
                      />
                      <span className="truncate">{c.label}</span>
                    </label>
                  );
                })}
              </div>
            </fieldset>
          ))}
        </div>
      </div>
    </Popover>
  );
}
