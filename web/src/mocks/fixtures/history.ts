import type { CaseHistoryPoint, CaseHistoryResponse } from "@/api";
import { clamp, toIso } from "./rng";
import { BASELINE_BY_SUITE, RUN_META_BY_ID, SUITE_RUN_IDS, type RunMeta } from "./runMeta";
import { caseScore, generateCases, type MockCaseRow } from "./cases";
import { configDigest } from "./config";

// ---------------------------------------------------------------------------
// Case history (`GET /projects/{project}/suites/{suite}/cases/{case_key}/history`).
// Mirrors the server's storage/history.rs: a `cases JOIN runs` walk of one
// matrix cell backwards in time, newest-first, capped at `limit`, with
// `output_changed[i]` computed against the next-older point (`points[i + 1]`).
// ---------------------------------------------------------------------------

const HISTORY_DEFAULT_LIMIT = 20;
const HISTORY_MAX_LIMIT = 100;

/**
 * A case's history across the recent runs of `(project, suite)`, newest-first.
 * Emits one point per run of the suite that carries `case_key` (the join's
 * `WHERE c.case_key = ? AND r.project = ? AND r.suite = ?`), ordered
 * `created_at DESC` and capped at `limit` (`ORDER BY ... DESC LIMIT ?`).
 * `output_changed[i]` compares `points[i]` to the chronologically previous run
 * (`points[i + 1]`, the next-older point): both output hashes present → whether
 * they differ, otherwise `null`; the oldest returned point is always `null` —
 * exactly `storage/history.rs`. Returns `undefined` when no run of the suite
 * carries the case (an unknown project, suite, or case) → a 404 at the handler.
 */
export function caseHistory(
  project: string,
  suite: string,
  caseKey: string,
  limit: number = HISTORY_DEFAULT_LIMIT,
): CaseHistoryResponse | undefined {
  const suiteKey = `${project}/${suite}`;
  const ids = SUITE_RUN_IDS.get(suiteKey);
  if (!ids) return undefined;
  const cap = clamp(Math.floor(limit), 1, HISTORY_MAX_LIMIT);

  // Collect every run of the suite that carries this case (the join's WHERE).
  // The per-run `created_at` uses a randomized interval, so it is NOT monotonic
  // with the run index — order the matches exactly like the server
  // (`ORDER BY r.created_at DESC, r.id DESC`) and only then cap at `limit`
  // (`LIMIT ?`), so `output_changed` is computed over the same window the server
  // would return.
  const matches: { runId: string; meta: RunMeta; row: MockCaseRow }[] = [];
  for (const runId of ids) {
    const meta = RUN_META_BY_ID.get(runId);
    if (!meta) continue;
    const row = generateCases(runId).find((c) => c.case_key === caseKey);
    if (!row) continue;
    matches.push({ runId, meta, row });
  }
  matches.sort(
    (a, b) => b.meta.created_at - a.meta.created_at || b.runId.localeCompare(a.runId),
  );

  // Zero matches means the case_key never appeared in any run of this
  // project/suite — a 404 at the handler (matches `case_history` returning None).
  if (matches.length === 0) return undefined;

  const points: CaseHistoryPoint[] = matches.slice(0, cap).map(({ runId, meta, row }) => ({
    run_id: runId,
    created_at: toIso(meta.created_at),
    status: row.status,
    score: caseScore(row),
    output_hash: row.output_hash,
    output_changed: null, // filled in below, once neighbours are known
    cached: row.cached,
    prompt_tokens: row.prompt_tokens,
    completion_tokens: row.completion_tokens,
    cost_usd: row.cost_usd,
    latency_ms: row.latency_ms,
    git_commit: meta.git_commit,
    config_digest: configDigest(runId),
  }));

  // `output_changed`: newest-first, so the next-older run is `points[i + 1]`.
  // Both hashes present → inequality; otherwise null; the oldest returned point
  // (last, no `points[i + 1]`) is always null.
  for (let i = 0; i < points.length; i++) {
    const cur = points[i]!;
    const older = points[i + 1];
    cur.output_changed =
      older && cur.output_hash != null && older.output_hash != null
        ? cur.output_hash !== older.output_hash
        : null;
  }

  return {
    project,
    suite,
    case_key: caseKey,
    baseline_run_id: BASELINE_BY_SUITE.get(suiteKey) ?? null,
    points,
  };
}

export function suiteBaseline(suiteKey: string): string | undefined {
  return BASELINE_BY_SUITE.get(suiteKey);
}

export function setSuiteBaseline(project: string, suite: string, runId: string): void {
  BASELINE_BY_SUITE.set(`${project}/${suite}`, runId);
}

export function defaultCompareTarget(runId: string): string | undefined {
  const meta = RUN_META_BY_ID.get(runId);
  if (!meta) return undefined;
  const baseline = BASELINE_BY_SUITE.get(meta.suiteKey);
  if (baseline && baseline !== runId) return baseline;
  const ids = SUITE_RUN_IDS.get(meta.suiteKey) ?? [];
  const idx = ids.indexOf(runId);
  return idx > 0 ? ids[idx - 1] : undefined;
}
