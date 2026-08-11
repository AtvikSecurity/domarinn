import { useEffect, useMemo, useState } from "react";
import type { CaseStatus, MatrixCell, MatrixColumn, ProviderCost } from "@/api";
import { useMatrixAll } from "@/api/queries";
import {
  cellBucketClass,
  columnGroups,
  distinctPrompts,
  singleCellStatus,
} from "@/lib/matrix";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { CHROME_FRAME } from "@/components/ui/chrome";
import { ErrorState, EmptyState } from "@/components/States";
import { cn } from "@/lib/cn";
import { formatCost } from "@/lib/format";
import { ColumnResizer } from "@/components/ui/ColumnResizer";
import { type ColumnDef, effectiveWidth } from "@/lib/tableColumns";
import {
  resetColumnWidth,
  setColumnWidth,
  useColumnPrefs,
} from "@/lib/useColumnPrefs";
import { MatrixCellPopover } from "./MatrixCellPopover";
import { ProviderCompare } from "./ProviderCompare";

const STATUS_DOT: Record<CaseStatus, string> = {
  pass: "bg-pass",
  fail: "bg-fail",
  error: "bg-error",
  skip: "bg-skip",
};

const MATRIX_TABLE_ID = "matrix";

/**
 * The sticky Test column, and only it.
 *
 * There is no column picker here and no `<colgroup>`, on purpose. The provider
 * columns are derived from the run's own data — a different run has different
 * ones — so a remembered per-column preference would key on identifiers that
 * may not exist next time, and the page already has `?provider=` and `?prompt=`
 * filters, which *are* its hide-columns mechanism. The two-tier
 * `rowSpan`/`colSpan` header cannot be described by a flat `<colgroup>` either.
 *
 * The Test column is the exception worth having: it is the one column that is
 * the same on every run, it is what everything else is read against, and it is
 * where long test names get cut off.
 */
const TEST_COLUMN: ColumnDef = {
  id: "test",
  label: "Test",
  track: "256px", // the `max-w-[16rem]` this column carried before
  min: 140,
  max: 560,
  alwaysVisible: true,
};

/** One entry of the spend legend: a provider that answered, what it spent, and
 *  whether the grid above has a column for it at all. */
interface SpendEntry {
  providerId: string;
  costUsd: number | null;
  /** False for a `fallback_only` provider: it answered for someone else and
   *  formed no column of its own. */
  isColumn: boolean;
}

/**
 * The per-provider spend legend, or `[]` when there is nothing worth saying.
 *
 * `provider_costs` is keyed by the provider that **answered**, which is the
 * whole reason this exists: a fallback spends its own tokens, and attributing
 * them to the column it stood in for would bill the primary for calls it
 * refused to make. That also means an entry can name a provider with no column
 * in the grid, which is what `isColumn` marks.
 *
 * Suppressed for the common case — one provider, one column, one bill — where
 * the legend would only restate the single column header above it. A run whose
 * costs are all NULL (`--no-raw`, a provider with no pricing) says nothing
 * either, unless a non-column answerer makes *who* ran it the news.
 */
function spendLegend(
  providerCosts: ProviderCost[],
  columns: MatrixColumn[],
): SpendEntry[] {
  if (providerCosts.length === 0) return [];
  const columnProviders = new Set(columns.map((c) => c.provider_id));
  const entries: SpendEntry[] = providerCosts.map((p) => ({
    providerId: p.provider_id,
    costUsd: p.cost_usd,
    isColumn: columnProviders.has(p.provider_id),
  }));
  // The single-configured-provider run: nothing here that the grid does not
  // already say.
  if (entries.length === 1 && entries[0]!.isColumn && columnProviders.size === 1) {
    return [];
  }
  const worthShowing =
    entries.some((e) => e.costUsd != null) || entries.some((e) => !e.isColumn);
  return worthShowing ? entries : [];
}

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
  const spend = useMemo(
    () => spendLegend(q.data?.pages[0]?.provider_costs ?? [], columns),
    [q.data, columns],
  );
  const groups = useMemo(() => columnGroups(columns), [columns]);
  const displayCols = useMemo(() => groups.flatMap((g) => g.columns), [groups]);
  const grouped = groups.length > 1;
  const showPrompt = distinctPrompts(q.data?.pages[0]).length > 1;
  const colPrefs = useColumnPrefs(MATRIX_TABLE_ID);
  const testWidth = effectiveWidth(TEST_COLUMN, colPrefs);
  // `width` and `maxWidth` together: under `table-layout: auto` a width is
  // advisory and content can still push the column wider, which is exactly
  // what the long test names in here would do.
  const testStyle = { width: testWidth, maxWidth: testWidth };

  // Which (test, prompt) the compare modal shows — component state, not URL.
  const [compare, setCompare] = useState<{ testId: string; promptId: string | null } | null>(
    null,
  );

  if (q.isPending) return <CenteredSpinner label="Loading matrix…" />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  if (rows.length === 0 || displayCols.length === 0) {
    return <EmptyState title="No matrix data for this run" />;
  }

  // `overflow-x-auto` computes `overflow-y` to `auto`, which makes this
  // container the scrollport that `position: sticky` resolves against — so a
  // `top-14` offset for the app header would do nothing, and the column headers
  // only stick once the container has a height of its own.
  //
  // `border-collapse` stays: the row separators are declared on `<tr>`, which
  // does not paint under `border-separate`.
  //
  // The legend is a sibling of the scrollport, not a row inside the table: it
  // is a property of the whole run, and a `position: sticky` header resolving
  // against a container it shared would have to account for it.
  return (
    <div className="flex flex-col gap-2 lg:min-h-0 lg:flex-1">
      {spend.length > 0 ? <SpendLegend entries={spend} /> : null}
      <div
        className={cn(
          CHROME_FRAME,
          "max-h-[70vh] overflow-auto overscroll-contain lg:max-h-none lg:min-h-0 lg:flex-1",
        )}
      >
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
                  style={testStyle}
                  // No `relative`: a sticky element is already positioned and
                  // is its own containing block, so the handle resolves
                  // against it — adding `relative` would cancel the stickiness.
                  className="sticky left-0 top-0 z-30 bg-surface-2 px-3 py-2 text-left text-xs font-semibold uppercase tracking-wide text-muted"
                >
                  <span id="matrix-h-test">Test</span>
                  <TestResizer width={testWidth} />
                </th>
                {groups.map((g) => (
                  <th
                    key={g.promptId ?? "∅"}
                    scope="colgroup"
                    colSpan={g.columns.length}
                    className="sticky top-0 z-20 h-8 border-l border-border bg-surface-2 px-2 py-1.5 text-center font-mono text-[11px] font-semibold text-fg"
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
                      "sticky top-8 z-20 h-7 bg-surface-2 px-2 py-1.5 text-center font-mono text-[11px] text-muted",
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
                style={testStyle}
                className="sticky left-0 top-0 z-30 bg-surface-2 px-3 py-2 text-left text-xs font-semibold uppercase tracking-wide text-muted"
              >
                <span id="matrix-h-test">Test</span>
                <TestResizer width={testWidth} />
              </th>
              {displayCols.map((c, i) => (
                <th
                  key={`${c.colIndex}-${i}`}
                  scope="col"
                  className="sticky top-0 z-20 bg-surface-2 px-2 py-1.5 text-center font-mono text-[11px] text-muted"
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
                  style={testStyle}
                  className="sticky left-0 z-10 truncate bg-surface px-3 py-1.5 text-left font-medium"
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
    </div>
  );
}

/**
 * `Spend  primary $0.42 · reserve-mini $0.02 (fallback)` — what each provider
 * that answered cost, in first-seen order.
 *
 * The `(fallback)` marker is not decoration: an entry with no column is a
 * provider the grid above never mentions, so without it the line reads as a
 * column total that does not match any column. Real text, not a tint, for the
 * same reason.
 */
function SpendLegend({ entries }: { entries: SpendEntry[] }) {
  return (
    <p
      className="flex shrink-0 flex-wrap items-center gap-x-2 gap-y-0.5 text-xs text-muted"
      title="cost attributed to the provider that answered"
    >
      <span className="font-medium text-fg">Spend</span>
      {entries.map((e, i) => (
        <span key={e.providerId} className="flex items-center gap-x-2">
          {i > 0 ? (
            <span className="text-muted/50" aria-hidden>
              ·
            </span>
          ) : null}
          <span className={cn("tabular-nums", !e.isColumn && "text-amber")}>
            {e.providerId} {formatCost(e.costUsd)}
            {e.isColumn ? "" : " (fallback)"}
          </span>
        </span>
      ))}
    </p>
  );
}

/**
 * The Test column's drag handle. A component only because the header exists in
 * two branches — grouped (`rowSpan=2`) and flat — and both must offer it.
 */
function TestResizer({ width }: { width: number }) {
  return (
    <ColumnResizer
      def={TEST_COLUMN}
      width={width}
      headerId="matrix-h-test"
      onResize={(px) => setColumnWidth(MATRIX_TABLE_ID, TEST_COLUMN.id, px)}
      onReset={() => resetColumnWidth(MATRIX_TABLE_ID, TEST_COLUMN.id)}
    />
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
  // Flakiness was previously only marked on cells that also passed everything,
  // which hid it exactly where it matters most — a cell that is both flaky and
  // failing.
  const flake = cell.distinct_outputs > 1;
  const label =
    `${testLabel}, ${providerId}${promptId != null ? ` · ${promptId}` : ""}: ` +
    (single
      ? singleCellStatus(cell)
      : `${cell.passed} of ${cell.total} passed` +
        (cell.errored > 0 ? `, ${cell.errored} errored` : ""));

  const trigger = (
    <button
      type="button"
      data-cell={`${testId}:${colIndex}`}
      aria-label={label}
      className={cn(
        "flex min-h-[2.25rem] w-full flex-col items-center justify-center gap-0 rounded px-1 py-1 text-xs font-medium tabular-nums transition-shadow",
        "hover:ring-2 hover:ring-ring focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
        single ? "" : cellBucketClass(cell.pass_fraction, cell.errored),
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
