import type { CompareCaseRow, CompareResponse, CompareSummary } from "@/api";
import { classifyDelta } from "@/lib/compare";
import { round2, round4 } from "./rng";
import { RUN_META_BY_ID } from "./runMeta";
import { caseScore, generateCases, outputRevision, type MockCaseRow } from "./cases";
import { runStats, type RunStats } from "./runStats";
import { configDigest } from "./config";

/** `GET /runs/{base}/compare/{head}` response: base/head are the run ids
 *  themselves (see generated `CompareResponse` — the server never embeds the
 *  run rows), not the run summary objects. */
export function compareRuns(baseId: string, headId: string): CompareResponse | undefined {
  const base = RUN_META_BY_ID.get(baseId);
  const head = RUN_META_BY_ID.get(headId);
  if (!base || !head) return undefined;

  const baseCases = new Map(generateCases(baseId).map((c) => [c.case_key, c]));
  const headCases = new Map(generateCases(headId).map((c) => [c.case_key, c]));
  const keys = new Set([...baseCases.keys(), ...headCases.keys()]);

  const rows: CompareCaseRow[] = [];
  const summary: CompareSummary = {
    newly_failing: 0,
    newly_passing: 0,
    still_failing: 0,
    output_changed: 0,
    added: 0,
    removed: 0,
  };

  for (const key of keys) {
    const b = baseCases.get(key);
    const h = headCases.get(key);
    const caseIdx = (b ?? h)!.idx;
    const delta = classifyDelta(b?.status ?? null, h?.status ?? null);
    let output_changed = false;
    if (b && h) {
      output_changed =
        outputRevision(base.suiteKey, caseIdx, base.runIndex) !==
        outputRevision(head.suiteKey, caseIdx, head.runIndex);
    }
    const baseScore = b ? caseScore(b) : null;
    const headScore = h ? caseScore(h) : null;
    rows.push({
      case_key: key,
      name: (h ?? b)?.name ?? null,
      base_status: b?.status ?? null,
      head_status: h?.status ?? null,
      delta,
      output_changed,
      base_score: baseScore,
      head_score: headScore,
      score_delta:
        baseScore !== null && headScore !== null
          ? round2(headScore - baseScore)
          : null,
      assert_flips: b && h ? assertFlips(b, h) : [],
    });
    if (delta === "newly_failing") summary.newly_failing++;
    else if (delta === "newly_passing") summary.newly_passing++;
    else if (delta === "still_failing") summary.still_failing++;
    else if (delta === "added") summary.added++;
    else if (delta === "removed") summary.removed++;
    if (output_changed) summary.output_changed++;
  }

  rows.sort((a, b) => a.case_key.localeCompare(b.case_key));

  const baseStats = runStats(baseId);
  const headStats = runStats(headId);
  const baseDigest = configDigest(baseId);
  const headDigest = configDigest(headId);
  const basePass = wilson(baseStats.pass_count, baseStats.case_count);
  const headPass = wilson(headStats.pass_count, headStats.case_count);
  // McNemar over the discordant pairs (regressions vs fixes), with the
  // continuity-corrected statistic and the usual chi-square(1) cutoff.
  const regressions = summary.newly_failing;
  const fixes = summary.newly_passing;
  const discordant = regressions + fixes;
  const statistic =
    discordant === 0
      ? 0
      : round2((Math.abs(regressions - fixes) - 1) ** 2 / discordant);

  return {
    base: baseId,
    head: headId,
    summary,
    cases: rows,
    stats: {
      mcnemar: { regressions, fixes, statistic, significant: statistic > 3.84 },
      base_pass_rate: basePass,
      head_pass_rate: headPass,
    },
    totals: {
      base: runTotals(baseStats),
      head: runTotals(headStats),
    },
    config: {
      base_digest: baseDigest,
      head_digest: headDigest,
      // Drift is exactly digest inequality: on within the drift suite's
      // 11→12 pair, on across suites, off for every same-config pair.
      changed: baseDigest !== headDigest,
    },
  };
}

/** Wilson score interval for a pass rate — the shape `WilsonView` carries. */
function wilson(passed: number, total: number) {
  if (total === 0) return { passed, total, rate: 0, lower: 0, upper: 0 };
  const z = 1.96;
  const z2 = z * z;
  const p = passed / total;
  const denom = 1 + z2 / total;
  const center = (p + z2 / (2 * total)) / denom;
  const margin = (z * Math.sqrt(p * (1 - p) / total + z2 / (4 * total * total))) / denom;
  return {
    passed,
    total,
    rate: round4(p),
    lower: round4(Math.max(0, center - margin)),
    upper: round4(Math.min(1, center + margin)),
  };
}

/** Per-run aggregate totals (`RunTotals`) pulled from the internal run stats. */
function runTotals(s: RunStats) {
  return {
    prompt_tokens: s.prompt_tokens,
    completion_tokens: s.completion_tokens,
    cost_usd: s.cost_usd,
    duration_ms: s.duration_ms,
    case_count: s.case_count,
    pass_count: s.pass_count,
  };
}

/** Assertions whose pass/fail flipped between base and head for one case,
 *  paired by kind (the demo's suite kinds are unique per suite). */
function assertFlips(b: MockCaseRow, h: MockCaseRow) {
  const headByKind = new Map(h.asserts.map((a) => [a.kind, a]));
  return b.asserts.flatMap((ba) => {
    const ha = headByKind.get(ba.kind);
    if (!ha || ba.passed === ha.passed) return [];
    return [
      {
        kind: ba.kind,
        base_passed: ba.passed,
        head_passed: ha.passed,
        base_score: ba.score,
        head_score: ha.score,
      },
    ];
  });
}
