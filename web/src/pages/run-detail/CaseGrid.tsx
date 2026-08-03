import { useEffect, useMemo, useRef, useState } from "react";
import {
  createColumnHelper,
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
  useReactTable,
  type OnChangeFn,
  type SortingState,
} from "@tanstack/react-table";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { CaseListItem } from "@/api";
import { AssertDot, StatusBadge } from "@/components/StatusBadge";
import { Tooltip } from "@/components/ui/Tooltip";
import { Spinner } from "@/components/ui/Spinner";
import {
  formatCaseCount,
  formatCost,
  formatLatency,
  formatTokens,
} from "@/lib/format";
import {
  ALWAYS_VISIBLE,
  isAssertColumn,
  isVisible,
  loadColumnVisibility,
  saveColumnVisibility,
  type ColumnVisibility,
} from "@/lib/gridColumns";
import { compareStatus } from "@/lib/sort";
import { cn } from "@/lib/cn";
import { CaseColumnPicker, type PickableColumn } from "./CaseColumnPicker";

/** Human labels for the non-assertion columns in the picker. */
const COLUMN_LABEL: Record<string, string> = {
  provider: "Provider",
  prompt: "Prompt",
  preview: "Preview",
  empty_reason: "Empty reason",
  asserts: "Asserts (combined)",
  tokens: "Tokens",
  cost: "Cost",
  latency: "Latency",
  score: "Score",
};

const STATUS_W = 84;
const NAME_MIN = 240;
const PREVIEW_MIN = 160;
const ASSERT_W = 72;
const ASSERTS_W = 116;
const NUM_W = 76;
const IDENT_W = 112;

/**
 * One row per column shape, replacing three parallel lookups (a width switch, a
 * min-width switch, and a numeric-column set) that had to be kept in sync by
 * hand. `sticky` pins a column against horizontal scroll.
 */
interface ColumnSpec {
  /** grid-template-columns token. */
  track: string;
  /** px this column contributes to the grid's intrinsic width. */
  min: number;
  /** Right-align the header and cells. */
  numeric?: boolean;
  /** Pin against horizontal scroll. */
  sticky?: boolean;
}

const COLUMN_SPEC: Record<string, ColumnSpec> = {
  // Pinned: with 12+ columns nothing makes them all fit, so the verdict must
  // stay put while you scroll right to read the numbers.
  status: { track: `${STATUS_W}px`, min: STATUS_W, sticky: true },
  name: { track: `minmax(${NAME_MIN}px, 1.2fr)`, min: NAME_MIN },
  provider: { track: `${IDENT_W}px`, min: IDENT_W },
  prompt: { track: `${IDENT_W}px`, min: IDENT_W },
  preview: { track: `minmax(${PREVIEW_MIN}px, 1.4fr)`, min: PREVIEW_MIN },
  empty_reason: { track: `${IDENT_W}px`, min: IDENT_W },
  asserts: { track: `${ASSERTS_W}px`, min: ASSERTS_W },
  tokens: { track: `${NUM_W}px`, min: NUM_W, numeric: true },
  cost: { track: `${NUM_W}px`, min: NUM_W, numeric: true },
  latency: { track: `${NUM_W}px`, min: NUM_W, numeric: true },
  score: { track: `${NUM_W}px`, min: NUM_W, numeric: true },
};

const ASSERT_SPEC: ColumnSpec = { track: `${ASSERT_W}px`, min: ASSERT_W };

function spec(id: string): ColumnSpec {
  return COLUMN_SPEC[id] ?? ASSERT_SPEC;
}

/** Horizontal padding the grid container adds around the tracks (`px-3`). */
const GRID_INSET = 24;

interface CaseGridProps {
  cases: CaseListItem[];
  assertLabels: string[];
  /** Show the provider column (matrix-shaped runs only — same >1-distinct
   *  signal that drives the RunDetail provider chips). */
  showProvider?: boolean;
  /** Show the prompt column (matrix-shaped runs only). */
  showPrompt?: boolean;
  selectedKey?: string;
  onSelect: (caseKey: string) => void;
  sorting: SortingState;
  onSortingChange: OnChangeFn<SortingState>;
  hasNextPage?: boolean;
  fetchNextPage?: () => void;
  isFetchingNextPage?: boolean;
  totalCount?: number;
  /** A filter/search change is in flight and the rows shown are the previous
   *  result. Without this the stale rows are fully opaque and interactive. */
  busy?: boolean;
}

const col = createColumnHelper<CaseListItem>();

export function CaseGrid({
  cases,
  assertLabels,
  showProvider = false,
  showPrompt = false,
  selectedKey,
  onSelect,
  sorting,
  onSortingChange,
  hasNextPage,
  fetchNextPage,
  isFetchingNextPage,
  totalCount,
  busy = false,
}: CaseGridProps) {
  const columns = useMemo(() => {
    const assertCols = assertLabels.map((label) =>
      col.display({
        id: `assert:${label}`,
        header: () => (
          <Tooltip content={label}>
            <span className="block truncate">{label}</span>
          </Tooltip>
        ),
        cell: ({ row }) => {
          const a = row.original.asserts.find((x) => x.label === label);
          if (!a)
            return (
              <>
                <span className="sr-only">{label}: not evaluated</span>
                <span className="text-muted/50" aria-hidden>
                  –
                </span>
              </>
            );
          return (
            // The label lives in `aria-label`, not a native `title`: the dot is
            // the only content in the cell, so this is the sole text a screen
            // reader has to work with.
            <AssertDot
              passed={a.passed}
              label={`${label} · ${a.kind} · ${a.passed ? "passed" : "failed"} (${a.score.toFixed(2)})`}
            />
          );
        },
      }),
    );

    return [
      col.accessor("status", {
        id: "status",
        header: () => <span>Status</span>,
        sortingFn: (a, b) => compareStatus(a.original.status, b.original.status),
        cell: ({ row }) => <StatusBadge status={row.original.status} size="xs" />,
      }),
      col.accessor((c) => c.name ?? c.case_key, {
        id: "name",
        header: () => <span>Case</span>,
        cell: ({ row }) => {
          const c = row.original;
          // A repeat index > 0 rides along as a subtle suffix (rather than its
          // own column); repeat 0 / null shows nothing.
          const repeat = c.repeat != null && c.repeat > 0 ? c.repeat : null;
          return (
            <div className="min-w-0">
              <div className="flex min-w-0 items-baseline gap-1">
                <span className="truncate font-medium">
                  {c.name ?? c.case_key}
                </span>
                {repeat != null ? (
                  <span className="shrink-0 font-mono text-[11px] text-muted">
                    #{repeat}
                  </span>
                ) : null}
                {c.cached === true ? (
                  <span
                    className="shrink-0 rounded bg-surface-2 px-1 py-px text-[10px] text-muted"
                    title="Provider response served from cache"
                  >
                    cached
                  </span>
                ) : null}
              </div>
              <div className="truncate font-mono text-[11px] text-muted">
                {c.case_key}
              </div>
            </div>
          );
        },
      }),
      ...(showProvider
        ? [
            col.accessor("provider_id", {
              id: "provider",
              header: () => <span>Provider</span>,
              cell: ({ getValue }) => <IdentCell value={getValue()} />,
            }),
          ]
        : []),
      ...(showPrompt
        ? [
            col.accessor("prompt_id", {
              id: "prompt",
              header: () => <span>Prompt</span>,
              cell: ({ getValue }) => <IdentCell value={getValue()} />,
            }),
          ]
        : []),
      col.accessor("output_preview", {
        id: "preview",
        enableSorting: false,
        header: () => <span>Preview</span>,
        cell: ({ row }) => {
          const c = row.original;
          const text = c.output_preview?.trim();
          // The error fills the gap rather than replacing the preview. A
          // provider failure leaves no output at all, which used to render a
          // bare dash on exactly the rows worth reading; but a case whose
          // output arrived and whose *assertion* errored still has something
          // worth showing, and the real text beats "one or more assertions
          // errored".
          const error = c.error?.trim();
          if (!text && error) {
            return (
              <span
                className="block truncate font-mono text-[11px] text-error"
                title={error}
              >
                {error}
              </span>
            );
          }
          if (!text)
            return <span className="text-muted/50" aria-hidden>–</span>;
          return (
            <span
              className="block truncate font-mono text-[11px] text-muted"
              title={text}
            >
              {text}
            </span>
          );
        },
      }),
      // Next to the preview it explains: an empty output is a *successful*
      // call, so the preview cell above renders a bare dash with no error to
      // fill it, and this is the only column that says why. Sorts on the
      // reason string, which groups a run's refusals together.
      col.accessor((c) => c.empty_reason ?? "", {
        id: "empty_reason",
        header: () => <span>Empty</span>,
        cell: ({ row }) => {
          const reason = row.original.empty_reason;
          // Absence is not "the output was fine": a row written before the
          // column existed reports nothing either way, so this renders the
          // same em-dash every other unknown does rather than a reassuring
          // blank. The reason set is open, so the value is shown verbatim.
          if (!reason)
            return <span className="text-muted/50" aria-hidden>–</span>;
          return (
            <span
              className="block truncate font-mono text-[11px] text-amber"
              title={`Output had nothing gradeable in it: ${reason}`}
            >
              {reason}
            </span>
          );
        },
      }),
      // The combined strip: only this case's own assertions, so the cell is
      // dense instead of a row of em-dashes. One column instead of N.
      col.display({
        id: "asserts",
        header: () => <span>Asserts</span>,
        cell: ({ row }) => {
          const list = row.original.asserts;
          if (list.length === 0)
            return (
              <span className="text-muted/50" aria-hidden>
                –
              </span>
            );
          return (
            <div className="flex flex-wrap items-center gap-1">
              {list.map((a, i) => (
                <AssertDot
                  key={`${a.label}-${i}`}
                  passed={a.passed}
                  label={`${a.label} · ${a.kind} · ${a.passed ? "passed" : "failed"} (${a.score.toFixed(2)})`}
                />
              ))}
            </div>
          );
        },
      }),
      ...assertCols,
      col.accessor((c) => (c.prompt_tokens ?? 0) + (c.completion_tokens ?? 0), {
        id: "tokens",
        header: () => <span>Tokens</span>,
        cell: ({ getValue }) => (
          <span className="block text-right tabular-nums text-muted">
            {formatTokens(getValue())}
          </span>
        ),
      }),
      col.accessor("cost_usd", {
        id: "cost",
        header: () => <span>Cost</span>,
        cell: ({ getValue }) => (
          <span className="block text-right tabular-nums text-muted">
            {formatCost(getValue())}
          </span>
        ),
      }),
      col.accessor("latency_ms", {
        id: "latency",
        header: () => <span>Latency</span>,
        cell: ({ getValue }) => (
          <span className="block text-right tabular-nums text-muted">
            {formatLatency(getValue())}
          </span>
        ),
      }),
      col.accessor("score", {
        id: "score",
        header: () => <span>Score</span>,
        cell: ({ getValue }) => {
          const v = getValue();
          return (
            <span className="block text-right tabular-nums text-muted">
              {v == null ? "–" : v.toFixed(2)}
            </span>
          );
        },
      }),
    ];
  }, [assertLabels, showProvider, showPrompt]);

  const [visibility, setVisibility] = useState<ColumnVisibility>(() =>
    loadColumnVisibility(),
  );

  function setColumnVisible(id: string, visible: boolean) {
    setVisibility((prev) => {
      const next = { ...prev, [id]: visible };
      saveColumnVisibility(next);
      return next;
    });
  }

  function resetColumns() {
    setVisibility({});
    saveColumnVisibility({});
  }

  const visibleColumns = useMemo(
    () => columns.filter((c) => isVisible(c.id ?? "", visibility)),
    [columns, visibility],
  );

  const pickable = useMemo<PickableColumn[]>(
    () =>
      columns
        .map((c) => c.id ?? "")
        .filter((id) => id && !ALWAYS_VISIBLE.has(id))
        .map((id) => ({
          id,
          label: isAssertColumn(id) ? id.slice("assert:".length) : COLUMN_LABEL[id] ?? id,
          group: isAssertColumn(id) ? ("assertions" as const) : ("columns" as const),
        })),
    [columns],
  );

  const table = useReactTable({
    data: cases,
    columns: visibleColumns,
    state: { sorting },
    onSortingChange,
    // Single-column sort, ascending-first for every column (so a numeric column
    // does not default to descending) — keeps the header cycle asc -> desc ->
    // clear and the `?sort=` encoding in lockstep.
    enableMultiSort: false,
    sortDescFirst: false,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
  });

  const rows = table.getRowModel().rows;
  const parentRef = useRef<HTMLDivElement>(null);

  const rowVirtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 44,
    overscan: 14,
  });

  // Both the header and every body row lay out on this one template, derived
  // from the live column ids so provider/prompt columns (when present) shift
  // everything after them consistently.
  const columnIds = useMemo(
    () => visibleColumns.map((c) => c.id ?? ""),
    [visibleColumns],
  );
  const gridTemplate = useMemo(
    () => columnIds.map((id) => spec(id).track).join(" "),
    [columnIds],
  );
  // + the container's own horizontal padding: without it the intrinsic width is
  // under-reported and the last column is clipped rather than scrolled to.
  const minWidth = useMemo(
    () => columnIds.reduce((sum, id) => sum + spec(id).min, 0) + GRID_INSET,
    [columnIds],
  );

  const virtualItems = rowVirtualizer.getVirtualItems();
  const rowCount = rows.length;

  // Infinite-scroll trigger: fetch the next page when the last rendered row is
  // within 20 of the end. Runs in an effect (never during render).
  const lastIndex = virtualItems[virtualItems.length - 1]?.index ?? -1;
  useEffect(() => {
    if (hasNextPage && !isFetchingNextPage && lastIndex >= rowCount - 20) {
      fetchNextPage?.();
    }
  }, [lastIndex, hasNextPage, isFetchingNextPage, rowCount, fetchNextPage]);

  return (
    <div className="flex flex-col gap-2 lg:min-h-0 lg:flex-1">
      {/* Above the grid, not in its footer: the count is unreachable at the
          bottom of a 68vh scroller without scrolling past every row, and this
          is also where the pending state belongs. */}
      <div className="flex shrink-0 items-center justify-between gap-3">
        <p
          role="status"
          aria-live="polite"
          className="flex items-center gap-2 text-xs text-muted"
        >
          {formatCaseCount(rows.length, totalCount, hasNextPage ?? false)}
          {hasNextPage ? (
            <span className="text-muted/70">(sorted within loaded cases)</span>
          ) : null}
          {busy ? (
            <span className="flex items-center gap-1.5">
              <Spinner /> Updating…
            </span>
          ) : null}
        </p>
        <CaseColumnPicker
          columns={pickable}
          visibility={visibility}
          onChange={setColumnVisible}
          onReset={resetColumns}
        />
      </div>

      <div className="flex flex-col overflow-hidden rounded-xl border border-border bg-surface lg:min-h-0 lg:flex-1">
        <div
          ref={parentRef}
          // Sized by the shell rather than by a guessed `vh` fraction: the
          // page no longer scrolls behind this, so it can simply take what is
          // left of the viewport.
          className={cn(
            // Bounded on short/narrow viewports (the page scrolls there);
            // sized by the shell at `lg`, where the page does not.
            "max-h-[70vh] overflow-auto overscroll-contain lg:max-h-none lg:min-h-0 lg:flex-1",
            busy && "opacity-60 transition-opacity",
          )}
          role="grid"
          aria-busy={busy}
          // `-1` means "unknown" — required while more pages may follow, since
          // the loaded count is not the total.
          aria-rowcount={hasNextPage ? -1 : rows.length + 1}
        >
        <div style={{ minWidth }}>
          {/* Header */}
          <div
            className="sticky top-0 z-20 grid items-center border-b border-border bg-surface-2 px-3 py-2 text-xs font-medium uppercase tracking-wide text-muted"
            style={{ gridTemplateColumns: gridTemplate }}
            role="row"
            aria-rowindex={1}
          >
            {table.getFlatHeaders().map((header) => {
              const canSort = header.column.getCanSort();
              const sorted = header.column.getIsSorted(); // false | "asc" | "desc"
              const s = spec(header.column.id);
              const numeric = s.numeric ?? false;
              return (
                <div
                  key={header.id}
                  className={cn(
                    "min-w-0 px-1",
                    // The pinned cell must carry its own background or the
                    // columns scrolling underneath show through it.
                    s.sticky && "sticky left-0 z-10 bg-surface-2",
                  )}
                  role="columnheader"
                  aria-sort={
                    !canSort
                      ? undefined
                      : sorted === "asc"
                        ? "ascending"
                        : sorted === "desc"
                          ? "descending"
                          : "none"
                  }
                >
                  {canSort ? (
                    <button
                      type="button"
                      onClick={header.column.getToggleSortingHandler()}
                      className={cn(
                        "inline-flex w-full min-w-0 items-center gap-1 rounded uppercase tracking-wide transition-colors hover:text-fg focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
                        numeric ? "justify-end" : "justify-start",
                      )}
                    >
                      <span className="truncate">
                        {flexRender(
                          header.column.columnDef.header,
                          header.getContext(),
                        )}
                      </span>
                      <SortArrow dir={sorted} />
                    </button>
                  ) : (
                    <div className="truncate">
                      {flexRender(
                        header.column.columnDef.header,
                        header.getContext(),
                      )}
                    </div>
                  )}
                </div>
              );
            })}
          </div>

          {/* Virtualized body */}
          <div
            style={{ height: rowVirtualizer.getTotalSize(), position: "relative" }}
          >
            {virtualItems.map((vi) => {
              const row = rows[vi.index];
              if (!row) return null;
              const c = row.original;
              const selected = c.case_key === selectedKey;
              return (
                <div
                  key={row.id}
                  role="row"
                  // +2: 1-based, and the header occupies row 1. Required
                  // alongside aria-rowcount — only ~20 of N rows exist in the
                  // DOM, so position must be stated, not inferred.
                  aria-rowindex={vi.index + 2}
                  aria-selected={selected}
                  tabIndex={0}
                  onClick={() => onSelect(c.case_key)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      onSelect(c.case_key);
                    }
                  }}
                  className={cn(
                    "absolute left-0 grid cursor-pointer items-center border-b border-border/50 px-3 text-sm outline-none",
                    // An explicit base background is what lets the pinned cell
                    // inherit something opaque.
                    "bg-surface hover:bg-surface-2 focus-visible:bg-surface-2 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
                    selected && "bg-accent/8 hover:bg-accent/10",
                  )}
                  style={{
                    top: 0,
                    transform: `translateY(${vi.start}px)`,
                    height: vi.size,
                    width: "100%",
                    gridTemplateColumns: gridTemplate,
                  }}
                >
                  {row.getVisibleCells().map((cell) => (
                    <div
                      key={cell.id}
                      className={cn(
                        "min-w-0 px-1",
                        spec(cell.column.id).sticky &&
                          "sticky left-0 z-10 bg-inherit",
                      )}
                      role="gridcell"
                    >
                      {flexRender(cell.column.columnDef.cell, cell.getContext())}
                    </div>
                  ))}
                </div>
              );
            })}
          </div>
        </div>
        </div>
        {isFetchingNextPage ? (
          <div className="border-t border-border px-3 py-1.5 text-xs text-muted">
            Loading more…
          </div>
        ) : null}
      </div>
    </div>
  );
}

/** Small mono cell for the provider/prompt identity columns; a faint dash when
 *  the value is absent. */
function IdentCell({ value }: { value: string | null }) {
  if (!value) return <span className="text-muted/50" aria-hidden>–</span>;
  return (
    <span className="block truncate font-mono text-[11px] text-muted" title={value}>
      {value}
    </span>
  );
}

/** Sort-direction indicator: solid arrow when active, faint glyph otherwise. */
function SortArrow({ dir }: { dir: false | "asc" | "desc" }) {
  return (
    <span
      aria-hidden
      className={cn(
        "shrink-0 text-[10px] leading-none",
        dir ? "text-fg" : "text-muted opacity-40",
      )}
    >
      {dir === "asc" ? "↑" : dir === "desc" ? "↓" : "↕"}
    </span>
  );
}
