// Deterministic in-memory fixture dataset for demo + tests. No randomness that
// changes between reloads: everything is derived from stable seeds so the 500
// case money page, compare deltas, and sparklines are reproducible.

import type {
  CacheStats,
  CaseAssertDetail,
  CaseAssertLean,
  CaseDetail,
  CaseRow,
  CaseStatus,
  CompareResult,
  CompareRow,
  CompareSummary,
  Meta,
  ProjectSummary,
  RunSummaryRow,
  SuiteSummary,
} from "@/api/types";
import { classifyDelta } from "@/lib/compare";

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
  return arr[Math.floor(rand(...seed) * arr.length)];
}

function clamp(n: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, n));
}

// ---------------------------------------------------------------------------
// Static shape of the world.
// ---------------------------------------------------------------------------

const NOW = Date.UTC(2026, 6, 19, 15, 0, 0); // fixed reference time
const DAY = 86_400_000;

const ASSERT_KINDS: Record<string, string> = {
  schema_valid: "json-schema",
  answer_match: "contains",
  no_pii: "llm-judge",
  tone: "llm-judge",
  latency_budget: "latency",
  cost_budget: "cost",
  json_parse: "json-parse",
  refusal_correct: "llm-judge",
  citation_present: "contains",
};

interface SuiteDef {
  project: string;
  suite: string;
  labels: string[];
  runs: number;
  featured?: boolean;
}

const SUITE_DEFS: SuiteDef[] = [
  {
    project: "checkout-agent",
    suite: "regression",
    labels: ["schema_valid", "answer_match", "no_pii", "tone", "latency_budget"],
    runs: 12,
    featured: true,
  },
  {
    project: "checkout-agent",
    suite: "smoke",
    labels: ["schema_valid", "answer_match", "latency_budget"],
    runs: 8,
  },
  {
    project: "search-rerank",
    suite: "ndcg-eval",
    labels: ["json_parse", "answer_match", "cost_budget", "no_pii"],
    runs: 10,
  },
  {
    project: "support-bot",
    suite: "tone-and-safety",
    labels: ["no_pii", "tone", "refusal_correct", "schema_valid"],
    runs: 9,
  },
  {
    project: "support-bot",
    suite: "faq-accuracy",
    labels: ["answer_match", "citation_present", "tone"],
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
  created_at: number;
  git_branch?: string;
  git_commit?: string;
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
  if (ids.length >= 2) BASELINE_BY_SUITE.set(suiteKey, ids[ids.length - 2]);
  else BASELINE_BY_SUITE.set(suiteKey, ids[0]);
}

// ---------------------------------------------------------------------------
// Case generation (deterministic per run, cached).
// ---------------------------------------------------------------------------

const CASE_CACHE = new Map<string, CaseRow[]>();

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
  const labels = meta.suiteDef.labels;
  // For a failing/error case, choose which labels are the culprits.
  const failingIdx =
    status === "pass"
      ? -1
      : Math.floor(rand(meta.suiteKey, caseIndex, meta.runIndex, "fl") * labels.length);
  return labels.map((label, li) => {
    const isCulprit = status !== "pass" && (li === failingIdx || (status === "error" && li === 0));
    const passed = !isCulprit;
    const score = passed
      ? 0.8 + rand(meta.suiteKey, caseIndex, label, "s") * 0.2
      : rand(meta.suiteKey, caseIndex, label, "s") * 0.45;
    return { label, kind: ASSERT_KINDS[label] ?? "custom", passed, score: round2(score) };
  });
}

function generateCases(runId: string): CaseRow[] {
  const cached = CASE_CACHE.get(runId);
  if (cached) return cached;
  const meta = RUN_META_BY_ID.get(runId);
  if (!meta) return [];
  const rows: CaseRow[] = [];
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

function renderedPrompt(meta: RunMeta, caseIndex: number): string {
  const noun = pick(NOUNS, "noun", caseIndex);
  return [
    `System: You are a careful assistant for ${meta.suiteDef.project}.`,
    `Follow the policy and return strict JSON.`,
    ``,
    `User: Please handle the following situation: "${noun}".`,
    `Respond with {intent, action, revision, confidence, explanation}.`,
  ].join("\n");
}

function detailAsserts(
  meta: RunMeta,
  caseIndex: number,
  status: CaseStatus,
  lean: CaseAssertLean[],
): CaseAssertDetail[] {
  return lean.map((a) => {
    const weight = round2(0.5 + rand(meta.suiteKey, caseIndex, a.label, "w") * 1.5);
    const st: CaseStatus = a.passed ? "pass" : status === "error" ? "error" : "fail";
    const reason = a.passed
      ? `${a.kind} check satisfied (score ${a.score.toFixed(2)}).`
      : status === "error"
        ? `${a.kind} could not be evaluated: grader errored on provider output.`
        : `${a.kind} check failed: expected value not found (score ${a.score.toFixed(2)}).`;
    return {
      label: a.label,
      kind: a.kind,
      status: st,
      score: a.score,
      weight,
      reason,
      details: a.passed ? undefined : { expected: "policy-compliant", got: "divergent" },
    };
  });
}

// ---------------------------------------------------------------------------
// Public dataset API used by the mock handlers.
// ---------------------------------------------------------------------------

export function summarizeRun(runId: string): RunSummaryRow {
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
    prompt_tokens += c.prompt_tokens ?? 0;
    completion_tokens += c.completion_tokens ?? 0;
    cost += c.cost_usd ?? 0;
    duration += c.latency_ms;
  }
  return {
    id: meta.id,
    project: meta.suiteDef.project,
    suite: meta.suiteDef.suite,
    created_at: meta.created_at,
    git_branch: meta.git_branch,
    git_commit: meta.git_commit,
    ci_run_url: meta.ci_run_url,
    case_count: cases.length,
    pass_count: pass,
    fail_count: fail,
    error_count: error,
    prompt_tokens,
    completion_tokens,
    cost_usd: round4(cost),
    duration_ms: duration,
    tags: meta.tags,
  };
}

export function allRunSummaries(): RunSummaryRow[] {
  return RUN_METAS.map((m) => summarizeRun(m.id)).sort(
    (a, b) => b.created_at - a.created_at,
  );
}

export function runAssertLabels(runId: string): string[] {
  return RUN_META_BY_ID.get(runId)?.suiteDef.labels ?? [];
}

export function runCases(runId: string): CaseRow[] {
  return generateCases(runId);
}

export function caseDetail(runId: string, caseKey: string): CaseDetail | undefined {
  const meta = RUN_META_BY_ID.get(runId);
  if (!meta) return undefined;
  const row = generateCases(runId).find((c) => c.case_key === caseKey);
  if (!row) return undefined;
  const caseIndex = Number(caseKey.replace("case-", ""));
  const lean = leanAsserts(meta, caseIndex, row.status);
  return {
    case_key: row.case_key,
    name: row.name,
    tags: row.tags,
    status: row.status,
    output_preview: row.output_preview,
    prompt_tokens: row.prompt_tokens,
    completion_tokens: row.completion_tokens,
    cost_usd: row.cost_usd,
    latency_ms: row.latency_ms,
    rendered_prompt: renderedPrompt(meta, caseIndex),
    output: fullOutput(meta, caseIndex, row.status),
    asserts: detailAsserts(meta, caseIndex, row.status, lean),
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

export function compareRuns(baseId: string, headId: string): CompareResult | undefined {
  const base = RUN_META_BY_ID.get(baseId);
  const head = RUN_META_BY_ID.get(headId);
  if (!base || !head) return undefined;

  const baseCases = new Map(generateCases(baseId).map((c) => [c.case_key, c]));
  const headCases = new Map(generateCases(headId).map((c) => [c.case_key, c]));
  const keys = new Set([...baseCases.keys(), ...headCases.keys()]);

  const rows: CompareRow[] = [];
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
    const caseIndex = Number(key.replace("case-", ""));
    const delta = classifyDelta(b?.status ?? null, h?.status ?? null);
    let output_changed = false;
    if (b && h) {
      output_changed =
        outputRevision(base.suiteKey, caseIndex, base.runIndex) !==
        outputRevision(head.suiteKey, caseIndex, head.runIndex);
    }
    rows.push({
      case_key: key,
      name: (h ?? b)?.name,
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

  return { base: summarizeRun(baseId), head: summarizeRun(headId), summary, cases: rows };
}

export function projectSummaries(): ProjectSummary[] {
  const byProject = new Map<string, { runs: number; suites: Set<string>; last: number }>();
  for (const s of allRunSummaries()) {
    const p = byProject.get(s.project) ?? { runs: 0, suites: new Set(), last: 0 };
    p.runs++;
    p.suites.add(s.suite);
    p.last = Math.max(p.last, s.created_at);
    byProject.set(s.project, p);
  }
  return [...byProject.entries()]
    .map(([project, v]) => ({
      project,
      run_count: v.runs,
      suite_count: v.suites.size,
      last_run_at: v.last,
    }))
    .sort((a, b) => a.project.localeCompare(b.project));
}

export function suiteSummaries(project: string): SuiteSummary[] {
  const out: SuiteSummary[] = [];
  for (const def of SUITE_DEFS) {
    if (def.project !== project) continue;
    const suiteKey = suiteKeyOf(def);
    const ids = SUITE_RUN_IDS.get(suiteKey) ?? [];
    const series = ids.map((id) => {
      const s = summarizeRun(id);
      const denom = s.pass_count + s.fail_count + s.error_count;
      return denom === 0 ? 0 : round4(s.pass_count / denom);
    });
    const lastId = ids[ids.length - 1];
    out.push({
      suite: def.suite,
      run_count: ids.length,
      baseline_run_id: BASELINE_BY_SUITE.get(suiteKey),
      pass_rate_series: series,
      last_run_id: lastId,
      last_run_at: lastId ? summarizeRun(lastId).created_at : undefined,
    });
  }
  return out;
}

export const META: Meta = {
  name: "measurellm",
  version: "0.1.0-mock",
  auth_mode: "open",
  supported_schema_versions: [1],
};

export function cacheStats(): CacheStats {
  return {
    entries: 4821,
    total_bytes: 268_435_456,
    hits: 19_233,
    misses: 4821,
    oldest_entry_at: NOW - 37 * DAY,
  };
}

// ---------------------------------------------------------------------------

function round2(n: number): number {
  return Math.round(n * 100) / 100;
}
function round4(n: number): number {
  return Math.round(n * 10000) / 10000;
}
