import { useEffect, useMemo, useRef } from "react";
import {
  flexRender,
  getCoreRowModel,
  useReactTable,
  type ColumnDef,
  type SortingState,
} from "@tanstack/react-table";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { CacheEntryListItem } from "@/api";
import { Chip } from "@/components/ui/Chip";
import { CopyButton } from "@/components/ui/CopyButton";
import { Spinner } from "@/components/ui/Spinner";
import { Tooltip } from "@/components/ui/Tooltip";
import { cn } from "@/lib/cn";
import {
  formatBytes,
  formatCost,
  formatDateAbsolute,
  formatRelative,
  formatTokens,
  shortCacheKey,
} from "@/lib/format";

/**
 * A column's track, its intrinsic minimum, and how it behaves.
 *
 * The same shape as the case grid's spec, and deliberately a copy rather than a
 * shared abstraction: only about 160 of that component's 600 lines are generic,
 * it is the most e2e-covered component in the app, and the two grids differ
 * where it counts — no status column to pin, no assert columns, no picker, and
 * a server-side sort. Two instances is not evidence for an abstraction.
 */
interface ColumnSpec {
  track: string;
  min: number;
  numeric?: boolean;
  sticky?: boolean;
}

/** Horizontal padding the grid reserves outside the tracks. */
const GRID_INSET = 24;

const COLUMN_SPEC: Record<string, ColumnSpec> = {
  key: { track: "minmax(200px, 1fr)", min: 200, sticky: true },
  kind: { track: "104px", min: 104 },
  model: { track: "minmax(160px, 1fr)", min: 160 },
  summary: { track: "minmax(220px, 1.6fr)", min: 220 },
  tokens: { track: "84px", min: 84, numeric: true },
  cost: { track: "84px", min: 84, numeric: true },
  size: { track: "84px", min: 84, numeric: true },
  created: { track: "112px", min: 112, numeric: true },
  last_access: { track: "112px", min: 112, numeric: true },
};

function spec(id: string): ColumnSpec {
  return COLUMN_SPEC[id] ?? { track: "minmax(120px, 1fr)", min: 120 };
}

/**
 * Columns the server can order by — the three literal indexed columns, plus
 * cost.
 *
 * Everything else a row shows lives inside the entry body. Sorting one page of
 * a large cache by a column the database cannot see would order the loaded rows
 * and quietly claim to have ordered the cache, which is worse than not offering
 * it: the case grid has to apologise for exactly that with "sorted within
 * loaded cases", and here the apology would be a lie rather than a caveat.
 */
const SORTABLE = new Set(["size", "created", "last_access", "cost"]);

/** A muted placeholder for a value the entry never carried. */
function Absent() {
  return (
    <span aria-hidden="true" className="text-muted/50">
      –
    </span>
  );
}

/** What a row shows before the backfill has looked at it. */
function Indexing() {
  return (
    <span className="text-muted/70" title="Not indexed yet">
      indexing…
    </span>
  );
}

function metaCell(row: CacheEntryListItem, value: React.ReactNode) {
  if (!row.indexed) return <Indexing />;
  return value;
}

export interface CacheEntryGridProps {
  entries: CacheEntryListItem[];
  busy?: boolean;
  hasNextPage?: boolean;
  isFetchingNextPage?: boolean;
  fetchNextPage?: () => void;
  sorting: SortingState;
  onSortingChange: (next: SortingState) => void;
  selectedKey?: string;
  onSelect: (key: string) => void;
}

export function CacheEntryGrid({
  entries,
  busy = false,
  hasNextPage = false,
  isFetchingNextPage = false,
  fetchNextPage,
  sorting,
  onSortingChange,
  selectedKey,
  onSelect,
}: CacheEntryGridProps) {
  const parentRef = useRef<HTMLDivElement>(null);

  const columns = useMemo<ColumnDef<CacheEntryListItem>[]>(
    () => [
      {
        id: "key",
        header: "Key",
        enableSorting: false,
        cell: ({ row }) => <KeyCell entry={row.original} />,
      },
      {
        id: "kind",
        header: "Kind",
        enableSorting: false,
        cell: ({ row }) =>
          metaCell(
            row.original,
            row.original.kind ? (
              // One tone for every kind. A per-kind colour needs a design token
              // the contrast test does not cover, and five hues for a nominal
              // facet is noise rather than information.
              <Chip tone="neutral" size="xs" mono>
                {row.original.kind}
              </Chip>
            ) : row.original.parseable === false ? (
              <Chip tone="amber" size="xs" title="This server could not read this entry">
                opaque
              </Chip>
            ) : (
              <Absent />
            ),
          ),
      },
      {
        id: "model",
        header: "Model",
        enableSorting: false,
        cell: ({ row }) =>
          metaCell(row.original, row.original.model ?? <Absent />),
      },
      {
        id: "summary",
        header: "Request",
        enableSorting: false,
        cell: ({ row }) =>
          metaCell(row.original, row.original.request_summary ?? <Absent />),
      },
      {
        id: "tokens",
        header: "Tokens",
        enableSorting: false,
        cell: ({ row }) => {
          const { input_tokens, output_tokens } = row.original;
          if (input_tokens === null && output_tokens === null) {
            return metaCell(row.original, <Absent />);
          }
          return formatTokens((input_tokens ?? 0) + (output_tokens ?? 0));
        },
      },
      {
        id: "cost",
        header: "Cost",
        // The sortable columns carry an accessor even though the ordering
        // happens server-side: react-table's `getCanSort()` requires one, so a
        // display-only column silently has no sort handler at all.
        accessorFn: (row) => row.cost_usd,
        cell: ({ row }) =>
          row.original.cost_usd === null ? (
            metaCell(row.original, <Absent />)
          ) : (
            <>{formatCost(row.original.cost_usd)}</>
          ),
      },
      {
        id: "size",
        header: "Size",
        accessorFn: (row) => row.size,
        cell: ({ row }) => formatBytes(row.original.size),
      },
      {
        id: "created",
        header: "Created",
        accessorFn: (row) => row.created_at,
        cell: ({ row }) => (
          <Tooltip content={formatDateAbsolute(row.original.created_at)}>
            <span>{formatRelative(row.original.created_at)}</span>
          </Tooltip>
        ),
      },
      {
        id: "last_access",
        header: "Last used",
        accessorFn: (row) => row.last_access_at,
        cell: ({ row }) =>
          row.original.last_access_at ? (
            <Tooltip content={formatDateAbsolute(row.original.last_access_at)}>
              <span>{formatRelative(row.original.last_access_at)}</span>
            </Tooltip>
          ) : (
            <Absent />
          ),
      },
    ],
    [],
  );

  const table = useReactTable({
    data: entries,
    columns,
    state: { sorting },
    // The server does the ordering. Without this react-table would re-sort the
    // loaded page on top of the server's answer and the two would disagree at
    // every page boundary.
    manualSorting: true,
    enableMultiSort: false,
    sortDescFirst: true,
    onSortingChange: (updater) => {
      const next = typeof updater === "function" ? updater(sorting) : updater;
      onSortingChange(next);
    },
    getCoreRowModel: getCoreRowModel(),
  });

  const rows = table.getRowModel().rows;
  const columnIds = table.getAllLeafColumns().map((c) => c.id);
  const gridTemplate = columnIds.map((id) => spec(id).track).join(" ");
  const minWidth = useMemo(
    () => columnIds.reduce((sum, id) => sum + spec(id).min, 0) + GRID_INSET,
    [columnIds],
  );

  const rowVirtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    // Exact, not an estimate: every cell below is single-line and truncated, so
    // no row can grow. There is no `measureElement`, and a row that wrapped
    // would desync the scrollbar from the content in a way that reads as "the
    // app is broken" and bisects badly.
    estimateSize: () => 44,
    overscan: 14,
  });

  const virtualItems = rowVirtualizer.getVirtualItems();
  const rowCount = rows.length;
  const lastIndex = virtualItems[virtualItems.length - 1]?.index ?? -1;
  useEffect(() => {
    if (hasNextPage && !isFetchingNextPage && lastIndex >= rowCount - 20) {
      fetchNextPage?.();
    }
  }, [lastIndex, hasNextPage, isFetchingNextPage, rowCount, fetchNextPage]);

  return (
    <div className="flex flex-col gap-2 lg:min-h-0 lg:flex-1">
      <p
        role="status"
        aria-live="polite"
        className="flex shrink-0 items-center gap-2 text-xs text-muted"
      >
        {rows.length} {rows.length === 1 ? "entry" : "entries"}
        {hasNextPage ? " loaded" : ""}
        {busy ? (
          <span className="flex items-center gap-1.5">
            <Spinner /> Updating…
          </span>
        ) : null}
      </p>

      <div className="flex flex-col overflow-hidden rounded-xl border border-border bg-surface lg:min-h-0 lg:flex-1">
        <div
          ref={parentRef}
          className={cn(
            "max-h-[70vh] overflow-auto overscroll-contain lg:max-h-none lg:min-h-0 lg:flex-1",
            busy && "opacity-60 transition-opacity",
          )}
          role="grid"
          aria-busy={busy}
          aria-rowcount={hasNextPage ? -1 : rows.length + 1}
        >
          <div style={{ minWidth }}>
            <div
              className="sticky top-0 z-20 grid items-center border-b border-border bg-surface-2 px-3 py-2 text-xs font-medium uppercase tracking-wide text-muted"
              style={{ gridTemplateColumns: gridTemplate }}
              role="row"
              aria-rowindex={1}
            >
              {table.getFlatHeaders().map((header) => {
                const id = header.column.id;
                const canSort = SORTABLE.has(id);
                const sorted = header.column.getIsSorted();
                const s = spec(id);
                const label = flexRender(
                  header.column.columnDef.header,
                  header.getContext(),
                );
                return (
                  <div
                    key={header.id}
                    className={cn(
                      "min-w-0 px-1",
                      s.sticky && "sticky left-0 z-10 bg-surface-2",
                      s.numeric && "text-right",
                    )}
                    role="columnheader"
                    // Only on columns that can actually sort: `aria-sort="none"`
                    // on a fixed column announces an affordance that is not there.
                    aria-sort={
                      canSort
                        ? sorted === "asc"
                          ? "ascending"
                          : sorted === "desc"
                            ? "descending"
                            : "none"
                        : undefined
                    }
                  >
                    {canSort ? (
                      <button
                        type="button"
                        className="flex w-full items-center gap-1 truncate hover:text-fg"
                        style={{ justifyContent: s.numeric ? "flex-end" : undefined }}
                        onClick={header.column.getToggleSortingHandler()}
                      >
                        <span className="truncate">{label}</span>
                        <span aria-hidden="true" className="text-[10px]">
                          {sorted === "asc" ? "▲" : sorted === "desc" ? "▼" : "↕"}
                        </span>
                      </button>
                    ) : (
                      <div className="truncate">{label}</div>
                    )}
                  </div>
                );
              })}
            </div>

            <div
              style={{ height: rowVirtualizer.getTotalSize(), position: "relative" }}
            >
              {virtualItems.map((vi) => {
                const row = rows[vi.index];
                if (!row) return null;
                const entry = row.original;
                const selected = entry.key === selectedKey;
                return (
                  <div
                    key={entry.key}
                    role="row"
                    aria-rowindex={vi.index + 2}
                    aria-selected={selected}
                    // A truncated hash is not announceable on its own, so the
                    // row carries what it is actually about.
                    aria-label={`Cache entry ${shortCacheKey(entry.key)}${
                      entry.model ? `, ${entry.model}` : ""
                    }`}
                    tabIndex={0}
                    onClick={() => onSelect(entry.key)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        e.preventDefault();
                        onSelect(entry.key);
                      }
                    }}
                    className={cn(
                      "absolute left-0 top-0 grid w-full cursor-pointer items-center border-b border-border/60 px-3 text-sm outline-none",
                      "hover:bg-surface-2 focus-visible:ring-2 focus-visible:ring-ring",
                      selected && "bg-surface-2",
                    )}
                    style={{
                      height: vi.size,
                      transform: `translateY(${vi.start}px)`,
                      gridTemplateColumns: gridTemplate,
                    }}
                  >
                    {row.getVisibleCells().map((cell) => {
                      const s = spec(cell.column.id);
                      return (
                        <div
                          key={cell.id}
                          role="gridcell"
                          className={cn(
                            "min-w-0 truncate px-1",
                            s.numeric && "text-right tabular-nums",
                            s.sticky &&
                              cn(
                                "sticky left-0 z-10",
                                selected ? "bg-surface-2" : "bg-surface",
                              ),
                          )}
                        >
                          {flexRender(cell.column.columnDef.cell, cell.getContext())}
                        </div>
                      );
                    })}
                  </div>
                );
              })}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

function KeyCell({ entry }: { entry: CacheEntryListItem }) {
  return (
    <div className="flex min-w-0 items-center gap-1">
      <span className="truncate font-mono text-[11px]" title={entry.key}>
        <span className="text-muted/70">sha256:</span>
        {shortCacheKey(entry.key)}
      </span>
      <span className="sr-only">{entry.key}</span>
      {/* The click must not also open the drawer; the row's handler is on an
          ancestor, so it is stopped here rather than inside the button. */}
      <span onClick={(e) => e.stopPropagation()}>
        <CopyButton value={entry.key} iconOnly label="Copy cache key" tabIndex={-1} />
      </span>
    </div>
  );
}
