import { useEffect, useMemo, useState } from "react";
import type { CaseStatus, MatrixCell } from "@/api";
import { useMatrixAll } from "@/api/queries";
import {
  cellBucketClass,
  columnGroups,
  distinctPrompts,
  singleCellStatus,
} from "@/lib/matrix";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { ErrorState, EmptyState } from "@/components/States";
import { cn } from "@/lib/cn";
import { MatrixCellPopover } from "./MatrixCellPopover";
import { ProviderCompare } from "./ProviderCompare";

const STATUS_DOT: Record<CaseStatus, string> = {
  pass: "bg-pass",
  fail: "bg-fail",
  error: "bg-error",
  skip: "bg-skip",
};

/**
 * The prompt × provider matrix: rows are tests, columns are providers (grouped
 * under prompt-section headers when the run has more than one prompt). Each cell
 * shows pass@k at a glance and opens a per-repeat popover; a cell's popover can
 * launch the cross-provider compare modal. Rendered as a real `<table>` with row
 * and column headers for assistive tech.
 */
export function MatrixView({
  runId,
  onSelectCase,
}: {
  runId: string;
  onSelectCase: (caseKey: string) => void;
}) {
  const q = useMatrixAll(runId);
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = q;

  // Drain the row pagination so the whole grid is present (rows are small). Runs
  // in an effect, never during render.
  useEffect(() => {
    if (hasNextPage && !isFetchingNextPage) void fetchNextPage();
  }, [hasNextPage, isFetchingNextPage, fetchNextPage]);

  const columns = useMemo(() => q.data?.pages[0]?.columns ?? [], [q.data]);
  const rows = useMemo(
    () => q.data?.pages.flatMap((p) => p.rows) ?? [],
    [q.data],
  );
  const groups = useMemo(() => columnGroups(columns), [columns]);
  const displayCols = useMemo(() => groups.flatMap((g) => g.columns), [groups]);
  const grouped = groups.length > 1;
  const showPrompt = distinctPrompts(q.data?.pages[0]).length > 1;

  // Which (test, prompt) the compare modal shows — component state, not URL.
  const [compare, setCompare] = useState<{ testId: string; promptId: string | null } | null>(
    null,
  );

  if (q.isPending) return <CenteredSpinner label="Loading matrix…" />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  if (rows.length === 0 || displayCols.length === 0) {
    return <EmptyState title="No matrix data for this run" />;
  }

  return (
    <div className="overflow-x-auto rounded-xl border border-border bg-surface">
      <table
        className="w-full border-collapse text-sm"
        aria-label="Prompt by provider matrix"
      >
        <thead>
          {grouped ? (
            <>
              <tr className="border-b border-border">
                <th
                  rowSpan={2}
                  scope="col"
                  className="sticky left-0 z-20 bg-surface-2/95 px-3 py-2 text-left text-xs font-semibold uppercase tracking-wide text-muted backdrop-blur"
                >
                  Test
                </th>
                {groups.map((g) => (
                  <th
                    key={g.promptId ?? "∅"}
                    scope="colgroup"
                    colSpan={g.columns.length}
                    className="border-l border-border bg-surface-2/95 px-2 py-1.5 text-center font-mono text-[11px] font-semibold text-fg"
                  >
                    {g.promptId ?? "—"}
                  </th>
                ))}
              </tr>
              <tr className="border-b border-border">
                {displayCols.map((c, i) => (
                  <th
                    key={`${c.colIndex}-${i}`}
                    scope="col"
                    className={cn(
                      "bg-surface-2/95 px-2 py-1.5 text-center font-mono text-[11px] text-muted",
                      i > 0 && c.promptId !== displayCols[i - 1]?.promptId
                        ? "border-l border-border"
                        : "",
                    )}
                  >
                    {c.providerId}
                  </th>
                ))}
              </tr>
            </>
          ) : (
            <tr className="border-b border-border">
              <th
                scope="col"
                className="sticky left-0 z-20 bg-surface-2/95 px-3 py-2 text-left text-xs font-semibold uppercase tracking-wide text-muted backdrop-blur"
              >
                Test
              </th>
              {displayCols.map((c, i) => (
                <th
                  key={`${c.colIndex}-${i}`}
                  scope="col"
                  className="bg-surface-2/95 px-2 py-1.5 text-center font-mono text-[11px] text-muted"
                >
                  {c.providerId}
                </th>
              ))}
            </tr>
          )}
        </thead>
        <tbody>
          {rows.map((row) => {
            const testLabel = row.name ?? row.test_id;
            return (
              <tr key={row.test_id} className="border-b border-border/60 last:border-b-0">
                <th
                  scope="row"
                  className="sticky left-0 z-10 max-w-[16rem] truncate bg-surface px-3 py-1.5 text-left font-medium"
                  title={testLabel}
                >
                  <span className="block truncate">{testLabel}</span>
                  <span className="block truncate font-mono text-[10px] font-normal text-muted">
                    {row.test_id}
                  </span>
                </th>
                {displayCols.map((c, i) => {
                  const cell = row.cells[c.colIndex] ?? null;
                  const borderLeft =
                    grouped && i > 0 && c.promptId !== displayCols[i - 1]?.promptId;
                  return (
                    <td
                      key={`${c.colIndex}-${i}`}
                      className={cn("p-1 text-center", borderLeft && "border-l border-border")}
                    >
                      <MatrixCellView
                        runId={runId}
                        cell={cell}
                        testId={row.test_id}
                        testLabel={testLabel}
                        colIndex={c.colIndex}
                        providerId={c.providerId}
                        promptId={c.promptId}
                        onSelectCase={onSelectCase}
                        onCompare={() =>
                          setCompare({ testId: row.test_id, promptId: c.promptId })
                        }
                      />
                    </td>
                  );
                })}
              </tr>
            );
          })}
        </tbody>
      </table>

      <ProviderCompare
        open={compare !== null}
        onOpenChange={(o) => !o && setCompare(null)}
        runId={runId}
        columns={columns}
        rows={rows}
        testId={compare?.testId ?? null}
        promptId={compare?.promptId ?? null}
        showPrompt={showPrompt}
        onNavigate={(testId) =>
          setCompare((c) => (c ? { ...c, testId } : c))
        }
      />
    </div>
  );
}

/** A single matrix cell: an em-dash for a missing cell, a status dot for a
 *  single run, or a `passed/total` tile (background stepped by pass fraction)
 *  wrapped in its popover for repeated cells. */
function MatrixCellView({
  runId,
  cell,
  testId,
  testLabel,
  colIndex,
  providerId,
  promptId,
  onSelectCase,
  onCompare,
}: {
  runId: string;
  cell: MatrixCell | null;
  testId: string;
  testLabel: string;
  colIndex: number;
  providerId: string;
  promptId: string | null;
  onSelectCase: (caseKey: string) => void;
  onCompare: () => void;
}) {
  if (!cell) {
    return (
      <span className="text-muted/40" aria-label="no data">
        —
      </span>
    );
  }

  const single = cell.total === 1;
  const flake = cell.distinct_outputs > 1 && cell.pass_fraction >= 1;
  const label =
    `${testLabel}, ${providerId}${promptId != null ? ` · ${promptId}` : ""}: ` +
    (single
      ? singleCellStatus(cell)
      : `${cell.passed} of ${cell.total} passed`);

  const trigger = (
    <button
      type="button"
      data-cell={`${testId}:${colIndex}`}
      aria-label={label}
      className={cn(
        "flex min-h-[2.25rem] w-full flex-col items-center justify-center gap-0 rounded px-1 py-1 text-xs font-medium tabular-nums transition-shadow",
        "hover:ring-2 hover:ring-ring focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
        single ? "" : cellBucketClass(cell.pass_fraction),
      )}
    >
      {single ? (
        <span
          className={cn("size-2.5 rounded-full", STATUS_DOT[singleCellStatus(cell)])}
          aria-hidden
        />
      ) : (
        <>
          <span className="flex items-center gap-0.5">
            <span>
              {cell.passed}/{cell.total}
            </span>
            {flake ? (
              <span
                className="text-amber"
                aria-hidden
                title={`${cell.distinct_outputs} distinct outputs across repeats`}
              >
                ~
              </span>
            ) : null}
          </span>
          {cell.score_mean != null ? (
            <span className="text-[10px] font-normal text-muted">
              {cell.score_mean.toFixed(2)}
            </span>
          ) : null}
        </>
      )}
    </button>
  );

  return (
    <MatrixCellPopover
      runId={runId}
      cell={cell}
      testLabel={testLabel}
      providerId={providerId}
      promptId={promptId}
      trigger={trigger}
      onSelectCase={onSelectCase}
      onCompare={onCompare}
    />
  );
}
