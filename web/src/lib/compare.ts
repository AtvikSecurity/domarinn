import type { CaseStatus, CompareDelta, RunListItem } from "@/api";
import { parseTimestamp } from "./format";

/**
 * Pick the default compare target for `runId` out of a list of runs (usually
 * the other runs already loaded for the same project/suite): the run
 * immediately OLDER than it by `created_at`.
 *
 * Returns `undefined` when `runId` isn't in `runs` or is already the oldest
 * one present — callers must treat that as "no target" and skip rendering a
 * compare link rather than navigating to a target-less `/runs/{id}/compare`
 * URL, which has no route on the real server (`Path((id, other))` requires
 * both segments) and 404s.
 */
export function previousRun(
  runs: RunListItem[],
  runId: string,
): RunListItem | undefined {
  const byNewestFirst = [...runs].sort(
    (a, b) => parseTimestamp(b.created_at) - parseTimestamp(a.created_at),
  );
  const idx = byNewestFirst.findIndex((r) => r.id === runId);
  if (idx === -1) return undefined;
  return byNewestFirst[idx + 1];
}

/** A status counts as "failing" for delta purposes when it is fail or error. */
export function isFailing(status: CaseStatus | null): boolean {
  return status === "fail" || status === "error";
}

/**
 * Classify how a case moved from a base run to a head run. Pure — mirrors
 * `classify` in the server's `storage::compare` module exactly (see that
 * module's doc comment): pass/pass is its own `still_passing` variant, and
 * anything else with matching presence-but-non-failing/non-passing status
 * (in practice, a `skip` on either side) falls back to `unchanged`. Used by
 * both the mock (to compute deltas the same way the server does) and tested
 * in isolation.
 */
export function classifyDelta(
  base: CaseStatus | null,
  head: CaseStatus | null,
): CompareDelta {
  if (base === null && head === null) return "unchanged";
  if (head === null) return "removed";
  if (base === null) return "added";

  const baseFail = isFailing(base);
  const headFail = isFailing(head);

  if (baseFail && headFail) return "still_failing";
  if (!baseFail && headFail) return "newly_failing";
  if (baseFail && !headFail) return "newly_passing";
  if (base === "pass" && head === "pass") return "still_passing";
  return "unchanged";
}

/** Delta groups that the compare summary chips can filter by. */
export const COMPARE_FILTER_DELTAS: CompareDelta[] = [
  "newly_failing",
  "newly_passing",
  "still_failing",
  "added",
  "removed",
];

export const DELTA_LABEL: Record<CompareDelta, string> = {
  newly_failing: "Newly failing",
  newly_passing: "Newly passing",
  still_failing: "Still failing",
  still_passing: "Still passing",
  unchanged: "Unchanged",
  added: "Added",
  removed: "Removed",
};
