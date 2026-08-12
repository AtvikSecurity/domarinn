import { useMemo } from "react";
import { Link } from "react-router";
import { useSets } from "@/api/queries";
import type { ProjectSetView } from "@/api";
import { Card } from "@/components/ui/Card";
import { Chip } from "@/components/ui/Chip";
import { CopyButton } from "@/components/ui/CopyButton";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { Tooltip } from "@/components/ui/Tooltip";
import { EmptyState, ErrorState } from "@/components/States";
import { ColumnGroup } from "@/components/ui/ColumnGroup";
import { ColumnPicker } from "@/components/ui/ColumnPicker";
import { ResizableTh } from "@/components/ui/ResizableTh";
import { type ColumnDef, visibleColumns } from "@/lib/tableColumns";
import { resetColumns, setColumnVisible, useColumnPrefs } from "@/lib/useColumnPrefs";
import { cn } from "@/lib/cn";
import { type SortAccessor, sortRows } from "@/lib/sort";
import { useSortParam } from "@/lib/useTableSort";
import { setsPath } from "@/lib/routes";
import { useRowNav } from "@/lib/useRowNav";
import { Sparkline } from "@/components/Sparkline";
import {
  formatDateAbsolute,
  formatInt,
  formatRelative,
  isoFromEpoch,
} from "@/lib/format";

/** What a suite has to declare before its runs form a set. */
const PROJECT_SNIPPET = `project: checkout-agent\nsuite: regression`;

const SETS_TABLE_ID = "sets";

const SET_COLUMNS: ColumnDef[] = [
  { id: "project", label: "Project", track: "auto", min: 180, alwaysVisible: true },
  { id: "suites", label: "Suites", track: "90px", min: 70, numeric: true },
  { id: "runs", label: "Runs", track: "90px", min: 70, numeric: true },
  { id: "last_activity", label: "Last activity", track: "150px", min: 110 },
  { id: "pass_rates", label: "Suite pass rates", track: "170px", min: 130 },
  // `sr-only` header: no content to be sized from, so it must state a track.
  { id: "access", label: "Access", track: "160px", min: 130, numeric: true },
];

/** Which columns sort, and by what. The sparkline and the access chips have
 *  no scalar order, so they stay inert headers. */
const SET_SORT_FIELDS: Record<string, SortAccessor<ProjectSetView>> = {
  project: (p) => p.project,
  suites: (p) => p.suite_count,
  runs: (p) => p.run_count,
  last_activity: (p) => p.last_run_at,
};

/**
 * The run-set browser's root: every project this caller may see.
 *
 * A navigation surface, not a report — no filters, because the runs list
 * already has them and the question here is "which sets exist and who can
 * reach them". The listing is never paginated: project cardinality is small by
 * construction, and the server returns it whole.
 */
export function SetsPage() {
  const q = useSets();
  const projects = useMemo(() => q.data?.projects ?? [], [q.data]);
  const rowNav = useRowNav();
  const colPrefs = useColumnPrefs(SETS_TABLE_ID);
  const shownCols = useMemo(
    () => visibleColumns(SET_COLUMNS, colPrefs),
    [colPrefs],
  );
  const shownIds = useMemo(() => new Set(shownCols.map((c) => c.id)), [shownCols]);
  const sort = useSortParam();
  const sortedProjects = useMemo(
    () => sortRows(projects, sort.sorting, SET_SORT_FIELDS),
    [projects, sort.sorting],
  );

  return (
    <div className="space-y-5">
      <div>
        <h1 className="text-lg font-semibold tracking-tight">Run sets</h1>
        <p className="text-sm text-muted">
          Every project you can see, and who may reach it. A set is a
          project/suite pair; restricted ones are visible only to the people
          granted them.
        </p>
      </div>

      {q.isPending ? (
        <CenteredSpinner label="Loading run sets…" />
      ) : q.isError ? (
        <ErrorState error={q.error} onRetry={() => q.refetch()} />
      ) : projects.length === 0 ? (
        <NoSets />
      ) : (
        <>
        <div className="flex justify-end">
          <ColumnPicker
            columns={SET_COLUMNS}
            prefs={colPrefs}
            onChange={(id, visible) => setColumnVisible(SETS_TABLE_ID, id, visible)}
            onReset={() => resetColumns(SETS_TABLE_ID)}
          />
        </div>
        <Card padding="flush">
          {/* `relative` is load-bearing, not decoration. `sr-only` is
              `position: absolute`, and without a positioned ancestor its
              containing block is the document — so the visually-hidden header
              of the trailing Access column, sitting ~765px into a 760px-wide
              table, widened the *page* rather than this scroller and gave
              /sets a sideways scrollbar at phone widths. Making the scroll
              container the containing block keeps that overflow in here. */}
          <div className="relative overflow-x-auto scroll-hint [--scroll-hint-bg:var(--bg)]">
            <table className="w-full min-w-[840px] table-fixed text-sm">
              <ColumnGroup columns={SET_COLUMNS} prefs={colPrefs} />
              <thead>
                <tr className="border-b border-border text-left text-xs uppercase tracking-wide text-muted">
                  {/* "Suite pass rates", not "pass rate": one latest rate per
                      suite, a spread across the project rather than a trend. */}
                  {shownCols.map((c, i, arr) => (
                    <ResizableTh
                      isLast={i === arr.length - 1}
                      key={c.id}
                      def={c}
                      tableId={SETS_TABLE_ID}
                      prefs={colPrefs}
                      sort={
                        SET_SORT_FIELDS[c.id]
                          ? {
                              active: sort.sortFor(c.id),
                              onToggle: () => sort.toggle(c.id),
                            }
                          : undefined
                      }
                      className={cn(
                        "py-2 font-medium",
                        c.id === "project" ? "px-4" : "px-3",
                        c.numeric && "text-right",
                      )}
                    >
                      {c.id === "access" ? (
                        <span className="sr-only">Access</span>
                      ) : (
                        c.label
                      )}
                    </ResizableTh>
                  ))}
                </tr>
              </thead>
              <tbody>
                {sortedProjects.map((p) => {
                  const last = isoFromEpoch(p.last_run_at);
                  return (
                    <tr
                      key={p.project}
                      data-testid={`set-row-${p.project}`}
                      // The row already had a hover tint but only its first
                      // cell navigated, so the highlight promised a target
                      // that was not there. `cursor-pointer` and the row-level
                      // handler make the promise true; the link below stays the
                      // tab stop and the accessible name.
                      {...rowNav(setsPath(p.project))}
                      className="cursor-pointer border-b border-border/60 last:border-0 hover:bg-surface-2/60"
                    >
                      {shownIds.has("project") && (
                        // `truncate`: the column that takes the leftover width
                        // is the one a long name spills out of under
                        // `table-layout: fixed`.
                        <td className="truncate px-4 py-2">
                          {/* Project names are words, not ids — no font-mono. */}
                          <Link
                            to={setsPath(p.project)}
                            className="rounded-sm font-medium text-accent hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                          >
                            {p.project}
                          </Link>
                        </td>
                      )}
                      {shownIds.has("suites") && (
                        <td className="px-3 py-2 text-right tabular-nums">
                          {formatInt(p.suite_count)}
                        </td>
                      )}
                      {shownIds.has("runs") && (
                        <td className="px-3 py-2 text-right tabular-nums">
                          {formatInt(p.run_count)}
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
                      {shownIds.has("pass_rates") && (
                        // `overflow-hidden`: the sparkline is a fixed-width SVG
                        // and this column can now be dragged narrower than it.
                        <td className="overflow-hidden px-3 py-2">
                          {/* Per-suite latest rates, not a time series: the
                              spread across the project's suites at a glance. */}
                          <Sparkline
                            values={p.recent_pass_rates}
                            min={0}
                            max={1}
                            title={`Latest pass rate per suite in ${p.project}`}
                          />
                        </td>
                      )}
                      {shownIds.has("access") && (
                      <td className="px-3 py-2 text-right">
                        {p.restricted ? (
                          <Chip
                            tone="amber"
                            size="xs"
                            title="Visible only to the people granted this project"
                          >
                            restricted
                          </Chip>
                        ) : null}
                        {p.my_level ? (
                          <Chip
                            size="xs"
                            className="ml-1"
                            title="The grant you hold over this project"
                          >
                            {p.my_level}
                          </Chip>
                        ) : null}
                      </td>
                      )}
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </Card>
        </>
      )}
    </div>
  );
}

/**
 * Empty for one of two reasons — nothing has been uploaded, or everything that
 * has is restricted away from this caller — and neither is visible from here.
 * Naming what makes a set is the only useful thing this state can say.
 */
function NoSets() {
  return (
    <EmptyState title="No run sets yet">
      <p className="mx-auto max-w-prose text-sm text-muted">
        A set appears once a run declares which project and suite it belongs to.
        Add them to your suite YAML and upload a run:
      </p>
      <div className="mx-auto mt-3 flex w-fit items-center gap-2 rounded-lg border border-border bg-surface-2 px-3 py-2">
        <pre className="text-left font-mono text-xs text-fg">
          {PROJECT_SNIPPET}
        </pre>
        <CopyButton value={PROJECT_SNIPPET} label="Copy" />
      </div>
      <p className="mx-auto mt-3 max-w-prose text-xs text-muted">
        Sets you have not been granted are not listed here at all.
      </p>
    </EmptyState>
  );
}
