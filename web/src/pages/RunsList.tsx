import { useMemo } from "react";
import { Link, useSearchParams } from "react-router";
import { useRuns } from "@/api/queries";
import type { RunSummaryRow } from "@/api/types";
import { parseRunsFilters } from "@/lib/filters";
import {
  formatCost,
  formatDuration,
  formatRelative,
  formatTokens,
  passRate,
} from "@/lib/format";
import { RunsFilterBar } from "@/components/RunsFilterBar";
import { Sparkline } from "@/components/Sparkline";
import { PassRateBadge } from "@/components/PassRateBadge";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { ErrorState, EmptyState } from "@/components/States";
import { Button } from "@/components/ui/Button";

interface Group {
  key: string;
  project: string;
  suite: string;
  runs: RunSummaryRow[]; // newest first
  series: number[]; // pass rate oldest -> newest
  latest: number;
}

function groupRuns(runs: RunSummaryRow[]): Group[] {
  const map = new Map<string, RunSummaryRow[]>();
  for (const r of runs) {
    const key = `${r.project}/${r.suite}`;
    (map.get(key) ?? map.set(key, []).get(key)!).push(r);
  }
  const groups: Group[] = [];
  for (const [key, list] of map) {
    const byDateAsc = [...list].sort((a, b) => a.created_at - b.created_at);
    groups.push({
      key,
      project: list[0].project,
      suite: list[0].suite,
      runs: [...list].sort((a, b) => b.created_at - a.created_at),
      series: byDateAsc.map(
        (r) => passRate(r.pass_count, r.fail_count, r.error_count) ?? 0,
      ),
      latest: Math.max(...list.map((r) => r.created_at)),
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
        <div className="ml-auto flex items-center gap-2">
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
        <table className="w-full min-w-[880px] text-sm">
          <thead>
            <tr className="border-b border-border text-left text-xs uppercase tracking-wide text-muted">
              <th className="px-4 py-2 font-medium">Run</th>
              <th className="px-3 py-2 font-medium">When</th>
              <th className="px-3 py-2 font-medium">Branch</th>
              <th className="px-3 py-2 font-medium">Pass rate</th>
              <th className="px-3 py-2 text-right font-medium">Cases</th>
              <th className="px-3 py-2 text-right font-medium">Tokens</th>
              <th className="px-3 py-2 text-right font-medium">Cost</th>
              <th className="px-3 py-2 text-right font-medium">Duration</th>
              <th className="px-3 py-2 font-medium">Tags</th>
            </tr>
          </thead>
          <tbody>
            {group.runs.map((r) => (
              <tr
                key={r.id}
                className="border-b border-border/60 last:border-0 hover:bg-surface-2"
              >
                <td className="px-4 py-2">
                  <Link
                    to={`/runs/${encodeURIComponent(r.id)}`}
                    className="font-medium text-accent hover:underline"
                  >
                    {r.id}
                  </Link>
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
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}
