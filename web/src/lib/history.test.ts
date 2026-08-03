import { describe, expect, it } from "vitest";
import type { CaseHistoryPoint } from "@/api";
import {
  changePoints,
  collapseCachedRuns,
  historySeries,
  historySummary,
  metricSpec,
  tokenSplit,
} from "./history";

function point(over: Partial<CaseHistoryPoint> = {}): CaseHistoryPoint {
  return {
    run_id: "run-01",
    created_at: "2026-07-01T00:00:00Z",
    status: "pass",
    score: 1,
    output_hash: null,
    output_changed: false,
    cached: false,
    prompt_tokens: null,
    completion_tokens: null,
    cost_usd: null,
    latency_ms: null,
    git_commit: null,
    config_digest: null,
    ...over,
  };
}

describe("historySeries", () => {
  it("keeps a null slot rather than dropping the point", () => {
    // Dropping would desynchronise the sparkline from the squares above it.
    const series = historySeries(
      [point({ score: 0.9 }), point({ score: null }), point({ score: 0.5 })],
      "score",
    );
    expect(series).toEqual([0.9, null, 0.5]);
  });

  it("sums the token halves, and gaps only when both are absent", () => {
    expect(
      historySeries(
        [
          point({ prompt_tokens: 10, completion_tokens: 5 }),
          point({ prompt_tokens: 10, completion_tokens: null }),
          point({ prompt_tokens: null, completion_tokens: null }),
        ],
        "tokens",
      ),
    ).toEqual([15, 10, null]);
  });

  it("reads latency and cost straight through", () => {
    expect(historySeries([point({ latency_ms: 1200 })], "latency")).toEqual([
      1200,
    ]);
    expect(historySeries([point({ cost_usd: 0.02 })], "cost")).toEqual([0.02]);
  });
});

describe("metricSpec", () => {
  it("marks latency, cost and tokens as lower-is-better", () => {
    // Without this the sparkline paints a latency regression green.
    expect(metricSpec("score").higherIsBetter).toBe(true);
    expect(metricSpec("latency").higherIsBetter).toBe(false);
    expect(metricSpec("cost").higherIsBetter).toBe(false);
    expect(metricSpec("tokens").higherIsBetter).toBe(false);
  });

  it("bounds score to 0..1 and leaves the others unbounded", () => {
    expect(metricSpec("score")).toMatchObject({ min: 0, max: 1 });
    expect(metricSpec("latency").min).toBeUndefined();
  });
});

describe("changePoints", () => {
  it("marks the run where the commit changed, not every run after it", () => {
    const changes = changePoints([
      point({ git_commit: "aaa" }),
      point({ git_commit: "aaa" }),
      point({ git_commit: "bbb" }),
      point({ git_commit: "bbb" }),
    ]);
    expect(changes.map((c) => c.commitChanged)).toEqual([
      false,
      false,
      true,
      false,
    ]);
  });

  it("tracks the config digest independently of the commit", () => {
    const changes = changePoints([
      point({ git_commit: "aaa", config_digest: "blake3:1" }),
      point({ git_commit: "bbb", config_digest: "blake3:1" }),
      point({ git_commit: "bbb", config_digest: "blake3:2" }),
    ]);
    expect(changes[1]).toEqual({ commitChanged: true, configChanged: false });
    expect(changes[2]).toEqual({ commitChanged: false, configChanged: true });
  });

  it("treats a missing value as unknown, not as a change", () => {
    const changes = changePoints([
      point({ git_commit: "aaa" }),
      point({ git_commit: null }),
      point({ git_commit: "aaa" }),
    ]);
    expect(changes.every((c) => !c.commitChanged)).toBe(true);
  });

  it("never marks the oldest point", () => {
    expect(changePoints([point({ git_commit: "aaa" })])[0]).toEqual({
      commitChanged: false,
      configChanged: false,
    });
  });
});

describe("historySummary", () => {
  it("counts runs and output changes", () => {
    const s = historySummary([
      point({ output_changed: true }),
      point({ output_changed: false }),
      point({ output_changed: true }),
    ]);
    expect(s.runs).toBe(3);
    expect(s.outputChanges).toBe(2);
  });

  it("reports first and last SCORED runs, skipping nulls", () => {
    const s = historySummary([
      point({ score: null }),
      point({ score: 0.4 }),
      point({ score: 0.8 }),
      point({ score: null }),
    ]);
    expect(s.firstScore).toBe(0.4);
    expect(s.lastScore).toBe(0.8);
  });

  it("omits the trend when there is fewer than one comparison", () => {
    const s = historySummary([point({ score: 0.5 })]);
    expect(s.firstScore).toBeUndefined();
    expect(s.lastScore).toBeUndefined();
  });
});

describe("tokenSplit", () => {
  it("decomposes a total that is otherwise indistinguishable", () => {
    expect(tokenSplit({ input_tokens: 728, output_tokens: 396 })).toEqual({
      input: 728,
      output: 396,
      total: 1124,
    });
  });

  it("returns null when there is no usage at all", () => {
    expect(tokenSplit(undefined)).toBeNull();
    expect(tokenSplit(null)).toBeNull();
  });
});

describe("collapseCachedRuns", () => {
  /** `n` points, chronological, with the given cached flags. */
  const window = (flags: (boolean | null)[]) =>
    flags.map((cached, i) => point({ run_id: `run-0${i}`, cached }));

  it("leaves a window of freshly measured runs alone", () => {
    const segments = collapseCachedRuns(window([false, false, false]));
    expect(segments).toHaveLength(3);
    expect(segments.every((s) => !s.collapsed)).toBe(true);
  });

  // The point of collapsing is that N replayed runs are one fact — "still
  // green, not re-measured since" — rather than N facts.
  it("folds consecutive replayed runs into one segment", () => {
    const segments = collapseCachedRuns(window([false, true, true, true]));
    expect(segments).toHaveLength(2);
    expect(segments[0]).toMatchObject({ collapsed: false, start: 0 });
    expect(segments[1]).toMatchObject({ collapsed: true, start: 1 });
    expect(segments[1]?.points).toHaveLength(3);
  });

  // Collapsing one point into a "×1" marker costs a click and saves nothing.
  it("leaves a lone replayed run expanded", () => {
    const segments = collapseCachedRuns(window([false, true, false]));
    expect(segments).toHaveLength(3);
    expect(segments.every((s) => !s.collapsed)).toBe(true);
  });

  // `null` is "we cannot tell", and a legacy row must not be folded away on a
  // claim nobody made. It also breaks a streak rather than joining it.
  it("never collapses runs of unknown provenance", () => {
    const segments = collapseCachedRuns(window([true, null, true]));
    expect(segments).toHaveLength(3);
    expect(segments.every((s) => !s.collapsed)).toBe(true);
  });

  it("splits a streak around a freshly measured run", () => {
    const segments = collapseCachedRuns(window([true, true, false, true, true]));
    expect(segments.map((s) => s.collapsed)).toEqual([true, false, true]);
    expect(segments.map((s) => s.start)).toEqual([0, 2, 3]);
  });

  it("collapses a window that is entirely replayed", () => {
    const segments = collapseCachedRuns(window([true, true, true, true]));
    expect(segments).toHaveLength(1);
    expect(segments[0]?.points).toHaveLength(4);
  });

  it("preserves every point exactly once, in order", () => {
    const points = window([false, true, true, null, true, true, false]);
    const flattened = collapseCachedRuns(points).flatMap((s) => s.points);
    expect(flattened).toEqual(points);
  });

  it("handles an empty window", () => {
    expect(collapseCachedRuns([])).toEqual([]);
  });
});
