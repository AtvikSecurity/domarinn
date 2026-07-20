import { describe, expect, it } from "vitest";
import { classifyDelta, isFailing, previousRun } from "./compare";
import type { CaseStatus, RunListItem } from "@/api";

function run(id: string, created_at: string): RunListItem {
  return {
    id,
    project: "p",
    suite: "s",
    created_at,
    git_branch: null,
    git_commit: null,
    git_dirty: null,
    case_count: 0,
    pass_count: 0,
    fail_count: 0,
    error_count: 0,
    pass_rate: 1,
    prompt_tokens: 0,
    completion_tokens: 0,
    cost_usd: null,
    duration_ms: 0,
    tags: [],
  };
}

describe("isFailing", () => {
  it("treats fail and error as failing, pass/skip/null as not", () => {
    expect(isFailing("fail")).toBe(true);
    expect(isFailing("error")).toBe(true);
    expect(isFailing("pass")).toBe(false);
    expect(isFailing("skip")).toBe(false);
    expect(isFailing(null)).toBe(false);
  });
});

describe("classifyDelta", () => {
  const cases: [CaseStatus | null, CaseStatus | null, string][] = [
    ["pass", "fail", "newly_failing"],
    ["pass", "error", "newly_failing"],
    ["fail", "pass", "newly_passing"],
    ["error", "pass", "newly_passing"],
    ["fail", "fail", "still_failing"],
    ["fail", "error", "still_failing"],
    ["error", "error", "still_failing"],
    ["pass", "pass", "still_passing"],
    ["skip", "skip", "unchanged"],
    [null, "pass", "added"],
    [null, "fail", "added"],
    ["pass", null, "removed"],
    ["fail", null, "removed"],
    [null, null, "unchanged"],
  ];

  it.each(cases)("%s -> %s classifies as %s", (base, head, expected) => {
    expect(classifyDelta(base, head)).toBe(expected);
  });

  it("removed takes precedence when head is missing even if base was failing", () => {
    expect(classifyDelta("error", null)).toBe("removed");
  });
});

describe("previousRun", () => {
  const runs = [
    run("a", "2024-01-01T00:00:00Z"),
    run("b", "2024-01-02T00:00:00Z"),
    run("c", "2024-01-03T00:00:00Z"),
  ];

  it("returns the immediately older run by created_at, regardless of input order", () => {
    expect(previousRun(runs, "c")?.id).toBe("b");
    expect(previousRun(runs, "b")?.id).toBe("a");
    // Order-independent: shuffling the input list doesn't change the answer.
    expect(previousRun([...runs].reverse(), "c")?.id).toBe("b");
  });

  it("returns undefined for the oldest run in the list", () => {
    expect(previousRun(runs, "a")).toBeUndefined();
  });

  it("returns undefined when the run id isn't present", () => {
    expect(previousRun(runs, "nope")).toBeUndefined();
  });
});
