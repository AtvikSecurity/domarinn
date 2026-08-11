import { useMemo, useState } from "react";
import { Link, useSearchParams } from "react-router";
import { useRuns, useSuites } from "@/api/queries";
import type { RunListItem } from "@/api";
import { mergeParams, parseRunsFilters } from "@/lib/filters";
import { suitePassRateSeries } from "@/lib/suites";
import { previousRun } from "@/lib/compare";
import { hiddenByCachedExclude, isFullyCached, resolveCached } from "@/lib/cached";
import { useCachedPref } from "@/lib/cachedPref";
import { CachedRunsToggle } from "@/components/CachedRunsToggle";
import { ColumnGroup } from "@/components/ui/ColumnGroup";
import { ColumnPicker } from "@/components/ui/ColumnPicker";
import { ResizableTh } from "@/components/ui/ResizableTh";
import { type ColumnDef, visibleColumns } from "@/lib/tableColumns";
import { cn } from "@/lib/cn";
import { resetColumns, setColumnVisible, useColumnPrefs } from "@/lib/useColumnPrefs";
import {
  formatCost,
  formatDateAbsolute,
  formatDuration,
  formatRelative,
  formatTokens,
  parseTimestamp,
  passRate,
} from "@/lib/format";
import { comparePath, runPath, suitePath } from "@/lib/routes";
import { useRowNav } from "@/lib/useRowNav";
import { RunsFilterBar } from "@/components/RunsFilterBar";
import { Sparkline } from "@/components/Sparkline";
import { PassRateBadge } from "@/components/PassRateBadge";
import { Chip } from "@/components/ui/Chip";
import { CHROME_FRAME } from "@/components/ui/chrome";
import { CopyButton } from "@/components/ui/CopyButton";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { ErrorState, EmptyState } from "@/components/States";
import { Button } from "@/components/ui/Button";
import { Tooltip } from "@/components/ui/Tooltip";
import { RunOriginCell } from "@/components/RunOriginCell";

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

/** This table's slot in the shared column-preference store. */
const RUNS_TABLE_ID = "runs";

/**
 * Twelve columns that want more width than any laptop has — the strongest case
 * in the app for letting people choose. `auto` means "take the leftover space",
 * the `<table>` analogue of an `fr` share, and exactly one column may sensibly
 * have it.
 *
 * `select` and `run` are structural: without the checkbox there is no way to
 * pick runs to compare, and a row that does not say which run it is is not a
 * row. Both carry explicit widths anyway — once a `<colgroup>` exists, a
 * column with no width collapses to nothing, and `select`'s header is
 * `sr-only`, so it has no content to be sized from.
 */
const RUNS_COLUMNS: ColumnDef[] = [
  { id: "select", label: "Select", track: "40px", min: 40, alwaysVisible: true },
  // A stated width, not `auto`. Under `table-layout: fixed` an `auto` column
  // gets only what the declared ones leave over, and this cell is a 26-char
  // ULID plus a copy button plus up to two chips — with twelve columns
  // competing there is not enough slack, and the content spills into `when`
  // rather than shrinking. `tags` is the one that flexes: it truncates.
  { id: "run", label: "Run", track: "340px", min: 240, alwaysVisible: true },
  { id: "when", label: "When", track: "110px", min: 100 },
  { id: "who", label: "Who", track: "130px", min: 110 },
  { id: "branch", label: "Branch", track: "200px", min: 120 },
  { id: "pass_rate", label: "Pass rate", track: "130px", min: 110 },
  { id: "cases", label: "Cases", track: "70px", min: 70, numeric: true },
  { id: "tokens", label: "Tokens", track: "80px", min: 80, numeric: true },
  { id: "cost", label: "Cost", track: "80px", min: 80, numeric: true },
  { id: "duration", label: "Duration", track: "90px", min: 90, numeric: true },
  { id: "tags", label: "Tags", track: "auto", min: 90 },
  { id: "compare", label: "Compare", track: "100px", min: 100, numeric: true },
];

interface Group {
  key: string;
  project: string | null;
  suite: string | null;
  runs: RunListItem[]; // newest first
  series: number[]; // pass rate oldest -> newest
  latest: number;
}

function groupRuns(runs: RunListItem[]): Group[] {
  const map = new Map<string, RunListItem[]>();
  for (const r of runs) {
    const key = `${r.project}/${r.suite}`;
    (map.get(key) ?? map.set(key, []).get(key)!).push(r);
  }
  const groups: Group[] = [];
  for (const [key, list] of map) {
    const first = list[0];
    // Every map entry is created by pushing at least one run, so `first` is
    // always present; skip defensively rather than assert.
    if (!first) continue;
    const byDateAsc = [...list].sort(
      (a, b) => parseTimestamp(a.created_at) - parseTimestamp(b.created_at),
    );
    groups.push({
      key,
      project: first.project,
      suite: first.suite,
      runs: [...list].sort(
        (a, b) => parseTimestamp(b.created_at) - parseTimestamp(a.created_at),
      ),
      series: byDateAsc.map(
        (r) => passRate(r.pass_count, r.fail_count, r.error_count) ?? 0,
      ),
      latest: Math.max(...list.map((r) => parseTimestamp(r.created_at))),
    });
  }
  return groups.sort((a, b) => b.latest - a.latest);
}

export function RunsList() {
  const [params, setParams] = useSearchParams();
  const filters = parseRunsFilters(params);
  const q = useRuns(filters);

  const runs = useMemo(
    () => q.data?.pages.flatMap((p) => p.runs) ?? [],
    [q.data],
  );
  const groups = useMemo(() => groupRuns(runs), [runs]);
  // Present only on the first page of the hidden view; the toggle keeps the
  // suppression discoverable and one click away.
  const cachedHidden = q.data?.pages[0]?.cached_hidden ?? 0;
  const cachedPref = useCachedPref();
  const resolvedCached = resolveCached(params.get("cached"), cachedPref);
  const runsColumnPrefs = useColumnPrefs(RUNS_TABLE_ID);

  return (
    <div className="space-y-5">
      <div>
        <h1 className="text-lg font-semibold tracking-tight">Eval runs</h1>
        <p className="text-sm text-muted">
          Browse runs grouped by suite. Filters live in the URL and are shareable.
        </p>
      </div>

      <RunsFilterBar />

      {/* One picker for the page, not one per suite group: the groups are the
          same table repeated, and a per-group control would imply otherwise. */}
      <div className="flex items-center justify-between gap-3">
        <CachedRunsToggle
          resolved={resolvedCached}
          hiddenCount={cachedHidden}
          onChange={(next) =>
            setParams(mergeParams(params, { cached: next }), { replace: true })
          }
        />
        <ColumnPicker
          columns={RUNS_COLUMNS}
          prefs={runsColumnPrefs}
          onChange={(id, visible) =>
            setColumnVisible(RUNS_TABLE_ID, id, visible)
          }
          onReset={() => resetColumns(RUNS_TABLE_ID)}
        />
      </div>

      {q.isPending ? (
        <CenteredSpinner label="Loading runs…" />
      ) : q.isError ? (
        <ErrorState error={q.error} onRetry={() => q.refetch()} />
      ) : groups.length === 0 ? (
        <EmptyState title="No runs match these filters">
          Try clearing filters or uploading a run.
        </EmptyState>
      ) : (
        <div className="space-y-6">
          {groups.map((g) => (
            <SuiteGroup key={g.key} group={g} />
          ))}
          {q.hasNextPage ? (
            <div className="flex justify-center">
              <Button
                variant="secondary"
                onClick={() => q.fetchNextPage()}
                disabled={q.isFetchingNextPage}
              >
                {q.isFetchingNextPage ? "Loading…" : "Load more runs"}
              </Button>
            </div>
          ) : null}
        </div>
      )}
    </div>
  );
}

function SuiteGroup({ group }: { group: Group }) {
  const [selected, setSelected] = useState<string[]>([]);
  const rowNav = useRowNav();
  // Every suite group on the page reads the same preference, so hiding a
  // column hides it in all of them rather than only where you clicked.
  const prefs = useColumnPrefs(RUNS_TABLE_ID);
  const shown = useMemo(() => visibleColumns(RUNS_COLUMNS, prefs), [prefs]);
  // Cells must be omitted, not merely blanked: with a <colgroup> in play a
  // stray <td> shifts every column after it out of alignment.
  const shownIds = useMemo(() => new Set(shown.map((c) => c.id)), [shown]);

  // Prefer the server's authoritative pass-rate series + pinned baseline
  // (deduped per project by TanStack Query). Fall back to the client-computed
  // series from the loaded run pages while it loads or on error.
  const suitesQ = useSuites(group.project ?? undefined);
  const summary = group.suite
    ? suitesQ.data?.suites.find((s) => s.suite === group.suite)
    : undefined;
  const series =
    summary && summary.series.length > 0
      ? suitePassRateSeries(summary)
      : group.series;
  const baselineRunId = summary?.baseline_run_id ?? null;

  function toggle(id: string) {
    setSelected((prev) =>
      prev.includes(id)
        ? prev.filter((x) => x !== id)
        : [...prev, id].slice(-2), // keep at most the two most recent picks
    );
  }

  const pair = comparePair(group.runs, selected);

  return (
    <section className={cn(CHROME_FRAME, "overflow-hidden")}>
      <header className="flex items-center gap-3 border-b border-border px-4 py-2.5">
        <div className="min-w-0">
          {/* The group is named by the set it belongs to, so it links there.
              Only the text, not the whole header — the header also carries the
              Compare and Clear buttons. Heading colour rather than accent, to
              match the overview cards: this is the group's identity, not one
              more inline link. */}
          <div className="truncate text-sm font-semibold">
            {group.project && group.suite ? (
              <Link
                to={suitePath(group.project, group.suite)}
                className="rounded-sm hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              >
                {group.project}
                <span className="text-muted"> / </span>
                {group.suite}
              </Link>
            ) : (
              // A run that declared neither has no set to link to.
              <>
                {group.project}
                <span className="text-muted"> / </span>
                {group.suite}
              </>
            )}
          </div>
          <div className="text-xs text-muted">{group.runs.length} loaded runs</div>
        </div>
        <div className="ml-auto flex items-center gap-3">
          {selected.length > 0 ? (
            <div className="flex items-center gap-2">
              <span className="text-[11px] text-muted">
                {selected.length} selected
              </span>
              {pair ? (
                // Server contract: first url segment = base, second = head
                // (Path((id, other)) -> { base: id, head: other }). The
                // older run is the base; the newer is the head.
                <Link to={comparePath(pair.baseId, pair.headId)}>
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
          <span className="text-[11px] text-muted">pass-rate trend</span>
          <Sparkline
            values={series}
            min={0}
            max={1}
            width={120}
            height={28}
            title={`Pass rate trend for ${group.suite}`}
          />
        </div>
      </header>
      {/* `relative`: contains the Select column's `sr-only` header. Harmless
          today only because that column is leftmost; see SetsPage, where the
          same markup in a trailing column gave the page a sideways scroll. */}
      <div className="relative overflow-x-auto scroll-hint [--scroll-hint-bg:var(--bg)]">
        {/* `table-layout: fixed` is what makes the <colgroup> widths
            authoritative; under the default `auto` a column's width is
            advisory and a resize does nothing. */}
        <table className="w-full min-w-[1460px] table-fixed text-sm">
          <ColumnGroup columns={RUNS_COLUMNS} prefs={prefs} />
          <thead>
            <tr className="border-b border-border text-left text-xs uppercase tracking-wide text-muted">
              {shown.map((c, i, arr) => (
                <ResizableTh
                  isLast={i === arr.length - 1}
                  key={c.id}
                  def={c}
                  tableId={RUNS_TABLE_ID}
                  prefs={prefs}
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
            {group.runs.map((r) => {
              // Older run in the loaded group = the default compare base for
              // this row. Undefined when `r` is the oldest loaded run — the
              // real server has no route for a target-less compare, so the
              // link is hidden rather than pointing at one.
              const compareTarget = previousRun(group.runs, r.id);
              const fullyCached = isFullyCached(r);
              // Dim exactly what `cached=exclude` would have hidden, so the
              // revealed view reads as "these are the ones you normally don't
              // see". A cached run that failed is real signal (verdicts are
              // never cached) and stays at full contrast.
              const dimmed = hiddenByCachedExclude(r);
              return (
              <tr
                key={r.id}
                // Pointer convenience only; the checkbox and the Compare link
                // in this row keep their own clicks (see `useRowNav`).
                {...rowNav(runPath(r.id))}
                className={`cursor-pointer border-b border-border/60 last:border-0 hover:bg-surface-2${dimmed ? " opacity-60" : ""}`}
              >
                {shownIds.has("select") && (
                  <td className="px-3 py-2">
                    {/* The label is the hit target: padding a parent does not
                        extend an input's own 16px box, which is under WCAG 2.2's
                        24px minimum. */}
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
                {shownIds.has("run") && (
                  // `overflow-hidden` + `min-w-0` + `truncate`: this cell is a
                  // fixed-width flex row, so without them a run carrying both
                  // chips overlaps the next column instead of clipping — and
                  // the column is now something the reader can drag narrower.
                  <td className="overflow-hidden px-4 py-2">
                    <span className="flex min-w-0 items-center gap-1">
                      <Link
                        to={runPath(r.id)}
                        className="truncate font-medium text-accent hover:underline"
                      >
                        {r.id}
                      </Link>
                      <CopyButton value={r.id} label="Copy run id" iconOnly />
                      {r.id === baselineRunId ? (
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
                {shownIds.has("when") && (
                  <td className="px-3 py-2 text-muted">
                    <Tooltip content={formatDateAbsolute(r.created_at)}>
                      <time dateTime={r.created_at}>
                        {formatRelative(r.created_at)}
                      </time>
                    </Tooltip>
                  </td>
                )}
                {shownIds.has("who") && (
                  <td className="px-3 py-2">
                    <RunOriginCell run={r} />
                  </td>
                )}
                {shownIds.has("branch") && (
                  <td className="px-3 py-2">
                    <span className="font-mono text-xs">{r.git_branch ?? "-"}</span>
                    {r.git_commit ? (
                      <Tooltip content={r.git_commit}>
                        <span className="ml-1 font-mono text-[11px] text-muted">
                          @{r.git_commit.slice(0, 7)}
                        </span>
                      </Tooltip>
                    ) : null}
                    {/* An uncommitted worktree means this result cannot be
                        reproduced from the commit next to it — the one piece of
                        trust information a shared board most needs. */}
                    {r.git_dirty ? (
                      <Tooltip content="Ran against an uncommitted worktree; not reproducible from this commit">
                        <span className="ml-1 text-[11px] text-amber" aria-label="dirty worktree">
                          ⟡
                        </span>
                      </Tooltip>
                    ) : null}
                  </td>
                )}
                {shownIds.has("pass_rate") && (
                  <td className="px-3 py-2">
                    <PassRateBadge
                      pass={r.pass_count}
                      fail={r.fail_count}
                      error={r.error_count}
                      xpass={r.xpass_count}
                    />
                  </td>
                )}
                {shownIds.has("cases") && (
                  <td className="px-3 py-2 text-right tabular-nums">{r.case_count}</td>
                )}
                {shownIds.has("tokens") && (
                  <td className="px-3 py-2 text-right tabular-nums text-muted">
                    {formatTokens(r.prompt_tokens + r.completion_tokens)}
                  </td>
                )}
                {shownIds.has("cost") && (
                  <td className="px-3 py-2 text-right tabular-nums text-muted">
                    {formatCost(r.cost_usd)}
                  </td>
                )}
                {shownIds.has("duration") && (
                  <td className="px-3 py-2 text-right tabular-nums text-muted">
                    {formatDuration(r.duration_ms)}
                  </td>
                )}
                {shownIds.has("tags") && (
                  <td className="px-3 py-2">
                    <div className="flex flex-wrap gap-1">
                      {r.tags.map((t) => (
                        <Chip key={t} size="xs" className="normal-case">
                          {t}
                        </Chip>
                      ))}
                    </div>
                  </td>
                )}
                {shownIds.has("compare") && (
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
                        title="No earlier run in this suite to compare against"
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
    </section>
  );
}
