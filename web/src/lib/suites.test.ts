import { describe, expect, it } from "vitest";
import { suiteLastRunId, suitePassRateSeries } from "./suites";
import type { SuiteSummary } from "@/api";

// Regression guard for the SuiteSummary drift bug: the real server returns
// `series: SuitePoint[]` (newest-first), not the hand-written
// `pass_rate_series: number[]` / `last_run_id` fields the UI used to expect.

function suite(overrides: Partial<SuiteSummary> = {}): SuiteSummary {
  return {
    suite: "regression",
    run_count: 3,
    last_run_at: "2026-07-19T15:00:00Z",
    baseline_run_id: "run-2",
    series: [
      { run_id: "run-3", created_at: "2026-07-19T15:00:00Z", total: 100, passed: 95, pass_rate: 0.95 },
      { run_id: "run-2", created_at: "2026-07-18T15:00:00Z", total: 100, passed: 90, pass_rate: 0.9 },
      { run_id: "run-1", created_at: "2026-07-17T15:00:00Z", total: 100, passed: 80, pass_rate: 0.8 },
    ],
    ...overrides,
  };
}

describe("suitePassRateSeries", () => {
  it("reverses the newest-first series into oldest -> newest for a sparkline", () => {
    expect(suitePassRateSeries(suite())).toEqual([0.8, 0.9, 0.95]);
  });

  it("returns an empty array for a suite with no runs", () => {
    expect(suitePassRateSeries(suite({ series: [] }))).toEqual([]);
  });
});

describe("suiteLastRunId", () => {
  it("is the newest point's run id (series[0])", () => {
    expect(suiteLastRunId(suite())).toBe("run-3");
  });

  it("is undefined for a suite with no runs", () => {
    expect(suiteLastRunId(suite({ series: [] }))).toBeUndefined();
  });
});
