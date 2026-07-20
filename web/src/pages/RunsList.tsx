import { useMemo, useState } from "react";
import { Link, useSearchParams } from "react-router";
import { useRuns } from "@/api/queries";
import type { RunListItem } from "@/api";
import { parseRunsFilters } from "@/lib/filters";
import { previousRun } from "@/lib/compare";
import {
  formatCost,
  formatDuration,
  formatRelative,
  formatTokens,
  parseTimestamp,
  passRate,
} from "@/lib/format";
import { RunsFilterBar } from "@/components/RunsFilterBar";
import { Sparkline } from "@/components/Sparkline";
import { PassRateBadge } from "@/components/PassRateBadge";
import { CopyButton } from "@/components/ui/CopyButton";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { ErrorState, EmptyState } from "@/components/States";
import { Button } from "@/components/ui/Button";

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
  const [params] = useSearchParams();
  const filters = parseRunsFilters(params);
  const q = useRuns(filters);

  const runs = useMemo(
    () => q.data?.pages.flatMap((p) => p.runs) ?? [],
    [q.data],
  );
  const groups = useMemo(() => groupRuns(runs), [runs]);

  return (
    <div className="space-y-5">
      <div>
        <h1 className="text-lg font-semibold tracking-tight">Eval runs</h1>
        <p className="text-sm text-muted">
          Browse runs grouped by suite. Filters live in the URL and are shareable.
        </p>
      </div>

      <RunsFilterBar />

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

  function toggle(id: string) {
    setSelected((prev) =>
      prev.includes(id)
        ? prev.filter((x) => x !== id)
        : [...prev, id].slice(-2), // keep at most the two most recent picks
    );
  }

  const pair = comparePair(group.runs, selected);

  return (
    <section className="overflow-hidden rounded-xl border border-border bg-surface">
      <header className="flex items-center gap-3 border-b border-border px-4 py-2.5">
        <div className="min-w-0">
          <div className="truncate text-sm font-semibold">
            {group.project}
            <span className="text-muted"> / </span>
            {group.suite}
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
                <Link to={`/runs/${encodeURIComponent(pair.baseId)}/compare/${encodeURIComponent(pair.headId)}`}>
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
            values={group.series}
            min={0}
            max={1}
            width={120}
            height={28}
            title={`Pass rate trend for ${group.suite}`}
          />
        </div>
      </header>
      <div className="overflow-x-auto">
        <table className="w-full min-w-[940px] text-sm">
          <thead>
            <tr className="border-b border-border text-left text-xs uppercase tracking-wide text-muted">
              <th className="w-8 px-3 py-2 font-medium">
                <span className="sr-only">Select</span>
              </th>
              <th className="px-4 py-2 font-medium">Run</th>
              <th className="px-3 py-2 font-medium">When</th>
              <th className="px-3 py-2 font-medium">Branch</th>
              <th className="px-3 py-2 font-medium">Pass rate</th>
              <th className="px-3 py-2 text-right font-medium">Cases</th>
              <th className="px-3 py-2 text-right font-medium">Tokens</th>
              <th className="px-3 py-2 text-right font-medium">Cost</th>
              <th className="px-3 py-2 text-right font-medium">Duration</th>
              <th className="px-3 py-2 font-medium">Tags</th>
              <th className="px-3 py-2 text-right font-medium">Compare</th>
            </tr>
          </thead>
          <tbody>
            {group.runs.map((r) => {
              // Older run in the loaded group = the default compare base for
              // this row. Undefined when `r` is the oldest loaded run — the
              // real server has no route for a target-less compare, so the
              // link is hidden rather than pointing at one.
              const compareTarget = previousRun(group.runs, r.id);
              return (
              <tr
                key={r.id}
                className="border-b border-border/60 last:border-0 hover:bg-surface-2"
              >
                <td className="px-3 py-2">
                  <input
                    type="checkbox"
                    aria-label={`Select run ${r.id}`}
                    checked={selected.includes(r.id)}
                    onChange={() => toggle(r.id)}
                    className="size-4 accent-[var(--color-accent)]"
                  />
                </td>
                <td className="px-4 py-2">
                  <span className="flex items-center gap-1">
                    <Link
                      to={`/runs/${encodeURIComponent(r.id)}`}
                      className="font-medium text-accent hover:underline"
                    >
                      {r.id}
                    </Link>
                    <CopyButton value={r.id} label="Copy run id" iconOnly />
                  </span>
                </td>
                <td className="px-3 py-2 text-muted">
                  {formatRelative(r.created_at)}
                </td>
                <td className="px-3 py-2">
                  <span className="font-mono text-xs">{r.git_branch ?? "-"}</span>
                  {r.git_commit ? (
                    <span className="ml-1 font-mono text-[11px] text-muted">
                      @{r.git_commit}
                    </span>
                  ) : null}
                </td>
                <td className="px-3 py-2">
                  <PassRateBadge
                    pass={r.pass_count}
                    fail={r.fail_count}
                    error={r.error_count}
                  />
                </td>
                <td className="px-3 py-2 text-right tabular-nums">{r.case_count}</td>
                <td className="px-3 py-2 text-right tabular-nums text-muted">
                  {formatTokens(r.prompt_tokens + r.completion_tokens)}
                </td>
                <td className="px-3 py-2 text-right tabular-nums text-muted">
                  {formatCost(r.cost_usd)}
                </td>
                <td className="px-3 py-2 text-right tabular-nums text-muted">
                  {formatDuration(r.duration_ms)}
                </td>
                <td className="px-3 py-2">
                  <div className="flex flex-wrap gap-1">
                    {r.tags.map((t) => (
                      <span
                        key={t}
                        className="rounded bg-surface-2 px-1.5 py-0.5 text-[11px] text-muted"
                      >
                        {t}
                      </span>
                    ))}
                  </div>
                </td>
                <td className="px-3 py-2 text-right">
                  {compareTarget ? (
                    <Link
                      to={`/runs/${encodeURIComponent(compareTarget.id)}/compare/${encodeURIComponent(r.id)}`}
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
              </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </section>
  );
}
