import { describe, expect, it } from "vitest";
import {
  allRunSummaries,
  buildMatrix,
  caseDetail,
  caseHistory,
  compareRuns,
  defaultCompareTarget,
  runCases,
  runListItem,
  toCaseListItem,
} from "./fixtures";
import { classifyDelta } from "@/lib/compare";
import { parseTimestamp } from "@/lib/format";

// The one matrix-shaped fixture suite (3 providers × 2 prompts × 12 tests × 2
// repeats = 144 cases). Its latest run is what the matrix view + provider/prompt
// filters exercise.
const MATRIX_RUN = "search-rerank-ndcg-eval-10";
const MONEY_RUN = "checkout-agent-regression-12";

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
    const xfail = cases.filter((c) => c.status === "xfail").length;
    const xpass = cases.filter((c) => c.status === "xpass").length;
    expect(summary.pass_count).toBe(pass);
    expect(summary.fail_count).toBe(fail);
    expect(summary.error_count).toBe(error);
    expect(summary.xfail_count).toBe(xfail);
    expect(summary.xpass_count).toBe(xpass);
    expect(pass + fail + error + skip + xfail + xpass).toBe(500);
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

  // Pins the case the baseline-diff E2E (web/e2e/baseline-diff.spec.ts) opens:
  // on the money run vs its baseline, `case-0024` must have two present,
  // differing outputs so the drawer's "Diff vs baseline" renders a real diff.
  // Mirrors OUTPUT_CHANGED_CASE in web/e2e/helpers.ts.
  it("has a deterministic output-changed case between the money run and its baseline", () => {
    const head = "checkout-agent-regression-12";
    const base = "checkout-agent-regression-11";
    const key = "case-0024";

    const headDetail = caseDetail(head, key)!;
    const baseDetail = caseDetail(base, key)!;
    expect(headDetail).toBeTruthy();
    expect(baseDetail).toBeTruthy();

    expect(headDetail.output).toBeTruthy();
    expect(baseDetail.output).toBeTruthy();
    expect(headDetail.output).not.toBe(baseDetail.output);
    // Both sides render output (not the skip/error special cases), so the diff
    // has content on both panes.
    expect(headDetail.status).not.toBe("skip");
    expect(headDetail.status).not.toBe("error");
    expect(baseDetail.status).not.toBe("skip");
    expect(baseDetail.status).not.toBe("error");
  });
});

// Pins the case-history fixture the timeline section + its E2E
// (web/e2e/case-history.spec.ts) rely on: on the money run's suite, `case-0024`
// must resolve to a deterministic newest-first window whose `output_changed`
// chain matches the server's `points[i + 1]` semantics (storage/history.rs).
// Mirrors OUTPUT_CHANGED_CASE in web/e2e/helpers.ts.
describe("case-history fixture", () => {
  const PROJECT = "checkout-agent";
  const SUITE = "regression";
  const KEY = "case-0024";

  it("returns one newest-first point per run of the suite that carries the case", () => {
    const h = caseHistory(PROJECT, SUITE, KEY)!;
    expect(h).toBeTruthy();
    expect(h.project).toBe(PROJECT);
    expect(h.suite).toBe(SUITE);
    expect(h.case_key).toBe(KEY);

    // case-0024's idx (24) is below every regression run's case count, so it
    // appears in all 12 runs of the suite.
    expect(h.points).toHaveLength(12);
    // Newest-first: strictly descending created_at, latest run first.
    expect(h.points[0]!.run_id).toBe("checkout-agent-regression-12");
    expect(h.points.at(-1)!.run_id).toBe("checkout-agent-regression-01");
    for (let i = 1; i < h.points.length; i++) {
      expect(parseTimestamp(h.points[i - 1]!.created_at)).toBeGreaterThan(
        parseTimestamp(h.points[i]!.created_at),
      );
    }
    // The suite's pinned baseline rides along on the response.
    expect(h.baseline_run_id).toBe("checkout-agent-regression-11");
  });

  it("computes output_changed against the next-older point, null at the oldest", () => {
    const h = caseHistory(PROJECT, SUITE, KEY)!;
    // Server semantics: output_changed[i] = points[i].hash !== points[i+1].hash
    // when both are present, else null; the oldest returned point is null.
    for (let i = 0; i < h.points.length; i++) {
      const cur = h.points[i]!;
      const older = h.points[i + 1];
      const expected =
        older && cur.output_hash != null && older.output_hash != null
          ? cur.output_hash !== older.output_hash
          : null;
      expect(cur.output_changed).toBe(expected);
    }
    expect(h.points.at(-1)!.output_changed).toBeNull();
    // The newest point differs from its predecessor — same fact the
    // baseline-diff pin encodes for regression-12 vs regression-11.
    expect(h.points[0]!.output_changed).toBe(true);
  });

  it("caps the window at `limit`, keeping the newest runs", () => {
    const h = caseHistory(PROJECT, SUITE, KEY, 3)!;
    expect(h.points.map((p) => p.run_id)).toEqual([
      "checkout-agent-regression-12",
      "checkout-agent-regression-11",
      "checkout-agent-regression-10",
    ]);
    // The oldest point in the (capped) window is still null — the server does
    // not look past the LIMIT window when computing output_changed.
    expect(h.points.at(-1)!.output_changed).toBeNull();
  });

  it("reversing the newest-first points yields the oldest→newest timeline", () => {
    const h = caseHistory(PROJECT, SUITE, KEY)!;
    const chronological = [...h.points].reverse();
    for (let i = 1; i < chronological.length; i++) {
      expect(parseTimestamp(chronological[i]!.created_at)).toBeGreaterThan(
        parseTimestamp(chronological[i - 1]!.created_at),
      );
    }
  });

  it("returns undefined when no run of the suite carries the case (a 404)", () => {
    expect(caseHistory(PROJECT, SUITE, "case-9999")).toBeUndefined();
    expect(caseHistory("no-such-project", SUITE, KEY)).toBeUndefined();
    expect(caseHistory(PROJECT, "no-such-suite", KEY)).toBeUndefined();
  });
});

describe("matrix-shaped fixture suite", () => {
  it("emits a real provider × prompt grid with >1 distinct provider and prompt", () => {
    const cases = runCases(MATRIX_RUN);
    // 3 providers × 2 prompts × 12 tests × 2 repeats.
    expect(cases).toHaveLength(144);

    const items = cases.map(toCaseListItem);
    const providers = new Set(items.map((c) => c.provider_id));
    const prompts = new Set(items.map((c) => c.prompt_id));
    const tests = new Set(items.map((c) => c.test_id));
    const repeats = new Set(items.map((c) => c.repeat));
    expect(providers).toEqual(new Set(["gpt-5-mini", "claude-sonnet", "llama-70b"]));
    expect(prompts).toEqual(new Set(["baseline", "cot-v2"]));
    expect(tests.size).toBe(12);
    expect(repeats).toEqual(new Set([0, 1]));

    // Every case carries the migration-3 identity fields (no nulls on the axes)
    // plus a numeric score.
    for (const c of items) {
      expect(c.provider_id).toBeTruthy();
      expect(c.prompt_id).toBeTruthy();
      expect(c.test_id).toBeTruthy();
      expect(typeof c.repeat).toBe("number");
      expect(typeof c.score).toBe("number");
    }
  });

  it("single-provider (flat) suites keep a set provider with one distinct value", () => {
    const items = runCases(MONEY_RUN).map(toCaseListItem);
    expect(new Set(items.map((c) => c.provider_id))).toEqual(new Set(["openai"]));
    // Real single-provider runs: null prompt, test_id === case_key, repeat 0.
    for (const c of items) {
      expect(c.provider_id).toBe("openai");
      expect(c.prompt_id).toBeNull();
      expect(c.test_id).toBe(c.case_key);
      expect(c.repeat).toBe(0);
    }
  });

  it("buildMatrix pivots the matrix run into complete columns and per-test rows", () => {
    const m = buildMatrix(MATRIX_RUN)!;
    expect(m).toBeTruthy();
    expect(m.run_id).toBe(MATRIX_RUN);

    // 6 columns = (provider, prompt) pairs, first-seen order (test 0 emits them
    // all before the next test).
    expect(m.columns).toEqual([
      { provider_id: "gpt-5-mini", prompt_id: "baseline" },
      { provider_id: "gpt-5-mini", prompt_id: "cot-v2" },
      { provider_id: "claude-sonnet", prompt_id: "baseline" },
      { provider_id: "claude-sonnet", prompt_id: "cot-v2" },
      { provider_id: "llama-70b", prompt_id: "baseline" },
      { provider_id: "llama-70b", prompt_id: "cot-v2" },
    ]);

    // 12 test rows, default page holds them all (no pagination).
    expect(m.rows).toHaveLength(12);
    expect(m.next_cursor).toBeNull();

    for (const row of m.rows) {
      expect(row.cells).toHaveLength(m.columns.length);
      for (const cell of row.cells) {
        expect(cell).not.toBeNull();
        const c = cell!;
        // Each cell collapses exactly the 2 repeats of that test × column.
        expect(c.total).toBe(2);
        expect(c.passed + c.failed + c.errored + c.skipped).toBe(c.total);
        expect(c.case_keys).toHaveLength(2);
        expect(c.pass_fraction).toBeCloseTo(c.passed / c.total, 6);
        // distinct_outputs is a flakiness signal in [1, total].
        expect(c.distinct_outputs).toBeGreaterThanOrEqual(1);
        expect(c.distinct_outputs).toBeLessThanOrEqual(c.total);
      }
    }
  });

  it("buildMatrix reconciles cell totals with the provider/prompt case filters", () => {
    const m = buildMatrix(MATRIX_RUN)!;
    const items = runCases(MATRIX_RUN).map(toCaseListItem);

    // The whole grid's cell totals sum to the run's case count.
    const gridTotal = m.rows
      .flatMap((r) => r.cells)
      .reduce((sum, cell) => sum + (cell?.total ?? 0), 0);
    expect(gridTotal).toBe(items.length);

    // A single provider's column totals equal the cases carrying that provider
    // (what the `?provider=` server filter returns).
    const providerColumns = m.columns
      .map((col, i) => ({ col, i }))
      .filter(({ col }) => col.provider_id === "gpt-5-mini");
    const providerCellTotal = m.rows
      .flatMap((r) => providerColumns.map(({ i }) => r.cells[i]))
      .reduce((sum, cell) => sum + (cell?.total ?? 0), 0);
    const providerCaseCount = items.filter(
      (c) => c.provider_id === "gpt-5-mini",
    ).length;
    expect(providerCellTotal).toBe(providerCaseCount);
    expect(providerCaseCount).toBe(48); // 2 prompts × 12 tests × 2 repeats
  });

  // A fallback that is not one of the suite's configured providers is the shape
  // the whole feature exists for: it answers for someone else, forms no column,
  // and spends its own tokens. A fixture without one leaves every fallback
  // surface in the UI unreachable.
  it("attributes run cost to the provider that ANSWERED, not the configured one", () => {
    const m = buildMatrix(MATRIX_RUN)!;
    const items = runCases(MATRIX_RUN).map(toCaseListItem);
    const fellBack = items.filter((c) => c.answered_by_provider_id != null);
    expect(fellBack.length).toBeGreaterThan(0);
    for (const c of fellBack) {
      // The configured provider is untouched — the matrix column and every
      // `case_key` join depend on it.
      expect(c.answered_by_provider_id).toBe("reserve-mini");
      expect(c.provider_id).not.toBe("reserve-mini");
    }

    // `cases` across every entry covers the whole run, on every page.
    expect(m.provider_costs.reduce((s, p) => s + p.cases, 0)).toBe(items.length);
    // The answerer bills itself, and forms no column of its own.
    const reserve = m.provider_costs.find((p) => p.provider_id === "reserve-mini");
    expect(reserve?.cases).toBe(fellBack.length);
    expect(m.columns.some((c) => c.provider_id === "reserve-mini")).toBe(false);

    // Cell-level: the fallback repeats are counted where they were configured.
    const cellFallbacks = m.rows
      .flatMap((r) => r.cells)
      .reduce((sum, cell) => sum + (cell?.fallback_answered ?? 0), 0);
    expect(cellFallbacks).toBe(fellBack.length);
  });

  it("buildMatrix collapses a flat run to a single column and 404s an unknown run", () => {
    const m = buildMatrix(MONEY_RUN, { limit: 5 })!;
    expect(m.columns).toEqual([{ provider_id: "openai", prompt_id: null }]);
    // Row pagination is honoured: 5 test rows + a cursor to continue.
    expect(m.rows).toHaveLength(5);
    expect(m.next_cursor).not.toBeNull();
    for (const row of m.rows) {
      expect(row.cells).toHaveLength(1);
      expect(row.cells[0]!.total).toBe(1); // one case per test on a flat run
    }

    expect(buildMatrix("no-such-run")).toBeUndefined();
  });

  // These two cells are hard-coded by the matrix E2E (web/e2e/matrix.spec.ts):
  // pin them here so any fixture-RNG drift fails fast at the unit level instead
  // of as an opaque Playwright failure. Columns are provider-major:
  // index 2 = claude-sonnet · baseline, index 0 = gpt-5-mini · baseline.
  it("pins the E2E's 1/2 cell and its two-provider compare group", () => {
    const m = buildMatrix(MATRIX_RUN)!;
    const test000 = m.rows.find((r) => r.test_id === "test-000")!;
    // The `1/2` tile the E2E clicks (data-cell="test-000:2").
    const oneOfTwo = test000.cells[2]!;
    expect({ passed: oneOfTwo.passed, total: oneOfTwo.total }).toEqual({
      passed: 1,
      total: 2,
    });

    // test-002 · baseline: exactly two of the three providers produced output on
    // their first repeat (the third's first repeat skipped), so the compare modal
    // offers the two-way word diff. Baseline column indices are 0/2/4.
    const test002 = m.rows.find((r) => r.test_id === "test-002")!;
    const withOutput = [0, 2, 4].filter((ci) => {
      const first = test002.cells[ci]!.case_keys[0]!;
      const d = caseDetail(MATRIX_RUN, first)!;
      const out = typeof d.output === "string" ? d.output : d.output == null ? "" : "x";
      return out.trim() !== "";
    });
    expect(withOutput).toHaveLength(2);
  });
});

// Pins the schema-v2 case-detail fixture the prompt-drawer E2E
// (web/e2e/prompt-drawer.spec.ts) opens: on the support-bot/tone-and-safety
// suite, specific cases must carry a messages/text prompt, a truncating/clean
// stop_reason, and raw metadata — while the money run stays fully v1 so the
// "v1 renders nothing" assertion has a clean target. Mirrors V2_RUN/V2_*_CASE in
// web/e2e/helpers.ts.
describe("schema-v2 case-detail fixture", () => {
  const V2_RUN = "support-bot-tone-and-safety-09";

  it("emits a messages-style prompt with system+user turns, a clean stop, and raw metadata", () => {
    const d = caseDetail(V2_RUN, "case-0000")!;
    expect(d.status).toBe("pass");
    expect(d.prompt && "messages" in d.prompt).toBe(true);
    const msgs = (d.prompt as { messages: { role: string }[] }).messages;
    expect(msgs.map((m) => m.role)).toEqual(["system", "user"]);
    expect(d.stop_reason).toBe("end_turn");
    expect(typeof d.raw).toBe("object");
    expect(d.raw).not.toBeNull();
  });

  it("emits a truncating stop_reason (max_tokens) on a deterministic case", () => {
    expect(caseDetail(V2_RUN, "case-0001")!.stop_reason).toBe("max_tokens");
  });

  it("emits a text-style prompt on a deterministic case", () => {
    const d = caseDetail(V2_RUN, "case-0003")!;
    expect(d.prompt && "text" in d.prompt).toBe(true);
  });

  it("leaves skipped cases fully v1 (nothing was sent or returned)", () => {
    const skip = runCases(V2_RUN).find((c) => c.status === "skip");
    expect(skip).toBeTruthy();
    const d = caseDetail(V2_RUN, skip!.case_key)!;
    expect(d.prompt).toBeUndefined();
    expect(d.stop_reason).toBeUndefined();
    expect(d.raw).toBeUndefined();
  });

  it("leaves every other suite (the money run) v1-shaped", () => {
    const d = caseDetail(MONEY_RUN, "case-0000")!;
    expect(d.prompt).toBeUndefined();
    expect(d.stop_reason).toBeUndefined();
    expect(d.raw).toBeUndefined();
  });
});
