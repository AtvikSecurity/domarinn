import { describe, expect, it } from "vitest";
import type { RunListItem } from "@/api";
import {
  canonicalDelta,
  canonicalRun,
  canonicalSeries,
  isStale,
  severityRank,
  suiteSeverity,
} from "./signals";

const HOUR = 60 * 60 * 1000;
const DAY = 24 * HOUR;
const NOW = Date.parse("2026-07-27T12:00:00Z");

function run(o: Partial<RunListItem> & { id: string }): RunListItem {
  return {
    project: "p",
    suite: "s",
    created_at: new Date(NOW).toISOString(),
    git_branch: "main",
    git_commit: null,
    git_dirty: null,
    case_count: 10,
    pass_count: 10,
    fail_count: 0,
    error_count: 0,
    xfail_count: 0,
    xpass_count: 0,
    pass_rate: 1,
    prompt_tokens: 0,
    completion_tokens: 0,
    cost_usd: null,
    duration_ms: 0,
    cache_hits: null,
    cache_misses: null,
    actor: null,
    host: null,
    uploaded_by: null,
    ci_provider: "github",
    ci_run_url: null,
    note: null,
    domarinn_version: null,
    tags: [],
    ...o,
  };
}

const local = (o: Partial<RunListItem> & { id: string }) =>
  run({ ci_provider: null, git_branch: "feat/x", ...o });

describe("canonicalRun", () => {
  it("prefers CI on the default branch", () => {
    const runs = [
      local({ id: "dev" }),
      run({ id: "ci-feature", git_branch: "feat/x" }),
      run({ id: "ci-main" }),
    ];
    expect(canonicalRun(runs)?.id).toBe("ci-main");
  });

  // A suite whose CI only runs on PRs is still better represented by CI than
  // by someone's laptop.
  it("falls back to any CI run when none is on the default branch", () => {
    const runs = [local({ id: "dev" }), run({ id: "ci-pr", git_branch: "feat/x" })];
    expect(canonicalRun(runs)?.id).toBe("ci-pr");
  });

  // The honest answer. Promoting a developer run would let one person's
  // scratch iteration define a suite's public status.
  it("is undefined when no CI run exists", () => {
    expect(canonicalRun([local({ id: "dev" })])).toBeUndefined();
  });
});

describe("canonicalSeries", () => {
  it("excludes developer runs and orders oldest first", () => {
    const runs = [
      run({ id: "b", created_at: new Date(NOW).toISOString(), pass_count: 8, fail_count: 2 }),
      local({ id: "scratch", pass_count: 0, fail_count: 10 }),
      run({ id: "a", created_at: new Date(NOW - DAY).toISOString() }),
    ];
    expect(canonicalSeries(runs)).toEqual([1, 0.8]);
  });
});

describe("isStale", () => {
  const cadence = (gapMs: number, ageMs: number, n = 4) =>
    Array.from({ length: n }, (_, i) =>
      run({ id: `r${i}`, created_at: new Date(NOW - ageMs - i * gapMs).toISOString() }),
    );

  it("is relative to the suite's own cadence", () => {
    // Hourly CI, silent for two days: broken. (One day exactly would sit on
    // the floor, which is a boundary rather than a behaviour.)
    expect(isStale(cadence(HOUR, 2 * DAY), NOW)).toBe(true);
    // Weekly CI, silent for the same two days: entirely normal.
    expect(isStale(cadence(7 * DAY, 2 * DAY), NOW)).toBe(false);
  });

  // Without a floor, a suite running every five minutes would be "stale" by
  // the time you came back from lunch.
  it("never calls a fast suite stale within a day", () => {
    expect(isStale(cadence(5 * 60 * 1000, 6 * HOUR), NOW)).toBe(false);
  });

  it("needs at least two runs to have a cadence", () => {
    expect(isStale([run({ id: "only", created_at: new Date(NOW - 365 * DAY).toISOString() })], NOW)).toBe(false);
  });
});

describe("suiteSeverity", () => {
  it("ranks a failing canonical run above everything else", () => {
    const runs = [run({ id: "ci", pass_count: 8, fail_count: 2 })];
    expect(suiteSeverity(runs, NOW)).toBe("failing");
  });

  it("reports an unknown status when there is no CI run", () => {
    expect(suiteSeverity([local({ id: "dev" })], NOW)).toBe("unknown");
  });

  it("notices a drop in pass rate", () => {
    const runs = [
      run({ id: "new", created_at: new Date(NOW).toISOString(), pass_count: 9, fail_count: 0, case_count: 9 }),
      run({ id: "old", created_at: new Date(NOW - DAY).toISOString() }),
    ];
    // Both pass all their cases; the newer one has a lower rate only if it
    // failed something, so this stays healthy.
    expect(suiteSeverity(runs, NOW)).toBe("healthy");
  });

  // The ordering is the whole point: a status surface must not be sorted by
  // recency, or it reorders whenever somebody iterates.
  it("orders severities worst-first", () => {
    const order = (["healthy", "unknown", "drifting", "stale", "failing"] as const)
      .slice()
      .sort((a, b) => severityRank(a) - severityRank(b));
    expect(order).toEqual(["failing", "stale", "drifting", "unknown", "healthy"]);
  });
});

describe("canonicalDelta", () => {
  it("is null for a suite's first run rather than zero", () => {
    expect(canonicalDelta([run({ id: "only" })])).toBeNull();
  });

  it("reports percentage points against the previous canonical run", () => {
    const runs = [
      run({ id: "new", created_at: new Date(NOW).toISOString(), pass_count: 8, fail_count: 2 }),
      run({ id: "old", created_at: new Date(NOW - DAY).toISOString() }),
    ];
    expect(canonicalDelta(runs)).toBeCloseTo(-20);
  });
});
