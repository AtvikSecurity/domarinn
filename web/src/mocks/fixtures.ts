// Deterministic in-memory fixture dataset for demo + tests. No randomness that
// changes between reloads: everything is derived from stable seeds so the 500
// case money page, compare deltas, and sparklines are reproducible.
//
// Every function here returns (or is projected into, right before the mock
// handler serializes it) the exact wire shape of the matching generated
// response type — imported from `@/api` so tsc enforces fixture correctness
// against the real server contract, not the other way around.

import type {
  AssertResult,
  AssertStatus,
  CacheStatsResponse,
  CaseAssertLean,
  CaseListItem,
  CaseResult,
  CaseStatus,
  CompareCaseRow,
  CompareResponse,
  CompareSummary,
  MetaResponse,
  ProjectListItem,
  RunDetailResponse,
  RunListItem,
  SuitePoint,
  SuiteSummary,
} from "@/api";
import type { AssertName } from "@/api";
import { classifyDelta } from "@/lib/compare";
import { parseTimestamp } from "@/lib/format";

// ---------------------------------------------------------------------------
// Seeded RNG helpers (mulberry32 + a small string/number hash).
// ---------------------------------------------------------------------------

function hash(...parts: (string | number)[]): number {
  let h = 2166136261 >>> 0;
  const str = parts.join("|");
  for (let i = 0; i < str.length; i++) {
    h ^= str.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

/** Deterministic float in [0, 1) from any set of seed parts. */
function rand(...parts: (string | number)[]): number {
  let a = hash(...parts);
  a |= 0;
  a = (a + 0x6d2b79f5) | 0;
  let t = Math.imul(a ^ (a >>> 15), 1 | a);
  t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}

function pick<T>(arr: readonly T[], ...seed: (string | number)[]): T {
  // `rand()` is in [0, 1), so `floor(rand() * len)` is always a valid index in
  // [0, len - 1] for any non-empty array; every call site passes a non-empty
  // constant pool. The assertion encodes that proven in-bounds invariant.
  return arr[Math.floor(rand(...seed) * arr.length)]!;
}

function clamp(n: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, n));
}

/** Epoch millis -> RFC3339, matching the server's wire format for every
 *  timestamp field (see `crates/measurellm-server/src/dto/accounts.rs::rfc3339`
 *  for the server-side equivalent). */
function toIso(ms: number): string {
  return new Date(ms).toISOString();
}

// ---------------------------------------------------------------------------
// Static shape of the world.
// ---------------------------------------------------------------------------

const NOW = Date.UTC(2026, 6, 19, 15, 0, 0); // fixed reference time
const DAY = 86_400_000;
const RESULT_SCHEMA_VERSION = 1;

interface SuiteDef {
  project: string;
  suite: string;
  /** Real `AssertName` kinds (label === kind on the wire, per CaseAssertLean's
   *  doc comment) always evaluated for every case in this suite. */
  labels: AssertName[];
  runs: number;
  featured?: boolean;
}

const SUITE_DEFS: SuiteDef[] = [
  {
    project: "checkout-agent",
    suite: "regression",
    labels: ["is-json", "contains", "llm-rubric", "latency", "cost"],
    runs: 12,
    featured: true,
  },
  {
    project: "checkout-agent",
    suite: "smoke",
    labels: ["is-json", "contains", "latency"],
    runs: 8,
  },
  {
    project: "search-rerank",
    suite: "ndcg-eval",
    labels: ["is-json", "contains", "cost", "llm-rubric"],
    runs: 10,
  },
  {
    project: "support-bot",
    suite: "tone-and-safety",
    labels: ["llm-rubric", "regex", "contains", "is-json"],
    runs: 9,
  },
  {
    project: "support-bot",
    suite: "faq-accuracy",
    labels: ["contains", "similar", "llm-rubric"],
    runs: 6,
  },
];

const VERBS = [
  "handles", "rejects", "renders", "summarizes", "classifies", "extracts",
  "refuses", "escalates", "validates", "retries", "parses", "ranks",
];
const NOUNS = [
  "empty cart", "expired coupon", "duplicate order", "unicode address",
  "partial refund", "gift card", "tax exemption", "backorder item",
  "fraud signal", "loyalty tier", "split shipment", "price override",
  "malformed json", "PII in prompt", "toxic request", "ambiguous intent",
];
const BRANCHES = ["main", "main", "main", "feat/new-grader", "fix/tokenizer", "chore/deps"];
const TAG_POOL = ["nightly", "pr", "release", "canary", "regression", "smoke"];

// ---------------------------------------------------------------------------
// Run metadata (counts are derived lazily from generated cases).
// ---------------------------------------------------------------------------

export interface RunMeta {
  id: string;
  suiteKey: string;
  suiteDef: SuiteDef;
  runIndex: number; // 0 = oldest in its suite
  runsInSuite: number;
  created_at: number; // epoch millis (internal only; the wire shape is RFC3339)
  git_branch: string;
  git_commit: string;
  ci_run_url?: string;
  tags: string[];
  caseCount: number;
}

function suiteKeyOf(def: SuiteDef): string {
  return `${def.project}/${def.suite}`;
}

function buildRunMetas(): RunMeta[] {
  const metas: RunMeta[] = [];
  for (const def of SUITE_DEFS) {
    const suiteKey = suiteKeyOf(def);
    for (let i = 0; i < def.runs; i++) {
      const isLatest = i === def.runs - 1;
      const caseCount = def.featured
        ? isLatest
          ? 500
          : 460 + Math.floor(rand(suiteKey, i, "cc") * 40)
        : 40 + Math.floor(rand(suiteKey, i, "cc") * 120);
      const interval = DAY * (0.8 + rand(suiteKey, i, "iv") * 0.6);
      const created_at = Math.round(NOW - (def.runs - 1 - i) * interval);
      const branch = pick(BRANCHES, suiteKey, i, "br");
      const tagCount = 1 + Math.floor(rand(suiteKey, i, "tc") * 2);
      const tags = Array.from(
        new Set(
          Array.from({ length: tagCount }, (_, t) =>
            pick(TAG_POOL, suiteKey, i, "tag", t),
          ),
        ),
      );
      metas.push({
        id: `${def.project}-${def.suite}-${String(i + 1).padStart(2, "0")}`,
        suiteKey,
        suiteDef: def,
        runIndex: i,
        runsInSuite: def.runs,
        created_at,
        git_branch: branch,
        git_commit: hash(suiteKey, i, "sha").toString(16).padStart(8, "0").slice(0, 7),
        ci_run_url:
          rand(suiteKey, i, "ci") > 0.3
            ? `https://ci.example.com/${def.project}/${1000 + i}`
            : undefined,
        tags,
        caseCount,
      });
    }
  }
  return metas;
}

const RUN_METAS = buildRunMetas();
const RUN_META_BY_ID = new Map(RUN_METAS.map((m) => [m.id, m]));

/** suiteKey -> run ids oldest..newest */
const SUITE_RUN_IDS = new Map<string, string[]>();
for (const m of RUN_METAS) {
  const list = SUITE_RUN_IDS.get(m.suiteKey) ?? [];
  list[m.runIndex] = m.id;
  SUITE_RUN_IDS.set(m.suiteKey, list);
}

// Mutable baselines (default: previous run in the series).
const BASELINE_BY_SUITE = new Map<string, string>();
for (const [suiteKey, ids] of SUITE_RUN_IDS) {
  // Default baseline = previous run in the series (or the sole run). Guard the
  // indexed reads instead of asserting; an empty series simply gets no baseline.
  const baseline = ids.length >= 2 ? ids[ids.length - 2] : ids[0];
  if (baseline !== undefined) BASELINE_BY_SUITE.set(suiteKey, baseline);
}

// ---------------------------------------------------------------------------
// Case generation (deterministic per run, cached). `MockCaseRow` is an
// internal-only shape (richer than the wire `CaseListItem`: it keeps
// per-case tags for server-side filtering, mirroring how the real server
// filters `/runs/{id}/cases?tag=` against stored data that the lean list
// projection itself does not return — see `CaseListItem`'s doc comment).
// ---------------------------------------------------------------------------

export interface MockCaseRow {
  case_key: string;
  idx: number;
  name: string;
  tags: string[];
  status: CaseStatus;
  output_preview: string;
  asserts: CaseAssertLean[];
  prompt_tokens: number;
  completion_tokens: number;
  cost_usd: number;
  latency_ms: number;
}

const CASE_CACHE = new Map<string, MockCaseRow[]>();

function caseName(caseIndex: number): string {
  return `${pick(VERBS, "verb", caseIndex)} ${pick(NOUNS, "noun", caseIndex)}`;
}

/** How many times this case's output text has changed by a given run index. */
function outputRevision(suiteKey: string, caseIndex: number, runIndex: number): number {
  let rev = 0;
  for (let k = 1; k <= runIndex; k++) {
    if (rand(suiteKey, "outrev", caseIndex, k) < 0.16) rev++;
  }
  return rev;
}

function statusFor(meta: RunMeta, caseIndex: number): CaseStatus {
  // Per-case intrinsic difficulty; harder cases fail more often.
  const difficulty = rand(meta.suiteKey, "diff", caseIndex);
  // Slow quality improvement across the run series.
  const drift = 0.025 * meta.runIndex;
  // A subset of cases are flaky and wobble run to run.
  const flaky = rand(meta.suiteKey, "flaky", caseIndex) > 0.8;
  const wobble = flaky ? (rand(meta.suiteKey, caseIndex, meta.runIndex, "w") - 0.5) * 0.5 : 0;
  const passProb = clamp(0.98 - difficulty * 0.9 + drift + wobble, 0.03, 0.99);

  if (rand(meta.suiteKey, "skip", caseIndex) > 0.985) return "skip";
  const roll = rand(meta.suiteKey, caseIndex, meta.runIndex, "roll");
  if (roll > passProb) {
    // Some failures are hard errors rather than assertion failures.
    return rand(meta.suiteKey, caseIndex, meta.runIndex, "err") > 0.8 ? "error" : "fail";
  }
  return "pass";
}

function leanAsserts(meta: RunMeta, caseIndex: number, status: CaseStatus): CaseAssertLean[] {
  if (status === "skip") return [];
  const kinds = meta.suiteDef.labels;
  // For a failing/error case, choose which labels are the culprits.
  const failingIdx =
    status === "pass"
      ? -1
      : Math.floor(rand(meta.suiteKey, caseIndex, meta.runIndex, "fl") * kinds.length);
  return kinds.map((kind, li) => {
    const isCulprit = status !== "pass" && (li === failingIdx || (status === "error" && li === 0));
    const passed = !isCulprit;
    const score = passed
      ? 0.8 + rand(meta.suiteKey, caseIndex, kind, "s") * 0.2
      : rand(meta.suiteKey, caseIndex, kind, "s") * 0.45;
    // label === kind: both are the assert's AssertName (see CaseAssertLean's doc).
    return { label: kind, kind, passed, score: round2(score) };
  });
}

function generateCases(runId: string): MockCaseRow[] {
  const cached = CASE_CACHE.get(runId);
  if (cached) return cached;
  const meta = RUN_META_BY_ID.get(runId);
  if (!meta) return [];
  const rows: MockCaseRow[] = [];
  for (let i = 0; i < meta.caseCount; i++) {
    const status = statusFor(meta, i);
    const asserts = leanAsserts(meta, i, status);
    const tags: string[] =
      rand(meta.suiteKey, i, "ct") > 0.6 ? [pick(TAG_POOL, meta.suiteKey, i, "ctag")] : [];
    const latency = Math.round(120 + rand(meta.suiteKey, i, "lat") * 4200);
    const pt = Math.round(180 + rand(meta.suiteKey, i, "pt") * 900);
    const ct = Math.round(40 + rand(meta.suiteKey, i, "cot") * 500);
    rows.push({
      case_key: `case-${String(i).padStart(4, "0")}`,
      idx: i,
      name: caseName(i),
      tags,
      status,
      output_preview: outputPreview(meta, i, status),
      asserts,
      prompt_tokens: pt,
      completion_tokens: ct,
      cost_usd: round4((pt * 3 + ct * 15) / 1_000_000),
      latency_ms: latency,
    });
  }
  CASE_CACHE.set(runId, rows);
  return rows;
}

function outputPreview(meta: RunMeta, caseIndex: number, status: CaseStatus): string {
  if (status === "skip") return "(skipped)";
  if (status === "error") return "provider returned 502 after 3 retries";
  const rev = outputRevision(meta.suiteKey, caseIndex, meta.runIndex);
  const verb = pick(VERBS, "verb", caseIndex);
  const noun = pick(NOUNS, "noun", caseIndex);
  return `The agent ${verb} the ${noun} and produced revision r${rev}.`;
}

function fullOutput(meta: RunMeta, caseIndex: number, status: CaseStatus): string {
  if (status === "skip") return "";
  if (status === "error") {
    return "Error: upstream provider returned HTTP 502 (Bad Gateway) after 3 retries.";
  }
  const rev = outputRevision(meta.suiteKey, caseIndex, meta.runIndex);
  const noun = pick(NOUNS, "noun", caseIndex);
  const confidence = round2(0.6 + rand(meta.suiteKey, caseIndex, "conf") * 0.4);
  return [
    `{`,
    `  "intent": "${noun.replace(/\s+/g, "_")}",`,
    `  "action": "resolve",`,
    `  "revision": ${rev},`,
    `  "confidence": ${confidence},`,
    `  "explanation": "Resolved the ${noun}; applied policy checks and returned a structured result."`,
    `}`,
  ].join("\n");
}

/** Full per-assert verdict for the case-detail endpoint's `AssertResult[]`. */
function detailAsserts(
  meta: RunMeta,
  caseIndex: number,
  status: CaseStatus,
  lean: CaseAssertLean[],
): AssertResult[] {
  return lean.map((a) => {
    const weight = round2(0.5 + rand(meta.suiteKey, caseIndex, a.kind, "w") * 1.5);
    const st: AssertStatus = a.passed ? "pass" : status === "error" ? "error" : "fail";
    const reason = a.passed
      ? `${a.kind} check satisfied (score ${a.score.toFixed(2)}).`
      : status === "error"
        ? `${a.kind} could not be evaluated: grader errored on provider output.`
        : `${a.kind} check failed: expected value not found (score ${a.score.toFixed(2)}).`;
    return {
      kind: a.kind,
      status: st,
      score: a.score,
      weight,
      reason,
      details: a.passed ? undefined : { expected: "policy-compliant", got: "divergent" },
      cached: false,
    };
  });
}

/** Project the internal row down to the lean wire shape `GET .../cases` returns
 *  (notably: no `tags` — see `CaseListItem`'s doc comment on the generated type). */
export function toCaseListItem(c: MockCaseRow): CaseListItem {
  return {
    case_key: c.case_key,
    idx: c.idx,
    name: c.name,
    status: c.status,
    output_preview: c.output_preview,
    asserts: c.asserts,
    prompt_tokens: c.prompt_tokens,
    completion_tokens: c.completion_tokens,
    cost_usd: c.cost_usd,
    latency_ms: c.latency_ms,
  };
}

// ---------------------------------------------------------------------------
// Run stats: one internal computation, projected into the two distinct wire
// shapes the real server exposes (`RunListItem` for `GET /runs`, richer
// `RunDetailResponse` for `GET /runs/{id}` — they diverge: only the list item
// carries `pass_rate`, only the detail response carries `assert_labels`/
// `uploaded_at`/etc; see the generated types).
// ---------------------------------------------------------------------------

interface RunStats {
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
  tags: string[];
  assert_labels: string[];
}

function runStats(runId: string): RunStats {
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
  for (const c of cases) {
    if (c.status === "pass") pass++;
    else if (c.status === "fail") fail++;
    else if (c.status === "error") error++;
    prompt_tokens += c.prompt_tokens;
    completion_tokens += c.completion_tokens;
    cost += c.cost_usd;
    duration += c.latency_ms;
  }
  const uploadDelayMs = 1500 + Math.floor(rand(meta.suiteKey, meta.runIndex, "upl") * 4000);
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
    uploaded_by: null,
    tags: meta.tags,
    assert_labels: [...new Set(meta.suiteDef.labels)],
  };
}

function toRunListItem(s: RunStats): RunListItem {
  const denom = s.pass_count + s.fail_count + s.error_count;
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
    tags: s.tags,
    assert_labels: s.assert_labels,
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

export function caseDetail(runId: string, caseKey: string): CaseResult | undefined {
  const meta = RUN_META_BY_ID.get(runId);
  if (!meta) return undefined;
  const row = generateCases(runId).find((c) => c.case_key === caseKey);
  if (!row) return undefined;
  const asserts = detailAsserts(meta, row.idx, row.status, row.asserts);
  const score =
    asserts.length === 0
      ? row.status === "pass"
        ? 1
        : 0
      : round2(asserts.reduce((sum, a) => sum + a.score, 0) / asserts.length);
  return {
    cell: { provider_id: "openai", test_id: row.case_key, repeat: 0 },
    case_key: row.case_key,
    name: row.name,
    tags: row.tags,
    status: row.status,
    score,
    output: fullOutput(meta, row.idx, row.status),
    asserts,
    usage: { input_tokens: row.prompt_tokens, output_tokens: row.completion_tokens },
    cost_usd: row.cost_usd,
    latency_ms: row.latency_ms,
    cached: false,
    attempts: row.status === "error" ? 3 : 1,
    error:
      row.status === "error"
        ? "upstream provider returned HTTP 502 (Bad Gateway) after 3 retries."
        : undefined,
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
    rows.push({
      case_key: key,
      name: (h ?? b)?.name ?? null,
      base_status: b?.status ?? null,
      head_status: h?.status ?? null,
      delta,
      output_changed,
    });
    if (delta === "newly_failing") summary.newly_failing++;
    else if (delta === "newly_passing") summary.newly_passing++;
    else if (delta === "still_failing") summary.still_failing++;
    else if (delta === "added") summary.added++;
    else if (delta === "removed") summary.removed++;
    if (output_changed) summary.output_changed++;
  }

  rows.sort((a, b) => a.case_key.localeCompare(b.case_key));

  return { base: baseId, head: headId, summary, cases: rows };
}

export function projectSummaries(): ProjectListItem[] {
  const byProject = new Map<string, { runs: number; suites: Set<string>; last: number }>();
  for (const meta of RUN_METAS) {
    const p = byProject.get(meta.suiteDef.project) ?? { runs: 0, suites: new Set<string>(), last: 0 };
    p.runs++;
    p.suites.add(meta.suiteDef.suite);
    p.last = Math.max(p.last, meta.created_at);
    byProject.set(meta.suiteDef.project, p);
  }
  return [...byProject.entries()]
    .map(([project, v]) => ({
      project,
      run_count: v.runs,
      suite_count: v.suites.size,
      last_run_at: toIso(v.last),
    }))
    .sort((a, b) => a.project.localeCompare(b.project));
}

export function suiteSummaries(project: string): SuiteSummary[] {
  const out: SuiteSummary[] = [];
  for (const def of SUITE_DEFS) {
    if (def.project !== project) continue;
    const suiteKey = suiteKeyOf(def);
    const ids = SUITE_RUN_IDS.get(suiteKey) ?? [];
    // SuitePoint.series is newest-first, capped at 20 runs (see the generated
    // type's doc comment).
    const newestFirst = [...ids].reverse().slice(0, 20);
    const series: SuitePoint[] = newestFirst.map((id) => {
      const s = runStats(id);
      const denom = s.pass_count + s.fail_count + s.error_count;
      return {
        run_id: id,
        created_at: s.created_at,
        total: s.case_count,
        passed: s.pass_count,
        pass_rate: denom === 0 ? 0 : round4(s.pass_count / denom),
      };
    });
    const lastId = ids[ids.length - 1];
    out.push({
      suite: def.suite,
      run_count: ids.length,
      last_run_at: lastId ? runStats(lastId).created_at : null,
      baseline_run_id: BASELINE_BY_SUITE.get(suiteKey) ?? null,
      series,
    });
  }
  return out;
}

export const META: MetaResponse = {
  name: "measurellm",
  version: "0.1.0-mock",
  auth_mode: "open",
  setup_required: false,
  supported_schema_versions: [1],
  result_schema_version: RESULT_SCHEMA_VERSION,
  cache: {
    max_entry_bytes: 10_485_760,
    max_bytes: 5_368_709_120,
    max_age_days: 30,
  },
};

export function cacheStats(): CacheStatsResponse {
  return {
    entries: 4821,
    total_bytes: 268_435_456,
    hits: 19_233,
    misses: 4821,
    oldest_entry_at: toIso(NOW - 37 * DAY),
  };
}

// ---------------------------------------------------------------------------

function round2(n: number): number {
  return Math.round(n * 100) / 100;
}
function round4(n: number): number {
  return Math.round(n * 10000) / 10000;
}
