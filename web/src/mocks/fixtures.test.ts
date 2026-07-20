import { describe, expect, it } from "vitest";
import {
  allRunSummaries,
  caseDetail,
  compareRuns,
  defaultCompareTarget,
  runCases,
  runListItem,
} from "./fixtures";
import { classifyDelta } from "@/lib/compare";
import { parseTimestamp } from "@/lib/format";

describe("fixture dataset", () => {
  const runs = allRunSummaries();

  it("exposes runs sorted newest-first, with RFC3339 created_at", () => {
    expect(runs.length).toBeGreaterThan(20);
    for (const r of runs) {
      expect(Number.isNaN(parseTimestamp(r.created_at))).toBe(false);
    }
    for (let i = 1; i < runs.length; i++) {
      const prev = runs[i - 1];
      const cur = runs[i];
      if (!prev || !cur) continue;
      expect(parseTimestamp(prev.created_at)).toBeGreaterThanOrEqual(
        parseTimestamp(cur.created_at),
      );
    }
  });

  it("has a featured run with exactly 500 cases and consistent counts", () => {
    const featured = runs.find((r) => r.case_count === 500);
    expect(featured, "expected a 500-case run for the money page").toBeTruthy();
    const id = featured!.id;

    const cases = runCases(id);
    expect(cases).toHaveLength(500);

    // Header counts must reconcile with the generated grid.
    const summary = runListItem(id);
    const pass = cases.filter((c) => c.status === "pass").length;
    const fail = cases.filter((c) => c.status === "fail").length;
    const error = cases.filter((c) => c.status === "error").length;
    const skip = cases.filter((c) => c.status === "skip").length;
    expect(summary.pass_count).toBe(pass);
    expect(summary.fail_count).toBe(fail);
    expect(summary.error_count).toBe(error);
    expect(pass + fail + error + skip).toBe(500);
  });

  it("returns full case detail (a real CaseResult) with output and per-assert reasons", () => {
    const featured = runs.find((r) => r.case_count === 500)!;
    const firstCase = runCases(featured.id)[0];
    if (!firstCase) throw new Error("fixture run must have at least one case");
    const key = firstCase.case_key;
    const detail = caseDetail(featured.id, key);
    expect(detail).toBeTruthy();
    expect(typeof detail!.output).toBe("string");
    for (const a of detail!.asserts) {
      expect(a.reason.length).toBeGreaterThan(0);
      expect(typeof a.weight).toBe("number");
      // `AssertResult.kind` must be a real AssertName, not a fabricated
      // human label — this is what the wire (and the generated
      // `CaseAssertLean`/`AssertResult` types) actually carries.
      expect(typeof a.kind).toBe("string");
    }
  });

  it("produces a compare whose base/head are the plain run ids, matching per-row classification", () => {
    const head = runs.find((r) => r.case_count === 500)!;
    const base = defaultCompareTarget(head.id);
    expect(base).toBeTruthy();

    const result = compareRuns(base!, head.id)!;
    expect(result).toBeTruthy();
    expect(result.base).toBe(base);
    expect(result.head).toBe(head.id);

    let newlyFailing = 0;
    let newlyPassing = 0;
    let stillFailing = 0;
    let outputChanged = 0;
    let added = 0;
    let removed = 0;
    for (const row of result.cases) {
      // Each row's delta agrees with the shared classifier.
      expect(row.delta).toBe(classifyDelta(row.base_status, row.head_status));
      if (row.delta === "newly_failing") newlyFailing++;
      else if (row.delta === "newly_passing") newlyPassing++;
      else if (row.delta === "still_failing") stillFailing++;
      else if (row.delta === "added") added++;
      else if (row.delta === "removed") removed++;
      if (row.output_changed) outputChanged++;
    }

    expect(result.summary.newly_failing).toBe(newlyFailing);
    expect(result.summary.newly_passing).toBe(newlyPassing);
    expect(result.summary.still_failing).toBe(stillFailing);
    expect(result.summary.output_changed).toBe(outputChanged);
    expect(result.summary.added).toBe(added);
    expect(result.summary.removed).toBe(removed);
  });
});
