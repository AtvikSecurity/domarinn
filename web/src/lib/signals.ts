import type { RunListItem } from "@/api";

/**
 * Deriving a suite's health from its runs.
 *
 * Every decision here is a pure function so it can be unit-tested, because the
 * e2e suite is a coarse net and these are the rules people will argue about:
 * which run counts as canonical, when a suite is stale, how cards are ordered.
 */

/** A run is canonical when CI produced it. `ci_provider` is the exact signal. */
export function isCanonical(run: Pick<RunListItem, "ci_provider">): boolean {
  return run.ci_provider != null;
}

/**
 * The run a suite's status should be read from: the newest CI run, preferring
 * the default branch.
 *
 * Falls back to any CI run when none is on the default branch (a suite whose CI
 * only runs on PRs is still better represented by CI than by someone's laptop),
 * and returns undefined when there is no CI run at all — the caller must say
 * "no CI run yet" rather than promoting a developer run to canonical.
 */
export function canonicalRun(
  runs: RunListItem[],
  defaultBranch = "main",
): RunListItem | undefined {
  // Sorted rather than trusting the caller's order. The server happens to
  // return newest-first today, so `find`/`[0]` would be correct by accident —
  // and any reordering (merged pages, a client-side sort) would silently
  // promote an ancient run to "current status" with nothing to catch it.
  const ci = runs
    .filter(isCanonical)
    .slice()
    .sort((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at));
  return ci.find((r) => r.git_branch === defaultBranch) ?? ci[0];
}

/**
 * Pass rate as a fraction, excluding skipped cases (matching `lib/format`).
 *
 * `null` when the run graded nothing — every case filtered out or skipped.
 * Returning 0 there would be indistinguishable from "everything failed", so a
 * run that judged nothing would read as a total regression.
 */
function rate(run: RunListItem): number | null {
  const denom = run.pass_count + run.fail_count + run.error_count;
  return denom === 0 ? null : run.pass_count / denom;
}

/**
 * Pass rates of the canonical runs, oldest first — the series a trend line
 * should show.
 *
 * Mixing CI with developer iteration is what made the existing suite sparkline
 * meaningless: a half-broken scratch run drags the "trend" down and the line
 * says nothing about whether the product regressed.
 */
export function canonicalSeries(
  runs: RunListItem[],
  defaultBranch = "main",
): number[] {
  // Scoped to the same branch `canonicalRun` picks, or the card's headline and
  // its trend/delta are computed from different runs: a green main run beside a
  // just-finished PR-branch run would show 100% with a ▼40pt delta and an amber
  // "drifting" border, none of which describe the run it names.
  const ci = runs.filter(isCanonical);
  const branch = ci.some((r) => r.git_branch === defaultBranch)
    ? defaultBranch
    : ci[0]?.git_branch;
  return ci
    .filter((r) => r.git_branch === branch)
    .slice()
    .sort((a, b) => Date.parse(a.created_at) - Date.parse(b.created_at))
    .map(rate)
    .filter((r): r is number => r !== null);
}

/** Median of a numeric list; 0 for an empty one. */
function median(values: number[]): number {
  if (values.length === 0) return 0;
  const sorted = values.slice().sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0
    ? ((sorted[mid - 1] ?? 0) + (sorted[mid] ?? 0)) / 2
    : (sorted[mid] ?? 0);
}

/** Never call a suite stale sooner than this, however fast its CI usually is. */
const STALE_FLOOR_MS = 24 * 60 * 60 * 1000;

/**
 * Whether a suite's CI appears to have stopped.
 *
 * Relative to the suite's own cadence — an hourly suite silent for a day is
 * broken, a weekly one is not — with a floor so a fast suite is not called
 * stale over lunch. Needs at least two canonical runs to have a cadence at all.
 *
 * This is the state nothing in the product could previously express: a suite
 * whose CI died last week renders as a serene 100%.
 */
export function isStale(runs: RunListItem[], now: number): boolean {
  const ci = runs
    .filter(isCanonical)
    .map((r) => Date.parse(r.created_at))
    .sort((a, b) => b - a);
  const newest = ci[0];
  if (newest === undefined || ci.length < 2) return false;

  const gaps: number[] = [];
  for (let i = 1; i < ci.length; i++) gaps.push((ci[i - 1] ?? 0) - (ci[i] ?? 0));
  const expected = Math.max(3 * median(gaps), STALE_FLOOR_MS);
  return now - newest > expected;
}

export type Severity = "failing" | "stale" | "drifting" | "healthy" | "unknown";

/**
 * How loudly a suite should shout, worst first.
 *
 * Cards sort by this and NEVER by recency. Sorting a status surface by
 * `max(created_at)` — which the runs list does — means the page reorders every
 * time somebody iterates locally, so the thing you were reading moves.
 */
export function suiteSeverity(runs: RunListItem[], now: number): Severity {
  const canonical = canonicalRun(runs);
  if (!canonical) return "unknown";
  if (canonical.fail_count > 0 || canonical.error_count > 0) return "failing";
  if (isStale(runs, now)) return "stale";
  const series = canonicalSeries(runs);
  const last = series[series.length - 1];
  const prev = series[series.length - 2];
  if (last !== undefined && prev !== undefined && last < prev) return "drifting";
  return "healthy";
}

const SEVERITY_ORDER: Record<Severity, number> = {
  failing: 0,
  stale: 1,
  drifting: 2,
  unknown: 3,
  healthy: 4,
};

export function severityRank(s: Severity): number {
  return SEVERITY_ORDER[s];
}

/**
 * Change in pass rate against the previous canonical run, in percentage points.
 * `null` when there is nothing to compare against — not 0, which would read as
 * "no change" for a suite's very first run.
 */
export function canonicalDelta(runs: RunListItem[]): number | null {
  const series = canonicalSeries(runs);
  const last = series[series.length - 1];
  const prev = series[series.length - 2];
  if (last === undefined || prev === undefined) return null;
  return (last - prev) * 100;
}
