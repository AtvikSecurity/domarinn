import { useEffect, useMemo, useState } from "react";

/** Pages of 50 runs to pull before giving up (see the effect below). */
const MAX_PAGES = 10;
import type { RunListItem } from "@/api";
import { useRuns } from "@/api/queries";
import { severityRank, suiteSeverity } from "@/lib/signals";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { ErrorState, EmptyState } from "@/components/States";
import { SuiteHealthCard } from "./overview/SuiteHealthCard";

/**
 * "What is the state of everything right now?"
 *
 * Separate from the runs list rather than folded into it, for a structural
 * reason: the list sorts suites by `max(created_at)`, so any developer push
 * reorders the page. A status surface must not move because somebody is
 * iterating — so these cards sort by SEVERITY and never by recency.
 *
 * Every number here links into `/runs`. This is an index, not a second source
 * of truth.
 */
export function OverviewPage() {
  // `cached=all` deliberately: the runs list hides fully-cached passing runs as
  // stream noise, but a suite whose latest CI run was fully cached still has a
  // status, and hiding it here would make the suite look stale.
  const runs = useRuns({ cached: "all" });

  // Auto-page. A single 50-run page describes only the busiest suites: with
  // more than 50 runs newer than a quiet suite's last run, that suite gets no
  // card at all — so a suite whose CI died would *disappear* instead of showing
  // the stale chip this page exists to show. Bounded so a large server cannot
  // turn the landing page into an unbounded fetch; past the bound the oldest
  // suites are simply absent, which is the same failure, only rarer.
  useEffect(() => {
    if (runs.hasNextPage && !runs.isFetchingNextPage) {
      if ((runs.data?.pages.length ?? 0) < MAX_PAGES) void runs.fetchNextPage();
    }
  }, [runs]);
  // Sampled once at mount, not per render: staleness and severity are derived
  // from it, and a clock that advances mid-render would let a card change
  // category between two renders of the same data.
  const [now] = useState(() => Date.now());

  const suites = useMemo(() => {
    const all: RunListItem[] = (runs.data?.pages ?? []).flatMap((p) => p.runs);
    const groups = new Map<string, { project: string; suite: string; runs: RunListItem[] }>();
    for (const r of all) {
      const project = r.project ?? "(no project)";
      const suite = r.suite ?? "(no suite)";
      const key = `${project}/${suite}`;
      const g = groups.get(key) ?? { project, suite, runs: [] };
      g.runs.push(r);
      groups.set(key, g);
    }
    return [...groups.values()].sort((a, b) => {
      const rank =
        severityRank(suiteSeverity(a.runs, now)) - severityRank(suiteSeverity(b.runs, now));
      return rank !== 0
        ? rank
        : `${a.project}/${a.suite}`.localeCompare(`${b.project}/${b.suite}`);
    });
  }, [runs.data, now]);

  if (runs.isPending) return <CenteredSpinner />;
  if (runs.isError) {
    return <ErrorState error={runs.error} onRetry={() => void runs.refetch()} />;
  }
  if (suites.length === 0) {
    return (
      <EmptyState title="No runs yet">
        Upload a run with <code>domarinn run --share</code> and its suite will
        appear here.
      </EmptyState>
    );
  }

  return (
    <div className="space-y-4">
      <div>
        <h1 className="text-lg font-semibold tracking-tight">Overview</h1>
        <p className="mt-0.5 text-sm text-muted">
          Each suite's latest CI run. Sorted by what needs attention, not by
          what ran most recently.
        </p>
      </div>
      <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
        {suites.map((s) => (
          <SuiteHealthCard
            key={`${s.project}/${s.suite}`}
            project={s.project}
            suite={s.suite}
            runs={s.runs}
            now={now}
          />
        ))}
      </div>
    </div>
  );
}
