// Presentation helpers derived from `SuiteSummary.series` (the generated
// `SuitePoint[]` the real `GET /projects/{project}/suites` endpoint returns).
//
// The hand-written mock/type used to bake `pass_rate_series: number[]` and
// `last_run_id` directly onto `SuiteSummary`; the real DTO only carries
// `series`, so any UI wanting those presentation values now derives them
// client-side from it.

import type { SuiteSummary } from "@/api";

/**
 * Recent pass-rate series (0..1), oldest -> newest, for a sparkline.
 * `SuiteSummary.series` itself is newest-first (see the generated type's doc
 * comment), so this reverses it.
 */
export function suitePassRateSeries(suite: SuiteSummary): number[] {
  return [...suite.series].reverse().map((p) => p.pass_rate);
}

/** The suite's most recent run id, or `undefined` if it has no runs yet. */
export function suiteLastRunId(suite: SuiteSummary): string | undefined {
  return suite.series[0]?.run_id;
}
