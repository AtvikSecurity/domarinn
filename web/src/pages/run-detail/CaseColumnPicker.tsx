import { Popover } from "@/components/ui/Popover";
import { isVisible, type ColumnVisibility } from "@/lib/gridColumns";

export interface PickableColumn {
  id: string;
  label: string;
  /** Grouped in the popover so the per-assertion list stays scannable. */
  group: "columns" | "assertions";
}

/**
 * Chooses which case-grid columns are shown.
 *
 * The default set favours the numbers (tokens, cost, latency, score) over the
 * per-assertion columns, because the numbers are what the grid is scrolled to
 * for and the per-assertion columns are mostly empty. This is where anyone who
 * disagrees puts them back — including one column per assertion type, which is
 * how the grid behaved before.
 */
export function CaseColumnPicker({
  columns,
  visibility,
  onChange,
  onReset,
}: {
  columns: readonly PickableColumn[];
  visibility: ColumnVisibility;
  onChange: (id: string, visible: boolean) => void;
  onReset: () => void;
}) {
  const hiddenCount = columns.filter((c) => !isVisible(c.id, visibility)).length;
  const groups = [
    { key: "columns" as const, label: "Columns" },
    { key: "assertions" as const, label: "Assertions" },
  ];

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
          {hiddenCount > 0 ? (
            <span className="tabular-nums text-muted/70">· {hiddenCount} hidden</span>
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
          {groups.map((g) => {
            const items = columns.filter((c) => c.group === g.key);
            if (items.length === 0) return null;
            return (
              <fieldset key={g.key}>
                <legend className="mb-1 text-[11px] font-medium uppercase tracking-wide text-muted/80">
                  {g.label}
                </legend>
                <div className="space-y-0.5">
                  {items.map((c) => (
                    <label
                      key={c.id}
                      className="flex cursor-pointer items-center gap-2 rounded px-1 py-1 text-sm hover:bg-surface-2"
                    >
                      <input
                        type="checkbox"
                        className="size-3.5 accent-[var(--color-accent)]"
                        checked={isVisible(c.id, visibility)}
                        onChange={(e) => onChange(c.id, e.target.checked)}
                      />
                      <span className="truncate">{c.label}</span>
                    </label>
                  ))}
                </div>
              </fieldset>
            );
          })}
        </div>
      </div>
    </Popover>
  );
}
