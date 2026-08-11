import { useMemo, useState } from "react";
import { Link, useParams, useSearchParams } from "react-router";
import { useRuns, useSetSuite } from "@/api/queries";
import type { RunListItem } from "@/api";
import { ApiError } from "@/api/client";
import { useAuthView } from "@/auth/AuthProvider";
import { Breadcrumb } from "@/components/ui/Breadcrumb";
import { Button } from "@/components/ui/Button";
import { Card } from "@/components/ui/Card";
import { Chip } from "@/components/ui/Chip";
import { CopyButton } from "@/components/ui/CopyButton";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { StatBlock } from "@/components/ui/StatBlock";
import { Tooltip } from "@/components/ui/Tooltip";
import { NotFound } from "@/components/NotFound";
import { EmptyState, ErrorState } from "@/components/States";
import { PassRateBadge, RateBadge } from "@/components/PassRateBadge";
import { RunOriginCell } from "@/components/RunOriginCell";
import { Sparkline } from "@/components/Sparkline";
import { previousRun } from "@/lib/compare";
import { hiddenByCachedExclude, isFullyCached, resolveCached } from "@/lib/cached";
import { useCachedPref } from "@/lib/cachedPref";
import { mergeParams } from "@/lib/filters";
import { CachedRunsToggle } from "@/components/CachedRunsToggle";
import { ColumnGroup } from "@/components/ui/ColumnGroup";
import { ColumnPicker } from "@/components/ui/ColumnPicker";
import { ResizableTh } from "@/components/ui/ResizableTh";
import { type ColumnDef, visibleColumns } from "@/lib/tableColumns";
import { resetColumns, setColumnVisible, useColumnPrefs } from "@/lib/useColumnPrefs";
import { cn } from "@/lib/cn";
import {
  formatDateAbsolute,
  formatDuration,
  formatInt,
  formatPercent,
  formatRelative,
  isoFromEpoch,
  parseTimestamp,
} from "@/lib/format";
import {
  comparePath,
  runPath,
  runsFilterHref,
  setsPath,
} from "@/lib/routes";
import { useRowNav } from "@/lib/useRowNav";
import { AccessPanel } from "./AccessPanel";

/** From a pair of selected run ids, resolve which is base (older) / head (newer). */
function comparePair(
  runs: RunListItem[],
  selected: string[],
): { baseId: string; headId: string } | null {
  if (selected.length !== 2) return null;
  const picked = runs.filter((r) => selected.includes(r.id));
  if (picked.length !== 2) return null;
  const sorted = [...picked].sort(
    (a, b) => parseTimestamp(a.created_at) - parseTimestamp(b.created_at),
  );
  const [older, newer] = sorted;
  if (!older || !newer) return null;
  return { baseId: older.id, headId: newer.id };
}

/** This table's slot in the shared column-preference store. Distinct from the
 *  runs list's: a different column set, for a page with a different job. */
const SUITE_RUNS_TABLE_ID = "setSuiteRuns";

const SUITE_RUN_COLUMNS: ColumnDef[] = [
  { id: "select", label: "Select", track: "40px", min: 40, alwaysVisible: true },
  // Stated, not `auto` — see RunsList: a ULID plus a copy button plus chips
  // does not fit in whatever the declared columns happen to leave over.
  { id: "run", label: "Run", track: "340px", min: 240, alwaysVisible: true },
  { id: "when", label: "When", track: "130px", min: 110 },
  { id: "who", label: "Who", track: "150px", min: 110 },
  { id: "pass_rate", label: "Pass rate", track: "130px", min: 110 },
  { id: "cases", label: "Cases", track: "80px", min: 70, numeric: true },
  { id: "duration", label: "Duration", track: "110px", min: 90, numeric: true },
  { id: "compare", label: "Compare", track: "110px", min: 100, numeric: true },
];

/**
 * One suite: what it is, who may reach it, and its runs.
 *
 * The runs table is a deliberate copy of the runs list's rows rather than a
 * shared component. Two instances of a table are not an abstraction, and the
 * runs list owns filtering and grouping this page deliberately does not have —
 * factoring them together would drag all of that in for no reader's benefit.
 */
export function SetSuitePage() {
  const { project = "", suite = "" } = useParams();
  const [params, setParams] = useSearchParams();
  const view = useAuthView();
  const q = useSetSuite(project, suite);
  const [accessOpen, setAccessOpen] = useState(false);

  // This page used to force `cached: "all"`, because hiding a suite's
  // fully-cached CI re-runs made the table disagree with the run count in the
  // header two lines above. The disagreement was the problem, not the hiding —
  // so the page now honours the same preference as everywhere else and states
  // the difference instead of avoiding it.
  const cachedPref = useCachedPref();
  const resolvedCached = resolveCached(params.get("cached"), cachedPref);
  const runsQ = useRuns({ project, suite, cached: resolvedCached });
  const colPrefs = useColumnPrefs(SUITE_RUNS_TABLE_ID);
  const shownCols = useMemo(
    () => visibleColumns(SUITE_RUN_COLUMNS, colPrefs),
    [colPrefs],
  );
  const shownColIds = useMemo(
    () => new Set(shownCols.map((c) => c.id)),
    [shownCols],
  );
  const runs = useMemo(
    () => runsQ.data?.pages.flatMap((p) => p.runs) ?? [],
    [runsQ.data],
  );
  const [selected, setSelected] = useState<string[]>([]);
  const rowNav = useRowNav();

  if (q.isPending) return <CenteredSpinner label="Loading suite…" />;
  if (q.isError) {
    if (q.error instanceof ApiError && q.error.status === 404) return <NotFound />;
    return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  }

  const detail = q.data;
  const last = isoFromEpoch(detail.last_run_at);
  const canOpenAccess = view.canAdmin || detail.my_level === "manage";
  const pair = comparePair(runs, selected);
  // Carry this page's own view through, so the stream opens showing what the
  // table above it was showing rather than always widening to everything.
  const runsHref = runsFilterHref(project, suite, resolvedCached);

  function toggle(id: string) {
    setSelected((prev) =>
      prev.includes(id)
        ? prev.filter((x) => x !== id)
        : [...prev, id].slice(-2), // keep at most the two most recent picks
    );
  }

  return (
    <div className="space-y-5">
      <Card>
        <Breadcrumb
          items={[
            { label: "Sets", to: "/sets" },
            { label: project, to: setsPath(project) },
            { label: suite },
          ]}
        />
        <div className="mt-1 flex flex-wrap items-center gap-2">
          <h1 className="text-lg font-semibold tracking-tight">{suite}</h1>
          {detail.restricted ? (
            <Chip
              tone="amber"
              size="xs"
              title="Visible only to the people granted this set"
            >
              restricted
            </Chip>
          ) : null}
          {detail.my_level ? (
            <Chip size="xs" title="The grant you hold over this suite">
              {detail.my_level}
            </Chip>
          ) : null}
          {detail.baseline_run_id ? (
            <Chip
              tone="accent"
              size="xs"
              title={`Pinned comparison baseline: ${detail.baseline_run_id}`}
            >
              baseline
            </Chip>
          ) : null}
          {canOpenAccess ? (
            <Button
              className="ml-auto"
              variant="secondary"
              size="sm"
              onClick={() => setAccessOpen(true)}
            >
              Access
            </Button>
          ) : null}
        </div>

        <div className="mt-4 grid grid-cols-2 gap-3 sm:grid-cols-4">
          <StatBlock label="Runs">{formatInt(detail.run_count)}</StatBlock>
          <StatBlock label="Latest pass rate">
            <RateBadge rate={detail.latest_pass_rate} title="Newest run" />
          </StatBlock>
          <StatBlock
            label="Cases"
            sub={`${formatPercent(
              detail.case_count === 0 ? null : detail.pass_count / detail.case_count,
            )} passed overall`}
          >
            {formatInt(detail.case_count)}
          </StatBlock>
          <StatBlock label="Last activity">
            {last ? (
              <Tooltip content={formatDateAbsolute(last)}>
                <time dateTime={last}>{formatRelative(last)}</time>
              </Tooltip>
            ) : (
              "-"
            )}
          </StatBlock>
        </div>

        {detail.sparkline.length > 1 ? (
          <div className="mt-4 flex items-center gap-3">
            <span className="text-[11px] text-muted">pass-rate trend</span>
            <Sparkline
              values={detail.sparkline}
              min={0}
              max={1}
              width={160}
              height={30}
              title={`Pass rate trend for ${suite}`}
            />
          </div>
        ) : null}
      </Card>

      <section className="space-y-3">
        <div className="flex flex-wrap items-center gap-3">
          <h2 className="text-sm font-semibold">Runs</h2>
          {selected.length > 0 ? (
            <div className="flex items-center gap-2">
              <span className="text-[11px] text-muted">
                {selected.length} selected
              </span>
              {pair ? (
                // Server contract: first url segment = base, second = head.
                <Link
                  to={comparePath(pair.baseId, pair.headId)}
                >
                  <Button variant="primary" size="sm">
                    Compare 2 runs
                  </Button>
                </Link>
              ) : (
                <Button
                  variant="secondary"
                  size="sm"
                  disabled
                  title="Select two runs to compare"
                >
                  Compare 2 runs
                </Button>
              )}
              <Button variant="ghost" size="sm" onClick={() => setSelected([])}>
                Clear
              </Button>
            </div>
          ) : null}
          <Link
            to={runsHref}
            className="ml-auto text-sm text-accent hover:underline"
          >
            View in Runs →
          </Link>
        </div>

        {/* The header's run count comes from the server's aggregate and counts
            every run, so whenever this table hides some it has to say so —
            otherwise the two numbers just disagree and the page looks wrong. */}
        <div className="flex items-center justify-between gap-3">
          <CachedRunsToggle
            resolved={resolvedCached}
            hiddenCount={runsQ.data?.pages[0]?.cached_hidden ?? 0}
            onChange={(next) =>
              setParams(mergeParams(params, { cached: next }), { replace: true })
            }
          />
          <ColumnPicker
            columns={SUITE_RUN_COLUMNS}
            prefs={colPrefs}
            onChange={(id, visible) =>
              setColumnVisible(SUITE_RUNS_TABLE_ID, id, visible)
            }
            onReset={() => resetColumns(SUITE_RUNS_TABLE_ID)}
          />
        </div>

        {runsQ.isPending ? (
          <CenteredSpinner label="Loading runs…" />
        ) : runsQ.isError ? (
          <ErrorState error={runsQ.error} onRetry={() => runsQ.refetch()} />
        ) : runs.length === 0 ? (
          <EmptyState title="No runs in this suite yet" />
        ) : (
          <>
            <Card padding="flush">
              {/* `relative`: contains the `sr-only` column headers, which are
                  `position: absolute` and escape a scroller that is not a
                  containing block. See SetsPage for the full account. */}
              <div className="relative overflow-x-auto scroll-hint [--scroll-hint-bg:var(--bg)]">
                <table className="w-full min-w-[1090px] table-fixed text-sm">
                  <ColumnGroup columns={SUITE_RUN_COLUMNS} prefs={colPrefs} />
                  <thead>
                    <tr className="border-b border-border text-left text-xs uppercase tracking-wide text-muted">
                      {shownCols.map((c, i, arr) => (
                        <ResizableTh
                          isLast={i === arr.length - 1}
                          key={c.id}
                          def={c}
                          tableId={SUITE_RUNS_TABLE_ID}
                          prefs={colPrefs}
                          className={cn(
                            "py-2 font-medium",
                            c.id === "run" ? "px-4" : "px-3",
                            c.numeric && "text-right",
                          )}
                        >
                          {c.id === "select" ? (
                            <span className="sr-only">Select</span>
                          ) : (
                            c.label
                          )}
                        </ResizableTh>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {runs.map((r) => {
                      const compareTarget = previousRun(runs, r.id);
                      const fullyCached = isFullyCached(r);
                      // Dim exactly what `cached=exclude` would have hidden. A
                      // cached run that failed is real signal (verdicts are
                      // never cached) and stays at full contrast.
                      const dimmed = hiddenByCachedExclude(r);
                      return (
                        <tr
                          key={r.id}
                          // The selection checkbox and the Compare link keep
                          // their own clicks: `useRowNav` bails on any real
                          // control in the row, so picking two runs to compare
                          // never navigates away mid-selection.
                          {...rowNav(runPath(r.id))}
                          className={`cursor-pointer border-b border-border/60 last:border-0 hover:bg-surface-2${dimmed ? " opacity-60" : ""}`}
                        >
                          {shownColIds.has("select") && (
                            <td className="px-3 py-2">
                              {/* The label is the hit target: an input's own 16px
                                  box is under WCAG 2.2's 24px minimum. */}
                              <label className="flex size-6 cursor-pointer items-center justify-center">
                                <input
                                  type="checkbox"
                                  aria-label={`Select run ${r.id}`}
                                  checked={selected.includes(r.id)}
                                  onChange={() => toggle(r.id)}
                                  className="size-4 accent-[var(--color-accent)]"
                                />
                              </label>
                            </td>
                          )}
                          {shownColIds.has("run") && (
                            // Clips rather than overlapping the next column —
                            // see RunsList for the full account.
                            <td className="overflow-hidden px-4 py-2">
                              <span className="flex min-w-0 items-center gap-1">
                                <Link
                                  to={runPath(r.id)}
                                  className="truncate font-medium text-accent hover:underline"
                                >
                                  {r.id}
                                </Link>
                                <CopyButton value={r.id} label="Copy run id" iconOnly />
                                {r.id === detail.baseline_run_id ? (
                                  <Chip
                                    tone="accent"
                                    title="Pinned comparison baseline for this suite"
                                  >
                                    baseline
                                  </Chip>
                                ) : null}
                                {fullyCached ? (
                                  <Chip title="Every provider call in this run was a cache hit">
                                    cached
                                  </Chip>
                                ) : null}
                              </span>
                            </td>
                          )}
                          {shownColIds.has("when") && (
                            <td className="px-3 py-2 text-muted">
                              <Tooltip content={formatDateAbsolute(r.created_at)}>
                                <time dateTime={r.created_at}>
                                  {formatRelative(r.created_at)}
                                </time>
                              </Tooltip>
                            </td>
                          )}
                          {shownColIds.has("who") && (
                            <td className="px-3 py-2">
                              <RunOriginCell run={r} />
                            </td>
                          )}
                          {shownColIds.has("pass_rate") && (
                            <td className="px-3 py-2">
                              <PassRateBadge
                                pass={r.pass_count}
                                fail={r.fail_count}
                                error={r.error_count}
                                xpass={r.xpass_count}
                              />
                            </td>
                          )}
                          {shownColIds.has("cases") && (
                            <td className="px-3 py-2 text-right tabular-nums">
                              {r.case_count}
                            </td>
                          )}
                          {shownColIds.has("duration") && (
                            <td className="px-3 py-2 text-right tabular-nums text-muted">
                              {formatDuration(r.duration_ms)}
                            </td>
                          )}
                          {shownColIds.has("compare") && (
                            <td className="px-3 py-2 text-right">
                              {compareTarget ? (
                                <Link
                                  to={comparePath(compareTarget.id, r.id)}
                                  className="text-xs font-medium text-accent hover:underline"
                                  title={`Compare against ${compareTarget.id}`}
                                >
                                  Compare
                                </Link>
                              ) : (
                                <span
                                  className="text-xs text-muted"
                                  title="No earlier loaded run in this suite to compare against"
                                >
                                  —
                                </span>
                              )}
                            </td>
                          )}
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            </Card>
            {runsQ.hasNextPage ? (
              <div className="flex justify-center">
                <Button
                  variant="secondary"
                  onClick={() => runsQ.fetchNextPage()}
                  disabled={runsQ.isFetchingNextPage}
                >
                  {runsQ.isFetchingNextPage ? "Loading…" : "Load more runs"}
                </Button>
              </div>
            ) : null}
          </>
        )}
      </section>

      <AccessPanel
        project={project}
        suite={suite}
        // Covering: true for a suite inside a locked project, which owns no
        // restriction row of its own.
        coveringRestricted={detail.restricted}
        open={accessOpen}
        onClose={() => setAccessOpen(false)}
      />
    </div>
  );
}
