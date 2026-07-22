import { describe, expect, it } from "vitest";
import {
  CELL_BUCKET_CLASS,
  cellBucket,
  cellBucketClass,
  columnGroups,
  distinctProviders,
  distinctPrompts,
  singleCellStatus,
} from "./matrix";
import type { MatrixCell, MatrixResponse } from "@/api";

function matrix(columns: MatrixResponse["columns"]): MatrixResponse {
  return { run_id: "r", columns, rows: [], next_cursor: null };
}

/** A cell carrying only the fields a given assertion needs. */
function cell(partial: Partial<MatrixCell>): MatrixCell {
  return {
    total: 2,
    passed: 0,
    failed: 0,
    errored: 0,
    skipped: 0,
    score_mean: null,
    pass_fraction: 0,
    distinct_outputs: 1,
    latency_ms_mean: null,
    cost_usd: null,
    case_keys: [],
    ...partial,
  };
}

describe("distinctProviders", () => {
  it("returns [] while the matrix is still loading", () => {
    expect(distinctProviders(undefined)).toEqual([]);
  });

  it("dedupes providers, preserving first-seen (column) order", () => {
    const m = matrix([
      { provider_id: "gpt-5-mini", prompt_id: "baseline" },
      { provider_id: "gpt-5-mini", prompt_id: "cot-v2" },
      { provider_id: "claude-sonnet", prompt_id: "baseline" },
      { provider_id: "llama-70b", prompt_id: "cot-v2" },
    ]);
    expect(distinctProviders(m)).toEqual([
      "gpt-5-mini",
      "claude-sonnet",
      "llama-70b",
    ]);
  });

  it("yields a single provider for a single-provider run", () => {
    const m = matrix([{ provider_id: "openai", prompt_id: null }]);
    expect(distinctProviders(m)).toEqual(["openai"]);
  });
});

describe("distinctPrompts", () => {
  it("returns [] while loading and [] when no column carries a prompt", () => {
    expect(distinctPrompts(undefined)).toEqual([]);
    expect(distinctPrompts(matrix([{ provider_id: "openai", prompt_id: null }]))).toEqual(
      [],
    );
  });

  it("dedupes non-null prompts, preserving first-seen order", () => {
    const m = matrix([
      { provider_id: "gpt-5-mini", prompt_id: "baseline" },
      { provider_id: "gpt-5-mini", prompt_id: "cot-v2" },
      { provider_id: "claude-sonnet", prompt_id: "baseline" },
    ]);
    expect(distinctPrompts(m)).toEqual(["baseline", "cot-v2"]);
  });
});

describe("cellBucket", () => {
  it("maps the boundary values 0, 0.5, and 1", () => {
    expect(cellBucket(0)).toBe("empty");
    expect(cellBucket(0.5)).toBe("half");
    expect(cellBucket(1)).toBe("full");
  });

  it("maps the open intervals (0,0.5) and (0.5,1)", () => {
    expect(cellBucket(0.0001)).toBe("low");
    expect(cellBucket(0.3333)).toBe("low");
    expect(cellBucket(0.6667)).toBe("high");
    expect(cellBucket(0.9999)).toBe("high");
  });

  it("clamps out-of-range and NaN input to the endpoints", () => {
    expect(cellBucket(-1)).toBe("empty");
    expect(cellBucket(NaN)).toBe("empty");
    expect(cellBucket(2)).toBe("full");
  });

  it("cellBucketClass returns the literal Tailwind class for the bucket", () => {
    expect(cellBucketClass(0)).toBe(CELL_BUCKET_CLASS.empty);
    expect(cellBucketClass(0.25)).toBe(CELL_BUCKET_CLASS.low);
    expect(cellBucketClass(0.5)).toBe(CELL_BUCKET_CLASS.half);
    expect(cellBucketClass(0.75)).toBe(CELL_BUCKET_CLASS.high);
    expect(cellBucketClass(1)).toBe(CELL_BUCKET_CLASS.full);
    // Every bucket maps to a distinct, purge-safe class string.
    expect(new Set(Object.values(CELL_BUCKET_CLASS)).size).toBe(5);
  });
});

describe("singleCellStatus", () => {
  it("prefers error, then fail, then skip, else pass", () => {
    expect(singleCellStatus(cell({ total: 1, errored: 1 }))).toBe("error");
    expect(singleCellStatus(cell({ total: 1, failed: 1 }))).toBe("fail");
    expect(singleCellStatus(cell({ total: 1, skipped: 1 }))).toBe("skip");
    expect(singleCellStatus(cell({ total: 1, passed: 1 }))).toBe("pass");
  });
});

describe("columnGroups", () => {
  it("returns a single provider-order group for a single-prompt run", () => {
    const groups = columnGroups([
      { provider_id: "a", prompt_id: "only" },
      { provider_id: "b", prompt_id: "only" },
    ]);
    expect(groups).toHaveLength(1);
    expect(groups[0]!.promptId).toBe("only");
    expect(groups[0]!.columns.map((c) => c.providerId)).toEqual(["a", "b"]);
    expect(groups[0]!.columns.map((c) => c.colIndex)).toEqual([0, 1]);
  });

  it("handles a promptless run (null prompt)", () => {
    const groups = columnGroups([{ provider_id: "openai", prompt_id: null }]);
    expect(groups).toHaveLength(1);
    expect(groups[0]!.promptId).toBeNull();
    expect(groups[0]!.columns).toEqual([
      { colIndex: 0, providerId: "openai", promptId: null },
    ]);
  });

  it("regroups a multi-prompt run prompt-major, mapping to original cell indices", () => {
    // Provider-major columns (a/x, a/y, b/x, b/y) regroup under prompt spans.
    const groups = columnGroups([
      { provider_id: "a", prompt_id: "x" },
      { provider_id: "a", prompt_id: "y" },
      { provider_id: "b", prompt_id: "x" },
      { provider_id: "b", prompt_id: "y" },
    ]);
    expect(groups.map((g) => g.promptId)).toEqual(["x", "y"]);
    // Prompt x spans providers a, b at their original cell indices 0 and 2.
    expect(groups[0]!.columns).toEqual([
      { colIndex: 0, providerId: "a", promptId: "x" },
      { colIndex: 2, providerId: "b", promptId: "x" },
    ]);
    // Prompt y at indices 1 and 3.
    expect(groups[1]!.columns).toEqual([
      { colIndex: 1, providerId: "a", promptId: "y" },
      { colIndex: 3, providerId: "b", promptId: "y" },
    ]);
  });

  it("omits (provider, prompt) pairs that never ran", () => {
    // b never ran prompt y — its column is simply absent, not a phantom.
    const groups = columnGroups([
      { provider_id: "a", prompt_id: "x" },
      { provider_id: "a", prompt_id: "y" },
      { provider_id: "b", prompt_id: "x" },
    ]);
    expect(groups.map((g) => g.columns.map((c) => c.providerId))).toEqual([
      ["a", "b"],
      ["a"],
    ]);
  });
});
