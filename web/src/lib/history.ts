import type { CaseHistoryPoint } from "@/api";

/**
 * Pure helpers over a case's history window.
 *
 * The API returns points newest-first; everything here takes them in
 * **chronological** order (oldest → newest), which is how they are read on
 * screen. Callers reverse once, at the edge.
 */

export type HistoryMetric = "score" | "latency" | "cost" | "tokens";

export interface HistoryMetricSpec {
  value: HistoryMetric;
  label: string;
  /** Whether a rising series is good — drives the sparkline's trend colour. */
  higherIsBetter: boolean;
  /** Fixed axis bounds, where the metric has them. */
  min?: number;
  max?: number;
}

/**
 * The metrics a history window can plot. Latency and cost are the reason
 * `higherIsBetter` exists: a rising latency series is a regression, and
 * colouring it green would be actively misleading.
 */
export const HISTORY_METRICS: readonly HistoryMetricSpec[] = [
  { value: "score", label: "Score", higherIsBetter: true, min: 0, max: 1 },
  { value: "latency", label: "Latency", higherIsBetter: false },
  { value: "cost", label: "Cost", higherIsBetter: false },
  { value: "tokens", label: "Tokens", higherIsBetter: false },
];

export function metricSpec(metric: HistoryMetric): HistoryMetricSpec {
  return HISTORY_METRICS.find((m) => m.value === metric) ?? HISTORY_METRICS[0]!;
}

/**
 * One value per point, `null` where the point has no reading. Nulls are kept
 * rather than filtered so the series stays index-aligned with the status
 * squares rendered above it.
 */
export function historySeries(
  points: readonly CaseHistoryPoint[],
  metric: HistoryMetric,
): (number | null)[] {
  return points.map((p) => {
    switch (metric) {
      case "score":
        return p.score;
      case "latency":
        return p.latency_ms;
      case "cost":
        return p.cost_usd;
      case "tokens": {
        // Either half may be present alone; only a total absence is a gap.
        if (p.prompt_tokens == null && p.completion_tokens == null) return null;
        return (p.prompt_tokens ?? 0) + (p.completion_tokens ?? 0);
      }
    }
  });
}

export interface HistoryChange {
  /** The suite's git commit differs from the previous (older) run's. */
  commitChanged: boolean;
  /** The resolved config digest differs from the previous run's. */
  configChanged: boolean;
}

/**
 * Per-point markers for "something outside this case changed here".
 *
 * This is what turns a row of coloured squares into an explanation: read
 * together with the status timeline it says *"it started failing at `abc1234`,
 * where the config also changed"* — attribution the UI already had the data for
 * and never showed. Index 0 is always false (nothing older to compare against).
 */
export function changePoints(
  points: readonly CaseHistoryPoint[],
): HistoryChange[] {
  return points.map((p, i) => {
    const prev = i > 0 ? points[i - 1] : undefined;
    if (!prev) return { commitChanged: false, configChanged: false };
    return {
      // A missing value on either side is unknown, not "changed".
      commitChanged:
        p.git_commit != null &&
        prev.git_commit != null &&
        p.git_commit !== prev.git_commit,
      configChanged:
        p.config_digest != null &&
        prev.config_digest != null &&
        p.config_digest !== prev.config_digest,
    };
  });
}

export interface HistorySummary {
  runs: number;
  outputChanges: number;
  /** First and last non-null score in the window, if there are at least two. */
  firstScore?: number;
  lastScore?: number;
}

export function historySummary(
  points: readonly CaseHistoryPoint[],
): HistorySummary {
  const scored = points
    .map((p) => p.score)
    .filter((s): s is number => s != null);
  const summary: HistorySummary = {
    runs: points.length,
    outputChanges: points.filter((p) => p.output_changed === true).length,
  };
  if (scored.length >= 2) {
    summary.firstScore = scored[0];
    summary.lastScore = scored[scored.length - 1];
  }
  return summary;
}

/**
 * Prompt/completion split for a token total. A prompt-heavy case is a cost
 * problem; an output-heavy one is a truncation risk. Summed, they are
 * indistinguishable — which is how every surface displayed them.
 */
export function tokenSplit(
  usage: { input_tokens?: number; output_tokens?: number } | null | undefined,
): { input: number; output: number; total: number } | null {
  if (!usage) return null;
  const input = usage.input_tokens ?? 0;
  const output = usage.output_tokens ?? 0;
  return { input, output, total: input + output };
}
