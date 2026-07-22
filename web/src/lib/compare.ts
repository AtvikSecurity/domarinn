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

/** Unicode minus (U+2212) — reads better than a hyphen next to a number, and
 *  matches the aggregate-delta formatting. */
const MINUS = "−";

/** How a score delta should render: `tone` drives the tint (pass tint for a
 *  gain, fail for a regression, muted for zero/absent), `text` is the signed
 *  2-dec magnitude (or an em-dash when either score was missing). */
export interface ScoreDeltaDisplay {
  text: string;
  tone: "pass" | "fail" | "muted";
}

/**
 * Format a per-case `score_delta` (`head_score - base_score`, or `null` when a
 * score was absent on either side) for the compare grid. Pure so it can be
 * unit-tested and shared. `null` ⇒ em-dash, muted; `0` ⇒ unsigned `0.00`,
 * muted; positive ⇒ `+X.XX`, pass tint; negative ⇒ `−X.XX`, fail tint.
 */
export function formatScoreDelta(delta: number | null): ScoreDeltaDisplay {
  if (delta === null) return { text: "—", tone: "muted" };
  const sign = delta > 0 ? "+" : delta < 0 ? MINUS : "";
  return {
    text: `${sign}${Math.abs(delta).toFixed(2)}`,
    tone: delta > 0 ? "pass" : delta < 0 ? "fail" : "muted",
  };
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
