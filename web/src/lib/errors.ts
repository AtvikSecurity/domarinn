import type { ChipTone } from "@/components/ui/Chip";

/**
 * Turning errors from noise into signal.
 *
 * A run reporting "14 errors" tells you nothing actionable. The classes group
 * by *owner*: `provider_*` and `cache_*` are infrastructure — not your model's
 * fault, and usually not worth waking anyone; `grader_*` means the eval did not
 * actually run, so the scores beside them are not evidence of anything.
 */

/** One class and how many cases hit it. */
export interface ErrorTally {
  class: string;
  count: number;
  /** Share of all cases in the run, 0..1. */
  share: number;
}

/**
 * Tone by prefix, mirroring the Rust `ErrorClass::is_infrastructure`.
 *
 * Amber for infrastructure and red for everything else is the opinionated bit:
 * a rate limit is a retry, a broken grader means your results are suspect, and
 * rendering both the same colour is what makes people ignore the count.
 */
export function errorClassTone(cls: string): ChipTone {
  if (cls.startsWith("provider_") || cls.startsWith("cache_") || cls === "exec_failed") {
    return "amber";
  }
  return "error";
}

/** A short human label: `provider_rate_limit` → `provider · rate limit`. */
export function errorClassLabel(cls: string): string {
  const [head, ...rest] = cls.split("_");
  if (rest.length === 0) return cls.replace(/_/g, " ");
  return `${head} · ${rest.join(" ")}`;
}

/**
 * Group a run's cases by error class, most frequent first.
 *
 * Cases that errored before classes existed have no class; they are tallied
 * under `unknown` rather than dropped, so the totals still add up to the run's
 * error count and a reader is not left wondering where the rest went.
 */
export function aggregateErrorClasses(
  cases: { status: string; error_class?: string | null }[],
  totalCases: number,
): ErrorTally[] {
  const counts = new Map<string, number>();
  for (const c of cases) {
    if (c.status !== "error") continue;
    const key = c.error_class ?? "unknown";
    counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  return [...counts.entries()]
    .map(([cls, count]) => ({
      class: cls,
      count,
      share: totalCases === 0 ? 0 : count / totalCases,
    }))
    .sort((a, b) => b.count - a.count || a.class.localeCompare(b.class));
}

/** Below this share of a run's cases, an error breakdown starts collapsed. */
export const ERROR_NOISE_THRESHOLD = 0.01;
