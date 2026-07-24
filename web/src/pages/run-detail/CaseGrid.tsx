import { useEffect, useMemo, useRef } from "react";
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
import { formatCost, formatLatency, formatTokens } from "@/lib/format";
import { compareStatus } from "@/lib/sort";
import { cn } from "@/lib/cn";

const STATUS_W = 84;
const NAME_MIN = 240;
const PREVIEW_MIN = 220;
const ASSERT_W = 92;
const NUM_W = 84;
const IDENT_W = 132;

/** Right-aligned numeric columns (header justified to the right). */
const NUMERIC_COLS = new Set(["tokens", "cost", "latency", "score"]);

/** Grid-template width token for a column, keyed by its stable id. Driven off
 *  the live column set so the header/body track the same order the table
 *  renders (matrix-only provider/prompt columns shift everything after them). */
function colWidth(id: string): string {
  if (id === "status") return `${STATUS_W}px`;
  if (id === "name") return `minmax(${NAME_MIN}px, 1.2fr)`;
  if (id === "provider" || id === "prompt") return `${IDENT_W}px`;
  if (id === "preview") return `minmax(${PREVIEW_MIN}px, 1.4fr)`;
  if (id.startsWith("assert:")) return `${ASSERT_W}px`;
  return `${NUM_W}px`;
}

/** Min px a column contributes to the grid's intrinsic width. */
function colMinWidth(id: string): number {
  if (id === "status") return STATUS_W;
  if (id === "name") return NAME_MIN;
  if (id === "provider" || id === "prompt") return IDENT_W;
  if (id === "preview") return PREVIEW_MIN;
  if (id.startsWith("assert:")) return ASSERT_W;
  return NUM_W;
}

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
            return <span className="text-muted/50" aria-hidden>–</span>;
          return (
            <AssertDot
              passed={a.passed}
              title={`${label} · ${a.kind} · ${a.passed ? "passed" : "failed"} (${a.score.toFixed(2)})`}
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
        cell: ({ getValue }) => {
          const text = getValue()?.trim();
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

  const table = useReactTable({
    data: cases,
    columns,
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
    () => columns.map((c) => c.id ?? ""),
    [columns],
  );
  const gridTemplate = useMemo(
    () => columnIds.map(colWidth).join(" "),
    [columnIds],
  );
  const minWidth = useMemo(
    () => columnIds.reduce((sum, id) => sum + colMinWidth(id), 0),
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
    <div className="overflow-hidden rounded-xl border border-border bg-surface">
      <div
        ref={parentRef}
        className="max-h-[68vh] overflow-auto"
        role="grid"
        aria-rowcount={totalCount ?? rows.length}
      >
        <div style={{ minWidth }}>
          {/* Header */}
          <div
            className="sticky top-0 z-10 grid items-center border-b border-border bg-surface-2/95 px-3 py-2 text-xs font-medium uppercase tracking-wide text-muted backdrop-blur"
            style={{ gridTemplateColumns: gridTemplate }}
            role="row"
          >
            {table.getFlatHeaders().map((header) => {
              const canSort = header.column.getCanSort();
              const sorted = header.column.getIsSorted(); // false | "asc" | "desc"
              const numeric = NUMERIC_COLS.has(header.column.id);
              return (
                <div
                  key={header.id}
                  className="min-w-0 px-1"
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
                    "hover:bg-surface-2 focus-visible:bg-surface-2 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
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
                    <div key={cell.id} className="min-w-0 px-1" role="gridcell">
                      {flexRender(cell.column.columnDef.cell, cell.getContext())}
                    </div>
                  ))}
                </div>
              );
            })}
          </div>
        </div>
      </div>
      <div className="flex items-center justify-between border-t border-border px-3 py-1.5 text-xs text-muted">
        <span>
          Showing {rows.length}
          {totalCount != null && totalCount > rows.length ? ` of ${totalCount}+` : ""} cases
          {hasNextPage ? (
            <span className="text-muted/70"> (sorted within loaded cases)</span>
          ) : null}
        </span>
        {isFetchingNextPage ? <span>Loading more…</span> : null}
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
