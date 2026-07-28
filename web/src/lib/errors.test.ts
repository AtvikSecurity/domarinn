import { describe, expect, it } from "vitest";
import {
  aggregateErrorClasses,
  errorClassLabel,
  errorClassTone,
} from "./errors";

const errored = (error_class: string | null) => ({ status: "error", error_class });

describe("errorClassTone", () => {
  // The mapping that turns a count into a signal: a rate limit is a retry, a
  // broken grader means the scores beside it are not evidence. Rendering both
  // the same colour is what makes people ignore the number.
  it("separates infrastructure from judgement", () => {
    for (const infra of ["provider_rate_limit", "provider_timeout", "cache_miss", "exec_failed"]) {
      expect(errorClassTone(infra)).toBe("amber");
    }
    for (const other of ["grader_failed", "grader_missing", "render_failed", "assert_failed"]) {
      expect(errorClassTone(other)).toBe("error");
    }
  });

  // The classes are an open set, so an unrecognized one must still render.
  it("gives an unknown class the louder tone", () => {
    expect(errorClassTone("invented_next_year")).toBe("error");
  });
});

describe("errorClassLabel", () => {
  it("reads as a phrase rather than an identifier", () => {
    expect(errorClassLabel("provider_rate_limit")).toBe("provider · rate limit");
    expect(errorClassLabel("unknown")).toBe("unknown");
  });
});

describe("aggregateErrorClasses", () => {
  it("counts by class, most frequent first", () => {
    const tallies = aggregateErrorClasses(
      [
        errored("provider_rate_limit"),
        errored("grader_failed"),
        errored("provider_rate_limit"),
        { status: "pass", error_class: null },
      ],
      10,
    );
    expect(tallies.map((t) => [t.class, t.count])).toEqual([
      ["provider_rate_limit", 2],
      ["grader_failed", 1],
    ]);
    expect(tallies[0]?.share).toBeCloseTo(0.2);
  });

  // Cases that errored before classes existed still have to be counted, or the
  // breakdown's total silently disagrees with the run's error count and a
  // reader is left wondering where the rest went.
  it("tallies unclassified errors under `unknown` rather than dropping them", () => {
    const tallies = aggregateErrorClasses([errored(null), errored(null)], 4);
    expect(tallies).toEqual([{ class: "unknown", count: 2, share: 0.5 }]);
  });

  it("ignores non-errored cases entirely", () => {
    expect(
      aggregateErrorClasses(
        [
          { status: "pass", error_class: null },
          { status: "fail", error_class: null },
          { status: "skip", error_class: null },
        ],
        3,
      ),
    ).toEqual([]);
  });

  it("does not divide by zero on an empty run", () => {
    expect(aggregateErrorClasses([errored("x")], 0)[0]?.share).toBe(0);
  });
});
