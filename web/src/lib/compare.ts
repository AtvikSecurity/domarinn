import type { CaseStatus, CompareDelta } from "@/api/types";

/** A status counts as "failing" for delta purposes when it is fail or error. */
export function isFailing(status: CaseStatus | null): boolean {
  return status === "fail" || status === "error";
}

/**
 * Classify how a case moved from a base run to a head run. Pure — this is the
 * same rule the backend applies, mirrored here for the mock and tested in
 * isolation.
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
  unchanged: "Unchanged",
  added: "Added",
  removed: "Removed",
};
