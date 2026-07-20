import { describe, expect, it } from "vitest";
import { classifyDelta, isFailing } from "./compare";
import type { CaseStatus } from "@/api/types";

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
    ["pass", "pass", "unchanged"],
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
