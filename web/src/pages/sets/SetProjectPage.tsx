import { useMemo, useState } from "react";
import { Link, useParams } from "react-router";
import { useSetProject } from "@/api/queries";
import type { SuiteSetView } from "@/api";
import { ApiError } from "@/api/client";
import { useAuthView } from "@/auth/AuthProvider";
import { Breadcrumb } from "@/components/ui/Breadcrumb";
import { Button } from "@/components/ui/Button";
import { Card } from "@/components/ui/Card";
import { Chip } from "@/components/ui/Chip";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { Tooltip } from "@/components/ui/Tooltip";
import { NotFound } from "@/components/NotFound";
import { ErrorState } from "@/components/States";
import { RateBadge } from "@/components/PassRateBadge";
import { Sparkline } from "@/components/Sparkline";
import { ColumnGroup } from "@/components/ui/ColumnGroup";
import { ColumnPicker } from "@/components/ui/ColumnPicker";
import { ResizableTh } from "@/components/ui/ResizableTh";
import { type ColumnDef, visibleColumns } from "@/lib/tableColumns";
import { resetColumns, setColumnVisible, useColumnPrefs } from "@/lib/useColumnPrefs";
import { cn } from "@/lib/cn";
import { type SortAccessor, sortRows } from "@/lib/sort";
import { useSortParam } from "@/lib/useTableSort";
import {
  formatDateAbsolute,
  formatInt,
  formatRelative,
  isoFromEpoch,
} from "@/lib/format";
import { suitePath } from "@/lib/routes";
import { useRowNav } from "@/lib/useRowNav";
import { AccessPanel } from "./AccessPanel";

const SUITES_TABLE_ID = "setProjectSuites";

/**
 * `flags` carries an `sr-only` header, so it has no content to be sized from
 * and must state a width — once a `<colgroup>` exists, a column with none
 * collapses to zero and its chips wrap into an unreadable stack.
 */
const SUITE_COLUMNS: ColumnDef[] = [
  { id: "suite", label: "Suite", track: "auto", min: 200, alwaysVisible: true },
  { id: "runs", label: "Runs", track: "90px", min: 70, numeric: true },
  { id: "last_activity", label: "Last activity", track: "150px", min: 110 },
  { id: "pass_rate", label: "Latest pass rate", track: "150px", min: 120 },
  { id: "trend", label: "Trend", track: "140px", min: 110 },
  { id: "flags", label: "Flags", track: "150px", min: 120, numeric: true },
];

/** Sortable columns. The trend sparkline and the flag chips have no scalar
 *  order, so their headers stay inert. */
const SUITE_SORT_FIELDS: Record<string, SortAccessor<SuiteSetView>> = {
  suite: (s) => s.suite,
  runs: (s) => s.run_count,
  last_activity: (s) => s.last_run_at,
  pass_rate: (s) => s.latest_pass_rate,
};

/**
 * One project's suites.
 *
 * A 404 renders the app's not-found page rather than an error: the server
 * refuses to distinguish "restricted away from you" from "never existed", and
 * a page saying "request failed (404)" would leak that the difference exists.
 */
export function SetProjectPage() {
  const { project = "" } = useParams();
  const view = useAuthView();
  const q = useSetProject(project);
  const [accessOpen, setAccessOpen] = useState(false);
  const rowNav = useRowNav();
  const colPrefs = useColumnPrefs(SUITES_TABLE_ID);
  const shownCols = useMemo(
    () => visibleColumns(SUITE_COLUMNS, colPrefs),
    [colPrefs],
  );
  const shownIds = useMemo(() => new Set(shownCols.map((c) => c.id)), [shownCols]);
  const sort = useSortParam();

  if (q.isPending) return <CenteredSpinner label="Loading project…" />;
  if (q.isError) {
    if (q.error instanceof ApiError && q.error.status === 404) return <NotFound />;
    return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  }

  const detail = q.data;
  // Below the early returns, so plain computation rather than a hook; the
  // suite list is small (a project's suites), so no memo is warranted.
  const sortedSuites = sortRows(detail.suites, sort.sorting, SUITE_SORT_FIELDS);
  const runCount = detail.suites.reduce((n, s) => n + s.run_count, 0);
  // Reading the panel is read-scoped, so a viewer-role manager may open it;
  // the panel itself is what gates the mutations inside.
  const canOpenAccess = view.canAdmin || detail.my_level === "manage";

  return (
    <div className="space-y-5">
      <div>
        <Breadcrumb items={[{ label: "Sets", to: "/sets" }, { label: project }]} />
        <div className="mt-1 flex flex-wrap items-center gap-2">
          <h1 className="text-lg font-semibold tracking-tight">{project}</h1>
          {detail.restricted ? (
            <Chip
              tone="amber"
              size="xs"
              title="Visible only to the people granted this project"
            >
              restricted
            </Chip>
          ) : null}
          {detail.my_level ? (
            <Chip size="xs" title="The grant you hold over this project">
              {detail.my_level}
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
        <p className="text-sm text-muted tabular-nums">
          {formatInt(detail.suites.length)}{" "}
          {detail.suites.length === 1 ? "suite" : "suites"} ·{" "}
          {formatInt(runCount)} {runCount === 1 ? "run" : "runs"}
        </p>
      </div>

      <div className="flex justify-end">
        <ColumnPicker
          columns={SUITE_COLUMNS}
          prefs={colPrefs}
          onChange={(id, visible) => setColumnVisible(SUITES_TABLE_ID, id, visible)}
          onReset={() => resetColumns(SUITES_TABLE_ID)}
        />
      </div>

      <Card padding="flush">
        {/* `relative`: contains the trailing Flags column's `sr-only` header,
            which is `position: absolute` and would otherwise widen the page
            instead of this scroller. See SetsPage for the full account. */}
        <div className="relative overflow-x-auto scroll-hint [--scroll-hint-bg:var(--bg)]">
          <table className="w-full min-w-[880px] table-fixed text-sm">
            <ColumnGroup columns={SUITE_COLUMNS} prefs={colPrefs} />
            <thead>
              <tr className="border-b border-border text-left text-xs uppercase tracking-wide text-muted">
                {shownCols.map((c, i, arr) => (
                  <ResizableTh
                    isLast={i === arr.length - 1}
                    key={c.id}
                    def={c}
                    tableId={SUITES_TABLE_ID}
                    prefs={colPrefs}
                    sort={
                      SUITE_SORT_FIELDS[c.id]
                        ? {
                            active: sort.sortFor(c.id),
                            onToggle: () => sort.toggle(c.id),
                          }
                        : undefined
                    }
                    className={cn(
                      "py-2 font-medium",
                      c.id === "suite" ? "px-4" : "px-3",
                      c.numeric && "text-right",
                    )}
                  >
                    {c.id === "flags" ? (
                      <span className="sr-only">Flags</span>
                    ) : (
                      c.label
                    )}
                  </ResizableTh>
                ))}
              </tr>
            </thead>
            <tbody>
              {sortedSuites.map((s) => {
                const last = isoFromEpoch(s.last_run_at);
                return (
                  <tr
                    key={s.suite}
                    data-testid={`suite-row-${s.suite}`}
                    {...rowNav(suitePath(project, s.suite))}
                    className="cursor-pointer border-b border-border/60 last:border-0 hover:bg-surface-2/60"
                  >
                    {shownIds.has("suite") && (
                      // `truncate`: this is the column that takes the leftover
                      // width, so under `table-layout: fixed` a long suite name
                      // is what would spill into its neighbour.
                      <td className="truncate px-4 py-2">
                        <Link
                          to={suitePath(project, s.suite)}
                          className="rounded-sm font-medium text-accent hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                        >
                          {s.suite}
                        </Link>
                      </td>
                    )}
                    {shownIds.has("runs") && (
                      <td className="px-3 py-2 text-right tabular-nums">
                        {formatInt(s.run_count)}
                      </td>
                    )}
                    {shownIds.has("last_activity") && (
                      <td className="px-3 py-2 text-muted">
                        {last ? (
                          <Tooltip content={formatDateAbsolute(last)}>
                            <time dateTime={last}>{formatRelative(last)}</time>
                          </Tooltip>
                        ) : (
                          "-"
                        )}
                      </td>
                    )}
                    {shownIds.has("pass_rate") && (
                      <td className="px-3 py-2">
                        <RateBadge
                          rate={s.latest_pass_rate}
                          title={`Newest run of ${s.suite}`}
                        />
                      </td>
                    )}
                    {shownIds.has("trend") && (
                      <td className="px-3 py-2">
                        <Sparkline
                          values={s.sparkline}
                          min={0}
                          max={1}
                          title={`Pass rate trend for ${s.suite}`}
                        />
                      </td>
                    )}
                    {shownIds.has("flags") && (
                    <td className="px-3 py-2 text-right">
                      <span className="inline-flex items-center gap-1">
                        {s.baseline_run_id ? (
                          <Chip
                            tone="accent"
                            size="xs"
                            title={`Pinned comparison baseline: ${s.baseline_run_id}`}
                          >
                            baseline
                          </Chip>
                        ) : s.baseline_branch ? (
                          <Chip
                            tone="accent"
                            size="xs"
                            title={`Baseline tracks branch ${s.baseline_branch}: the newest runs on it merge into the comparison`}
                          >
                            baseline: {s.baseline_branch}
                          </Chip>
                        ) : null}
                        {/* Only when the suite's own lock is what restricts it:
                            inside an already-restricted project the chip would
                            repeat the heading's, on every row. */}
                        {s.restricted && !detail.restricted ? (
                          <Chip
                            tone="amber"
                            size="xs"
                            title="This suite is restricted on its own"
                          >
                            restricted
                          </Chip>
                        ) : null}
                      </span>
                    </td>
                    )}
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </Card>

      <AccessPanel
        project={project}
        suite={null}
        // The panel's own payload is exact-scope; only the browse detail knows
        // whether anything covers the set.
        coveringRestricted={detail.restricted}
        open={accessOpen}
        onClose={() => setAccessOpen(false)}
      />
    </div>
  );
}
