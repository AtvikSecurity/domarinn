import { useEffect, useMemo, useRef } from "react";
import {
  createColumnHelper,
  flexRender,
  getCoreRowModel,
  useReactTable,
  type ColumnDef,
} from "@tanstack/react-table";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { CaseRow } from "@/api/types";
import { AssertDot, StatusBadge } from "@/components/StatusBadge";
import { Tooltip } from "@/components/ui/Tooltip";
import { formatCost, formatLatency, formatTokens } from "@/lib/format";
import { cn } from "@/lib/cn";

const NAME_MIN = 300;
const ASSERT_W = 92;
const NUM_W = 84;

interface CaseGridProps {
  cases: CaseRow[];
  assertLabels: string[];
  selectedKey?: string;
  onSelect: (caseKey: string) => void;
  hasNextPage?: boolean;
  fetchNextPage?: () => void;
  isFetchingNextPage?: boolean;
  totalCount?: number;
}

const col = createColumnHelper<CaseRow>();

export function CaseGrid({
  cases,
  assertLabels,
  selectedKey,
  onSelect,
  hasNextPage,
  fetchNextPage,
  isFetchingNextPage,
  totalCount,
}: CaseGridProps) {
  const columns = useMemo<ColumnDef<CaseRow, any>[]>(() => {
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
      col.accessor("name", {
        id: "name",
        header: () => <span>Case</span>,
        cell: ({ row }) => {
          const c = row.original;
          return (
            <div className="flex min-w-0 items-center gap-2">
              <StatusBadge status={c.status} size="xs" />
              <div className="min-w-0">
                <div className="truncate font-medium">{c.name ?? c.case_key}</div>
                <div className="truncate font-mono text-[11px] text-muted">
                  {c.case_key}
                </div>
              </div>
            </div>
          );
        },
      }),
      ...assertCols,
      col.display({
        id: "tokens",
        header: () => <span className="block text-right">Tokens</span>,
        cell: ({ row }) => (
          <span className="block text-right tabular-nums text-muted">
            {formatTokens(
              (row.original.prompt_tokens ?? 0) +
                (row.original.completion_tokens ?? 0),
            )}
          </span>
        ),
      }),
      col.accessor("cost_usd", {
        id: "cost",
        header: () => <span className="block text-right">Cost</span>,
        cell: ({ getValue }) => (
          <span className="block text-right tabular-nums text-muted">
            {formatCost(getValue() as number | undefined)}
          </span>
        ),
      }),
      col.accessor("latency_ms", {
        id: "latency",
        header: () => <span className="block text-right">Latency</span>,
        cell: ({ getValue }) => (
          <span className="block text-right tabular-nums text-muted">
            {formatLatency(getValue() as number)}
          </span>
        ),
      }),
    ];
  }, [assertLabels]);

  const table = useReactTable({
    data: cases,
    columns,
    getCoreRowModel: getCoreRowModel(),
  });

  const rows = table.getRowModel().rows;
  const parentRef = useRef<HTMLDivElement>(null);

  const rowVirtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 44,
    overscan: 14,
  });

  const gridTemplate = useMemo(
    () =>
      `minmax(${NAME_MIN}px, 1.7fr) repeat(${assertLabels.length}, ${ASSERT_W}px) ${NUM_W}px ${NUM_W}px ${NUM_W}px`,
    [assertLabels.length],
  );
  const minWidth = NAME_MIN + assertLabels.length * ASSERT_W + NUM_W * 3;

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
            {table.getFlatHeaders().map((header) => (
              <div key={header.id} className="truncate px-1" role="columnheader">
                {flexRender(header.column.columnDef.header, header.getContext())}
              </div>
            ))}
          </div>

          {/* Virtualized body */}
          <div
            style={{ height: rowVirtualizer.getTotalSize(), position: "relative" }}
          >
            {virtualItems.map((vi) => {
              const row = rows[vi.index];
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
        </span>
        {isFetchingNextPage ? <span>Loading more…</span> : null}
      </div>
    </div>
  );
}
