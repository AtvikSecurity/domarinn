import type { RunDetailResponse, RunListItem } from "@/api";
import { parseTimestamp } from "@/lib/format";
import { hash, pick, rand, round4, toIso } from "./rng";
import { RESULT_SCHEMA_VERSION } from "./suites";
import { RUN_METAS, RUN_META_BY_ID } from "./runMeta";
import { generateCases, type MockCaseRow } from "./cases";
import { configDigest } from "./config";

/** Deterministic people the mock attributes runs to. */
const ACTORS = ["alice", "bob", "dana", "erik"];

/** Notes a developer might leave on an iteration run. */
const NOTES = [
  "trying temperature 0.3",
  "retry backoff, 3rd attempt",
  "new rubric wording",
  "checking the tokenizer fix",
];

// ---------------------------------------------------------------------------
// Run stats: one internal computation, projected into the two distinct wire
// shapes the real server exposes (`RunListItem` for `GET /runs`, richer
// `RunDetailResponse` for `GET /runs/{id}` — they diverge: only the list item
// carries `pass_rate`, only the detail response carries `assert_labels`/
// `uploaded_at`/etc; see the generated types).
// ---------------------------------------------------------------------------

export interface RunStats {
  id: string;
  project: string;
  suite: string;
  created_at: string;
  uploaded_at: string;
  schema_version: number;
  git_branch: string;
  git_commit: string;
  git_dirty: boolean;
  ci_provider: string | null;
  ci_run_url: string | null;
  case_count: number;
  pass_count: number;
  fail_count: number;
  error_count: number;
  prompt_tokens: number;
  completion_tokens: number;
  cost_usd: number;
  duration_ms: number;
  content_hash: string;
  uploaded_by: string | null;
  actor: string | null;
  host: string | null;
  note: string | null;
  domarinn_version: string | null;
  cache_hits: number;
  cache_misses: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  cache_savings_usd: number;
  grader_cost_usd: number;
  /** Empty-output cases tallied by reason, grouped from this run's cases —
   *  the same derivation the real server uses, so the list count, the detail
   *  map and the case grid cannot disagree. `{}` when the run had none; the
   *  projections below drop the key entirely rather than sending it. */
  empty_counts: Record<string, number>;
  tags: string[];
  assert_labels: string[];
}

export function runStats(runId: string): RunStats {
  const meta = RUN_META_BY_ID.get(runId);
  if (!meta) throw new Error(`unknown run ${runId}`);
  const cases = generateCases(runId);
  let pass = 0;
  let fail = 0;
  let error = 0;
  let prompt_tokens = 0;
  let completion_tokens = 0;
  let cost = 0;
  let duration = 0;
  let cache_hits = 0;
  let cache_misses = 0;
  let cache_savings = 0;
  const empty_counts: Record<string, number> = {};
  for (const c of cases) {
    if (c.empty_reason != null) {
      empty_counts[c.empty_reason] = (empty_counts[c.empty_reason] ?? 0) + 1;
    }
    if (c.status === "pass") pass++;
    else if (c.status === "fail") fail++;
    else if (c.status === "error") error++;
    prompt_tokens += c.prompt_tokens;
    completion_tokens += c.completion_tokens;
    cost += c.cost_usd;
    duration += c.latency_ms;
    if (c.cached) {
      cache_hits++;
      // Exactly what the engine computes: the sum of `cost_usd` over the cases
      // that were hits. Not a re-pricing.
      cache_savings += c.cost_usd;
    } else cache_misses++;
  }
  const uploadDelayMs = 1500 + Math.floor(rand(meta.suiteKey, meta.runIndex, "upl") * 4000);
  // Provenance mirrors the real split: a CI run is attributed to the person who
  // pushed the change (the CI actor) and uploaded by a shared token, while a
  // developer run is both run and uploaded by the same account.
  const isCi = meta.ci_run_url != null;
  const person = pick(ACTORS, meta.suiteKey, meta.runIndex, "actor");
  return {
    id: meta.id,
    project: meta.suiteDef.project,
    suite: meta.suiteDef.suite,
    created_at: toIso(meta.created_at),
    uploaded_at: toIso(meta.created_at + uploadDelayMs),
    schema_version: RESULT_SCHEMA_VERSION,
    git_branch: meta.git_branch,
    git_commit: meta.git_commit,
    git_dirty: rand(meta.suiteKey, meta.runIndex, "dirty") > 0.9,
    ci_provider: meta.ci_run_url ? "github-actions" : null,
    ci_run_url: meta.ci_run_url ?? null,
    case_count: cases.length,
    pass_count: pass,
    fail_count: fail,
    error_count: error,
    prompt_tokens,
    completion_tokens,
    cost_usd: round4(cost),
    duration_ms: duration,
    content_hash: `sha256:${hash(meta.id, "hash").toString(16).padStart(16, "0")}`,
    uploaded_by: isCi ? "ci-token" : person,
    actor: person,
    host: isCi
      ? `runner-${String(1 + Math.floor(rand(meta.suiteKey, meta.runIndex, "host") * 9)).padStart(2, "0")}`
      : `${person}-laptop`,
    note: isCi ? null : pick(NOTES, meta.suiteKey, meta.runIndex, "note"),
    domarinn_version: "0.2.0",
    cache_hits,
    cache_misses,
    // Provider-side prompt caching, roughly proportional to prompt volume so
    // the offline UI shows a plausible split rather than a round number.
    cache_read_tokens: Math.floor(prompt_tokens * 0.6),
    cache_write_tokens: Math.floor(prompt_tokens * 0.05),
    cache_savings_usd: round4(cache_savings),
    // Grading is its own line item and is usually a meaningful fraction of the
    // run — the whole reason it is reported apart from `cost_usd`.
    grader_cost_usd: round4(cost * 0.35),
    empty_counts,
    tags: meta.tags,
    assert_labels: [...new Set(meta.suiteDef.labels)],
  };
}

/** The run's empty cases as one number — what the list row carries. */
function emptyTotal(s: RunStats): number {
  return Object.values(s.empty_counts).reduce((sum, n) => sum + n, 0);
}

function toRunListItem(s: RunStats): RunListItem {
  const denom = s.pass_count + s.fail_count + s.error_count;
  const empty = emptyTotal(s);
  return {
    id: s.id,
    project: s.project,
    suite: s.suite,
    created_at: s.created_at,
    git_branch: s.git_branch,
    git_commit: s.git_commit,
    git_dirty: s.git_dirty,
    case_count: s.case_count,
    pass_count: s.pass_count,
    fail_count: s.fail_count,
    error_count: s.error_count,
    pass_rate: denom === 0 ? 0 : round4(s.pass_count / denom),
    prompt_tokens: s.prompt_tokens,
    completion_tokens: s.completion_tokens,
    cost_usd: s.cost_usd,
    duration_ms: s.duration_ms,
    cache_hits: s.cache_hits,
    cache_misses: s.cache_misses,
    // Omitted, never `0`: absence has to keep meaning "nothing to report",
    // which on the real server also covers a row written before the column
    // existed. A rendered zero would claim knowledge the row does not have.
    ...(empty > 0 ? { empty_count: empty } : {}),
    actor: s.actor,
    host: s.host,
    uploaded_by: s.uploaded_by,
    ci_provider: s.ci_provider,
    ci_run_url: s.ci_run_url,
    note: s.note,
    domarinn_version: s.domarinn_version,
    tags: s.tags,
  };
}

function toRunDetailResponse(s: RunStats): RunDetailResponse {
  return {
    id: s.id,
    project: s.project,
    suite: s.suite,
    created_at: s.created_at,
    uploaded_at: s.uploaded_at,
    schema_version: s.schema_version,
    git_branch: s.git_branch,
    git_commit: s.git_commit,
    git_dirty: s.git_dirty,
    ci_provider: s.ci_provider,
    ci_run_url: s.ci_run_url,
    case_count: s.case_count,
    pass_count: s.pass_count,
    fail_count: s.fail_count,
    error_count: s.error_count,
    prompt_tokens: s.prompt_tokens,
    completion_tokens: s.completion_tokens,
    cost_usd: s.cost_usd,
    duration_ms: s.duration_ms,
    content_hash: s.content_hash,
    uploaded_by: s.uploaded_by,
    config_digest: configDigest(s.id),
    cache_hits: s.cache_hits,
    cache_misses: s.cache_misses,
    cache_read_tokens: s.cache_read_tokens,
    cache_write_tokens: s.cache_write_tokens,
    cache_savings_usd: s.cache_savings_usd,
    grader_cost_usd: s.grader_cost_usd,
    actor: s.actor,
    host: s.host,
    note: s.note,
    domarinn_version: s.domarinn_version,
    tags: s.tags,
    assert_labels: s.assert_labels,
    // Omitted, never `{}` — same rule as `RunListItem.empty_count` above, so
    // "absent" means one thing across both shapes.
    ...(Object.keys(s.empty_counts).length > 0
      ? { empty_counts: s.empty_counts }
      : {}),
  };
}

// ---------------------------------------------------------------------------
// Public dataset API used by the mock handlers.
// ---------------------------------------------------------------------------

/** `GET /runs` row shape. */
export function runListItem(runId: string): RunListItem {
  return toRunListItem(runStats(runId));
}

/** `GET /runs/{id}` response shape. */
export function runDetail(runId: string): RunDetailResponse {
  return toRunDetailResponse(runStats(runId));
}

export function allRunSummaries(): RunListItem[] {
  return RUN_METAS.map((m) => runListItem(m.id)).sort(
    (a, b) => parseTimestamp(b.created_at) - parseTimestamp(a.created_at),
  );
}

export function runCases(runId: string): MockCaseRow[] {
  return generateCases(runId);
}
