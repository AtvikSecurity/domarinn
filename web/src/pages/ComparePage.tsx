import { useMemo, useRef, useState } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useCompare, useRun, useRuns } from "@/api/queries";
import type { CaseStatus, CompareRow, CompareSummary } from "@/api/types";
import { DELTA_LABEL } from "@/lib/compare";
import { mergeParams } from "@/lib/filters";
import { StatusBadge } from "@/components/StatusBadge";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { ErrorState, EmptyState } from "@/components/States";
import { cn } from "@/lib/cn";
import { CompareRowExpansion } from "./compare/CompareRowExpansion";

type ChipKey =
  | "newly_failing"
  | "newly_passing"
  | "still_failing"
  | "output_changed"
  | "added"
  | "removed";

const CHIP_META: { key: ChipKey; label: string; tone: string }[] = [
  { key: "newly_failing", label: "Newly failing", tone: "text-fail ring-fail/30 bg-fail/8" },
  { key: "newly_passing", label: "Newly passing", tone: "text-pass ring-pass/30 bg-pass/8" },
  { key: "output_changed", label: "Output changed", tone: "text-amber ring-amber/30 bg-amber/8" },
  { key: "still_failing", label: "Still failing", tone: "text-error ring-error/30 bg-error/8" },
  { key: "added", label: "Added", tone: "text-muted ring-border bg-surface-2" },
  { key: "removed", label: "Removed", tone: "text-muted ring-border bg-surface-2" },
];

export function ComparePage() {
  const { id = "", other } = useParams();
  const [params, setParams] = useSearchParams();
  const navigate = useNavigate();
  const activeDelta = params.get("delta") as ChipKey | null;

  const run = useRun(id);
  const compare = useCompare(id, other);
  const suiteRuns = useRuns({
    project: run.data?.project,
    suite: run.data?.suite,
  });

  const rows = useMemo(() => {
    const all = compare.data?.cases ?? [];
    if (!activeDelta) return all;
    if (activeDelta === "output_changed") return all.filter((r) => r.output_changed);
    return all.filter((r) => r.delta === activeDelta);
  }, [compare.data, activeDelta]);

  function setDelta(key: ChipKey) {
    setParams(
      mergeParams(params, { delta: activeDelta === key ? undefined : key }),
      { replace: true },
    );
  }

  if (compare.isPending) return <CenteredSpinner label="Computing comparison…" />;
  if (compare.isError)
    return <ErrorState error={compare.error} onRetry={() => compare.refetch()} />;

  const { base, head, summary } = compare.data;
  const runOptions = suiteRuns.data?.pages.flatMap((p) => p.runs) ?? [];

  return (
    <div className="space-y-5">
      <div className="rounded-xl border border-border bg-surface p-4">
        <div className="flex flex-wrap items-center gap-3">
          <Link to={`/runs/${encodeURIComponent(id)}`} className="text-sm text-muted hover:text-fg">
            ← Back to run
          </Link>
          <h1 className="text-lg font-semibold tracking-tight">Compare</h1>
        </div>
        <div className="mt-2 flex flex-wrap items-center gap-2 text-sm">
          <span className="text-muted">base</span>
          <select
            aria-label="Base run"
            className="h-8 rounded-md border border-border bg-surface px-2 font-mono text-xs outline-none focus:ring-2 focus:ring-ring"
            value={base.id}
            onChange={(e) =>
              navigate(
                `/runs/${encodeURIComponent(head.id)}/compare/${encodeURIComponent(e.target.value)}`,
              )
            }
          >
            {runOptions.length === 0 ? (
              <option value={base.id}>{base.id}</option>
            ) : (
              runOptions.map((r) => (
                <option key={r.id} value={r.id} disabled={r.id === head.id}>
                  {r.id}
                </option>
              ))
            )}
          </select>
          <span className="text-muted">→ head</span>
          <select
            aria-label="Head run"
            className="h-8 rounded-md border border-border bg-surface px-2 font-mono text-xs outline-none focus:ring-2 focus:ring-ring"
            value={head.id}
            onChange={(e) =>
              navigate(
                `/runs/${encodeURIComponent(e.target.value)}/compare/${encodeURIComponent(base.id)}`,
              )
            }
          >
            {runOptions.length === 0 ? (
              <option value={head.id}>{head.id}</option>
            ) : (
              runOptions.map((r) => (
                <option key={r.id} value={r.id} disabled={r.id === base.id}>
                  {r.id}
                </option>
              ))
            )}
          </select>
          {base.id === head.id ? (
            <span className="text-xs text-amber">
              Pick two different runs to compare.
            </span>
          ) : null}
        </div>
      </div>

      {/* Summary chips (filter the grid) */}
      <div className="flex flex-wrap gap-2">
        {CHIP_META.map((chip) => (
          <button
            key={chip.key}
            onClick={() => setDelta(chip.key)}
            aria-pressed={activeDelta === chip.key}
            className={cn(
              "inline-flex items-center gap-1.5 rounded-full px-3 py-1 text-sm font-medium ring-1 ring-inset transition-all",
              chip.tone,
              activeDelta === chip.key
                ? "ring-2 ring-offset-1 ring-offset-bg"
                : "opacity-90 hover:opacity-100",
              activeDelta && activeDelta !== chip.key ? "opacity-40" : "",
            )}
          >
            {chip.label}
            <span className="tabular-nums">{summaryCount(summary, chip.key)}</span>
          </button>
        ))}
        {activeDelta ? (
          <button
            onClick={() => setParams(mergeParams(params, { delta: undefined }), { replace: true })}
            className="rounded-full px-3 py-1 text-sm text-muted hover:text-fg"
          >
            Clear filter
          </button>
        ) : null}
      </div>

      {rows.length === 0 ? (
        <EmptyState title="No cases in this delta group" />
      ) : (
        <DeltaTable baseId={base.id} headId={head.id} rows={rows} />
      )}
    </div>
  );
}

function summaryCount(summary: CompareSummary, key: ChipKey): number {
  return summary[key];
}

function StatusCell({ status }: { status: CaseStatus | null }) {
  if (status === null) return <span className="text-xs text-muted">—</span>;
  return <StatusBadge status={status} size="xs" />;
}

const deltaTone: Record<string, string> = {
  newly_failing: "text-fail",
  newly_passing: "text-pass",
  still_failing: "text-error",
  added: "text-muted",
  removed: "text-muted",
  unchanged: "text-muted",
};

function DeltaTable({
  baseId,
  headId,
  rows,
}: {
  baseId: string;
  headId: string;
  rows: CompareRow[];
}) {
  const parentRef = useRef<HTMLDivElement>(null);
  const [expanded, setExpanded] = useState<string | null>(null);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 46,
    overscan: 12,
    getItemKey: (i) => rows[i].case_key,
  });

  return (
    <div className="overflow-hidden rounded-xl border border-border bg-surface">
      <div
        className="grid items-center border-b border-border bg-surface-2/95 px-4 py-2 text-xs font-medium uppercase tracking-wide text-muted"
        style={{ gridTemplateColumns: "1.6fr 90px 90px 130px 90px" }}
      >
        <span>Case</span>
        <span>Base</span>
        <span>Head</span>
        <span>Delta</span>
        <span className="text-right">Output</span>
      </div>
      <div ref={parentRef} className="max-h-[64vh] overflow-auto">
        <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
          {virtualizer.getVirtualItems().map((vi) => {
            const row = rows[vi.index];
            const isOpen = expanded === row.case_key;
            return (
              <div
                key={row.case_key}
                data-index={vi.index}
                ref={virtualizer.measureElement}
                className="absolute left-0 w-full border-b border-border/50"
                style={{ top: 0, transform: `translateY(${vi.start}px)` }}
              >
                <button
                  onClick={() =>
                    setExpanded((cur) => (cur === row.case_key ? null : row.case_key))
                  }
                  aria-expanded={isOpen}
                  className={cn(
                    "grid w-full items-center px-4 py-2 text-left text-sm outline-none hover:bg-surface-2 focus-visible:bg-surface-2",
                    isOpen && "bg-surface-2",
                  )}
                  style={{ gridTemplateColumns: "1.6fr 90px 90px 130px 90px" }}
                >
                  <span className="flex min-w-0 items-center gap-2">
                    <svg
                      width="14"
                      height="14"
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="2"
                      className={cn("shrink-0 text-muted transition-transform", isOpen && "rotate-90")}
                    >
                      <path d="M9 6l6 6-6 6" />
                    </svg>
                    <span className="min-w-0">
                      <span className="block truncate font-medium">
                        {row.name ?? row.case_key}
                      </span>
                      <span className="block truncate font-mono text-[11px] text-muted">
                        {row.case_key}
                      </span>
                    </span>
                  </span>
                  <StatusCell status={row.base_status} />
                  <StatusCell status={row.head_status} />
                  <span className={cn("text-xs font-medium", deltaTone[row.delta])}>
                    {DELTA_LABEL[row.delta]}
                  </span>
                  <span className="text-right">
                    {row.output_changed ? (
                      <span className="rounded-full bg-amber/12 px-2 py-0.5 text-[11px] font-medium text-amber ring-1 ring-inset ring-amber/25">
                        changed
                      </span>
                    ) : (
                      <span className="text-[11px] text-muted">same</span>
                    )}
                  </span>
                </button>
                {isOpen ? (
                  <CompareRowExpansion
                    baseRunId={baseId}
                    headRunId={headId}
                    caseKey={row.case_key}
                  />
                ) : null}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
