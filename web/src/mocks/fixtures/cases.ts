import type {
  AssertName,
  AssertResult,
  AssertStatus,
  CaseAssertLean,
  CaseListItem,
  CaseStatus,
} from "@/api";
import { clamp, hash, pick, rand, round2, round4 } from "./rng";
import { NOUNS, TAG_POOL, VERBS, type MatrixSpec } from "./suites";
import { RUN_META_BY_ID, type RunMeta } from "./runMeta";

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
  /** NULL for an errored case: the preview derives from the output, and an
   *  errored case has none — same as the server. */
  output_preview: string | null;
  /** The failure reason, for `status === "error"` only (migration-7 column). */
  error: string | null;
  /** What kind of failure it was (migration-10 column). */
  error_class: string | null;
  /** Why the output had nothing gradeable in it (migration-15 column), or null
   *  when it was not empty. An empty output is a *successful* call, so this is
   *  the only field that explains a case with no preview and no error. */
  empty_reason: string | null;
  asserts: CaseAssertLean[];
  prompt_tokens: number;
  completion_tokens: number;
  cost_usd: number;
  latency_ms: number;
  // Matrix identity (migration-3 columns), populated for every case. Flat
  // (single-provider) suites carry a constant provider, a null prompt,
  // `test_id === case_key`, and `repeat === 0`; matrix suites carry the real
  // grid coordinates.
  provider_id: string;
  prompt_id: string | null;
  test_id: string;
  repeat: number;
  /** Whether the provider response was a cache hit (migration-6 column). */
  cached: boolean;
  /** The provider that actually *answered*, when a `fallback:` chain stood in
   *  for the configured one (migration-17 column); null when the configured
   *  provider answered. `provider_id` above stays the configured provider — the
   *  matrix column and every `case_key` join depend on that — so this is the
   *  only place a handoff is visible. */
  answered_by_provider_id: string | null;
  /** Stable per-case RNG seed (the numeric idx for flat suites, a composite for
   *  matrix ones) so the full case-detail output stays coherent with the row. */
  seed: string | number;
  /** Content hash of the case's output. Distinct hashes across a cell's repeats
   *  are the matrix's `distinct_outputs` flakiness signal. */
  output_hash: string;
}

const CASE_CACHE = new Map<string, MockCaseRow[]>();

function caseName(seed: string | number): string {
  return `${pick(VERBS, "verb", seed)} ${pick(NOUNS, "noun", seed)}`;
}

/** How many times this case's output text has changed by a given run index. */
export function outputRevision(suiteKey: string, seed: string | number, runIndex: number): number {
  let rev = 0;
  for (let k = 1; k <= runIndex; k++) {
    if (rand(suiteKey, "outrev", seed, k) < 0.16) rev++;
  }
  return rev;
}

function statusFor(meta: RunMeta, seed: string | number): CaseStatus {
  // A stable suite never fails (see `SuiteDef.stable`).
  if (meta.suiteDef.stable) return "pass";
  // A replayed-failing suite fails the SAME cases every run: the verdict is a
  // property of the case, never of the run index, so every replay reports an
  // identical pass rate (see `SuiteDef.replayedFailing`).
  if (meta.suiteDef.replayedFailing) {
    return rand(meta.suiteKey, "stablefail", seed) > 0.8 ? "fail" : "pass";
  }
  // Per-case intrinsic difficulty; harder cases fail more often.
  const difficulty = rand(meta.suiteKey, "diff", seed);
  // Slow quality improvement across the run series.
  const drift = 0.025 * meta.runIndex;
  // A subset of cases are flaky and wobble run to run.
  const flaky = rand(meta.suiteKey, "flaky", seed) > 0.8;
  const wobble = flaky ? (rand(meta.suiteKey, seed, meta.runIndex, "w") - 0.5) * 0.5 : 0;
  const passProb = clamp(0.98 - difficulty * 0.9 + drift + wobble, 0.03, 0.99);

  if (rand(meta.suiteKey, "skip", seed) > 0.985) return "skip";
  // A small stable subset of cases carries an `expect_fail` annotation — a
  // property of the *case*, so it never wobbles with the run. Annotated cases
  // land xfail/xpass instead of fail/pass; a hard error outranks the marker.
  const annotated = rand(meta.suiteKey, "xf", seed) > 0.94;
  const roll = rand(meta.suiteKey, seed, meta.runIndex, "roll");
  if (roll > passProb) {
    // Some failures are hard errors rather than assertion failures.
    if (rand(meta.suiteKey, seed, meta.runIndex, "err") > 0.8) return "error";
    return annotated ? "xfail" : "fail";
  }
  return annotated ? "xpass" : "pass";
}

function leanAsserts(meta: RunMeta, seed: string | number, status: CaseStatus): CaseAssertLean[] {
  if (status === "skip") return [];
  const kinds = meta.suiteDef.labels;
  // For a failing/error case, choose which labels are the culprits.
  // An xpass graded like a pass and an xfail like a fail — the annotation
  // moves the *status*, never the asserts underneath it.
  const passish = status === "pass" || status === "xpass";
  const failingIdx = passish
    ? -1
    : Math.floor(rand(meta.suiteKey, seed, meta.runIndex, "fl") * kinds.length);
  return kinds.map((kind, li) => {
    const isCulprit = !passish && (li === failingIdx || (status === "error" && li === 0));
    const passed = !isCulprit;
    const score = passed
      ? 0.8 + rand(meta.suiteKey, seed, kind, "s") * 0.2
      : rand(meta.suiteKey, seed, kind, "s") * 0.45;
    // label === kind: both are the assert's AssertName (see CaseAssertLean's doc).
    return { label: kind, kind, passed, score: round2(score) };
  });
}

// ---------------------------------------------------------------------------
// Per-run cache regime. CI re-runs of an unchanged suite are fully cached
// (roughly 30% of runs), an edited suite is partially cached (~40%), and the
// rest run fresh. Deterministic per run, like everything else here.
// ---------------------------------------------------------------------------

interface CacheRegime {
  full: boolean;
  hitRate: number;
}

function cacheRegime(meta: RunMeta): CacheRegime {
  // A stable suite pays for its first run, then every re-run hits the cache.
  // Same for a replayed-failing one — the point of that suite is that its
  // replays are indistinguishable from each other, verdict included.
  if (meta.suiteDef.stable || meta.suiteDef.replayedFailing) {
    return meta.runIndex === 0 ? { full: false, hitRate: 0 } : { full: true, hitRate: 1 };
  }
  // Every other suite is fresh or partially cached, never fully.
  //
  // Being fully cached is what makes a run disappear from the default view, so
  // leaving it to a hash meant the anchor runs the e2e suite navigates by could
  // vanish on a fixture reseed. Which suites are replays is now a property of
  // the suite, declared above, rather than an accident.
  const r = rand(meta.suiteKey, meta.runIndex, "cachereg");
  if (r < 0.45) return { full: false, hitRate: 0 };
  return { full: false, hitRate: 0.4 + rand(meta.suiteKey, meta.runIndex, "hr") * 0.5 };
}

function caseCached(regime: CacheRegime, meta: RunMeta, seed: string | number): boolean {
  if (regime.full) return true;
  if (regime.hitRate === 0) return false;
  return rand(meta.suiteKey, seed, "hit") < regime.hitRate;
}

/**
 * The provider the matrix suite's `fallback:` chains hand off to.
 *
 * Deliberately **not** one of the suite's configured providers: a
 * `fallback_only` provider forms no matrix column of its own, so it is the
 * shape that proves the per-provider spend legend can name a provider the grid
 * never shows — and that a fallback's tokens are billed to it rather than to
 * the primary it stood in for.
 */
export const FALLBACK_PROVIDER = "reserve-mini";

/**
 * Whether a fallback stood in for this case's configured provider.
 *
 * Kept to matrix suites, and to a small deterministic slice of them: a walked
 * chain is rare in practice, and a fixture where it were common would make the
 * amber handoff callouts read as the norm rather than the exception. Skipped
 * cases are excluded — nothing was ever sent to a provider for one.
 */
function caseAnsweredBy(
  meta: RunMeta,
  seed: string | number,
  status: CaseStatus,
): string | null {
  if (!meta.suiteDef.matrix) return null;
  if (status === "skip") return null;
  return rand(meta.suiteKey, seed, "fallback") < 0.07 ? FALLBACK_PROVIDER : null;
}

export function generateCases(runId: string): MockCaseRow[] {
  const cached = CASE_CACHE.get(runId);
  if (cached) return cached;
  const meta = RUN_META_BY_ID.get(runId);
  if (!meta) return [];
  const rows = meta.suiteDef.matrix
    ? generateMatrixCases(meta, meta.suiteDef.matrix)
    : generateFlatCases(meta);
  CASE_CACHE.set(runId, rows);
  return rows;
}

/** Single-provider (non-matrix) suite: one case per `idx`, all sharing a
 *  constant provider, no prompt dimension, `test_id === case_key`, `repeat 0`. */
function generateFlatCases(meta: RunMeta): MockCaseRow[] {
  const rows: MockCaseRow[] = [];
  const regime = cacheRegime(meta);
  for (let i = 0; i < meta.caseCount; i++) {
    const status = statusFor(meta, i);
    const asserts = leanAsserts(meta, i, status);
    const tags: string[] =
      rand(meta.suiteKey, i, "ct") > 0.6 ? [pick(TAG_POOL, meta.suiteKey, i, "ctag")] : [];
    const latency = Math.round(120 + rand(meta.suiteKey, i, "lat") * 4200);
    const pt = Math.round(180 + rand(meta.suiteKey, i, "pt") * 900);
    const ct = Math.round(40 + rand(meta.suiteKey, i, "cot") * 500);
    const caseKey = `case-${String(i).padStart(4, "0")}`;
    // An empty output has no preview to show: the row is exactly the one whose
    // Preview cell is a bare dash, which is what `empty_reason` explains.
    const emptyReason = caseEmptyReason(meta, i, status);
    rows.push({
      case_key: caseKey,
      idx: i,
      name: caseName(i),
      tags,
      status,
      output_preview: emptyReason ? null : outputPreview(meta, i, status),
      error: caseError(status, i),
      error_class: caseErrorClass(status, i),
      empty_reason: emptyReason,
      asserts,
      prompt_tokens: pt,
      completion_tokens: ct,
      cost_usd: round4((pt * 3 + ct * 15) / 1_000_000),
      latency_ms: latency,
      provider_id: "openai",
      prompt_id: null,
      test_id: caseKey,
      repeat: 0,
      cached: caseCached(regime, meta, i),
      // Flat suites configure a single provider and no chain behind it.
      answered_by_provider_id: null,
      seed: i,
      output_hash: hash(
        meta.suiteKey,
        "out",
        i,
        outputRevision(meta.suiteKey, i, meta.runIndex),
      ).toString(16),
    });
  }
  return rows;
}

/** Matrix suite: the full provider × prompt × test × repeat grid, iterated
 *  test → provider → prompt → repeat so the first-seen `idx` order the matrix
 *  endpoint pivots on yields every `(provider, prompt)` column within the first
 *  test's rows. Repeats of a cell usually share their output hash; a subset of
 *  cells are flaky and mutate on a later repeat (a `distinct_outputs > 1`
 *  signal). */
function generateMatrixCases(meta: RunMeta, spec: MatrixSpec): MockCaseRow[] {
  const rows: MockCaseRow[] = [];
  const regime = cacheRegime(meta);
  let idx = 0;
  for (let t = 0; t < spec.tests; t++) {
    const testId = `test-${String(t).padStart(3, "0")}`;
    const name = caseName(`t${t}`);
    for (const provider of spec.providers) {
      for (const prompt of spec.prompts) {
        // A subset of cells produce unstable output across their repeats.
        const flakyCell = rand(meta.suiteKey, "mflaky", t, provider, prompt) > 0.78;
        for (let rep = 0; rep < spec.repeats; rep++) {
          const seed = `${testId}|${provider}|${prompt}|${rep}`;
          const status = statusFor(meta, seed);
          const asserts = leanAsserts(meta, seed, status);
          const tags: string[] =
            rand(meta.suiteKey, seed, "ct") > 0.6
              ? [pick(TAG_POOL, meta.suiteKey, seed, "ctag")]
              : [];
          const latency = Math.round(120 + rand(meta.suiteKey, seed, "lat") * 4200);
          const pt = Math.round(180 + rand(meta.suiteKey, seed, "pt") * 900);
          const ct = Math.round(40 + rand(meta.suiteKey, seed, "cot") * 500);
          // Output revision is stable across a cell's repeats unless the cell is
          // flaky and this repeat mutates.
          const rev =
            flakyCell && rep > 0 && rand(meta.suiteKey, t, provider, prompt, rep, "mut") < 0.5
              ? rep
              : 0;
          const emptyReason = caseEmptyReason(meta, seed, status);
          rows.push({
            case_key: `case-${String(idx).padStart(4, "0")}`,
            idx,
            name,
            tags,
            status,
            output_preview: emptyReason
              ? null
              : matrixPreview(status, provider, name, rev),
            error: caseError(status, idx),
            error_class: caseErrorClass(status, idx),
            empty_reason: emptyReason,
            asserts,
            prompt_tokens: pt,
            completion_tokens: ct,
            cost_usd: round4((pt * 3 + ct * 15) / 1_000_000),
            latency_ms: latency,
            provider_id: provider,
            prompt_id: prompt,
            test_id: testId,
            repeat: rep,
            cached: caseCached(regime, meta, seed),
            answered_by_provider_id: caseAnsweredBy(meta, seed, status),
            seed,
            output_hash: hash(meta.suiteKey, testId, provider, prompt, "out", rev).toString(16),
          });
          idx++;
        }
      }
    }
  }
  return rows;
}

/** The failure reason an errored case carries instead of an output. */
/**
 * The mock's error spread. Deliberately more than one class: the point of the
 * breakdown is that "14 errors" is usually several different problems, and a
 * fixture with one class would render a bar with one segment and prove nothing.
 */
const ERROR_KINDS = [
  { class: "provider_unavailable", message: "provider returned 502 after 3 retries" },
  { class: "provider_rate_limit", message: "provider error: HTTP 429" },
  { class: "grader_failed", message: "llm-rubric assertion errored: verdict truncated" },
] as const;

function errorKind(seed: string | number): (typeof ERROR_KINDS)[number] {
  const n = typeof seed === "number" ? seed : seed.length;
  // Fallback rather than a non-null assertion: the modulo is always in range,
  // but tsc cannot know that and an assertion would hide a real bug later.
  return ERROR_KINDS[Math.abs(n) % ERROR_KINDS.length] ?? ERROR_KINDS[0];
}

function caseError(status: CaseStatus, seed: string | number = 0): string | null {
  return status === "error" ? errorKind(seed).message : null;
}

function caseErrorClass(status: CaseStatus, seed: string | number = 0): string | null {
  return status === "error" ? errorKind(seed).class : null;
}

/**
 * The empty-output reasons the fixture draws from. More than one on purpose:
 * "4 empty" is usually several different problems — a refusal, a truncation
 * and a tool-only reply each call for a different fix — and a fixture with one
 * reason would render a breakdown that proves nothing.
 */
const EMPTY_REASONS = ["refusal", "truncated", "thinking_only", "tool_use_only"] as const;

/**
 * Why this case came back with nothing gradeable in it, or null when it did
 * not.
 *
 * Kept to failing cases, which is the common shape rather than the only one: a
 * case whose assertions are all metric bounds can come back empty and still
 * pass, but the fixture does not model an assertion mix that would show it.
 * Errored cases are excluded outright — those never produced an output for a
 * reason to be about.
 */
function caseEmptyReason(
  meta: RunMeta,
  seed: string | number,
  status: CaseStatus,
): string | null {
  if (status !== "fail") return null;
  if (rand(meta.suiteKey, seed, "empty") > 0.22) return null;
  return pick(EMPTY_REASONS, meta.suiteKey, seed, "emptyreason");
}

function outputPreview(
  meta: RunMeta,
  seed: string | number,
  status: CaseStatus,
): string | null {
  if (status === "skip") return "(skipped)";
  // An errored case produced no output, so there is nothing to preview; the
  // reason lives in `error` instead.
  if (status === "error") return null;
  const rev = outputRevision(meta.suiteKey, seed, meta.runIndex);
  const verb = pick(VERBS, "verb", seed);
  const noun = pick(NOUNS, "noun", seed);
  return `The agent ${verb} the ${noun} and produced revision r${rev}.`;
}

/** Grid preview for a matrix case; names the provider so the provider column
 *  and the preview visibly agree. */
function matrixPreview(
  status: CaseStatus,
  provider: string,
  name: string,
  rev: number,
): string | null {
  if (status === "skip") return "(skipped)";
  if (status === "error") return null;
  return `${provider} ${name} → revision r${rev}.`;
}

/** Which rendering the case's full output exercises. Stable per case index
 *  (not per run), so a case keeps its shape across the run series while the
 *  revision inside still changes — this is what lets the OutputViewer demo show
 *  a deterministic mix of json / markdown / plain-text outputs. Roughly 18%
 *  markdown, 12% text, the rest json. Case index 0 is json by construction, so
 *  the run-detail e2e's first passing case still renders a JSON tree. */
type OutputFlavor = "json" | "markdown" | "text";
function outputFlavor(seed: string | number): OutputFlavor {
  const r = rand("outflavor", seed);
  if (r < 0.18) return "markdown";
  if (r < 0.3) return "text";
  return "json";
}

export function fullOutput(meta: RunMeta, seed: string | number, status: CaseStatus): string {
  if (status === "skip") return "";
  if (status === "error") {
    return "Error: upstream provider returned HTTP 502 (Bad Gateway) after 3 retries.";
  }
  const rev = outputRevision(meta.suiteKey, seed, meta.runIndex);
  const noun = pick(NOUNS, "noun", seed);
  const verb = pick(VERBS, "verb", seed);
  const intent = noun.replace(/\s+/g, "_");
  const confidence = round2(0.6 + rand(meta.suiteKey, seed, "conf") * 0.4);

  switch (outputFlavor(seed)) {
    case "markdown":
      return [
        `# Resolution summary`,
        ``,
        `The agent **${verb}** the **${noun}** and returned a structured verdict (revision r${rev}).`,
        ``,
        `## Checks`,
        ``,
        `- Parsed the request payload`,
        `- Applied policy and safety checks`,
        `- Emitted the final decision`,
        ``,
        `Confidence landed at \`${confidence}\`. See the [policy reference](https://example.com/policy) for the rubric.`,
        ``,
        "```json",
        `{ "intent": "${intent}", "revision": ${rev}, "confidence": ${confidence} }`,
        "```",
        ``,
      ].join("\n");
    case "text":
      return (
        `The agent ${verb} the ${noun} without incident. The final decision was ` +
        `recorded as revision r${rev} with a confidence of ${confidence}. No ` +
        `escalation or manual review was required for this case.`
      );
    case "json":
      return [
        `{`,
        `  "intent": "${intent}",`,
        `  "action": "resolve",`,
        `  "revision": ${rev},`,
        `  "confidence": ${confidence},`,
        `  "explanation": "Resolved the ${noun}; applied policy checks and returned a structured result."`,
        `}`,
      ].join("\n");
  }
}

/**
 * A synthesized authored-criteria blob (the assertion's definition) for the
 * mock case-detail drawer. The lean list carries only the assert kind, so we
 * fabricate a plausible `criteria` object per kind — the flattened assertion
 * `type` plus its type-specific fields, and `negate` when the check is inverted.
 */
function synthCriteria(
  kind: AssertName,
  noun: string,
  negate: boolean,
): Record<string, string | number | boolean> {
  const base: Record<string, string | number | boolean> =
    kind === "llm-rubric"
      ? {
          type: kind,
          value:
            "The assistant stays empathetic, refuses unsafe or toxic requests, and never discloses PII.",
          threshold: 0.7,
        }
      : kind === "length"
        ? { type: kind, min: 20, max: 600 }
        : kind === "regex"
          ? { type: kind, value: "\\b(refund|apolog\\w+)\\b" }
          : { type: kind, value: noun };
  if (negate) base.negate = true;
  return base;
}

/**
 * Full per-assert verdict for the case-detail endpoint's `AssertResult[]`. When
 * `v2` (the schema-v2 fixture suite), each assert also carries a synthesized
 * `criteria` blob; v1 suites omit it, mirroring pre-v2.1 stored runs.
 */
export function detailAsserts(
  meta: RunMeta,
  seed: string | number,
  status: CaseStatus,
  lean: CaseAssertLean[],
  v2: boolean,
): AssertResult[] {
  return lean.map((a) => {
    const weight = round2(0.5 + rand(meta.suiteKey, seed, a.kind, "w") * 1.5);
    const st: AssertStatus = a.passed ? "pass" : status === "error" ? "error" : "fail";
    const reason = a.passed
      ? `${a.kind} check satisfied (score ${a.score.toFixed(2)}).`
      : status === "error"
        ? `${a.kind} could not be evaluated: grader errored on provider output.`
        : `${a.kind} check failed: expected value not found (score ${a.score.toFixed(2)}).`;
    const negate = rand(meta.suiteKey, seed, a.kind, "neg") < 0.15;
    return {
      kind: a.kind,
      status: st,
      score: a.score,
      weight,
      reason,
      details: a.passed ? undefined : { expected: "policy-compliant", got: "divergent" },
      ...(v2
        ? { criteria: synthCriteria(a.kind, pick(NOUNS, a.kind, seed), negate) }
        : {}),
      cached: false,
      // Only the kinds that call a model to decide carry a cost. A `contains`
      // check is local arithmetic, and pricing it would misrepresent what a
      // suite actually spends on grading.
      ...(a.kind === "llm-rubric" || a.kind === "similar"
        ? {
            cost_usd: round4(
              0.0004 + rand(meta.suiteKey, seed, a.kind, "gcost") * 0.004,
            ),
          }
        : {}),
    };
  });
}

/** A case's overall score = mean of its per-assert scores (or a pass/fail
 *  fallback when nothing was asserted). Matches what `caseDetail` derives, so
 *  the lean list score and the full `CaseResult.score` agree. */
export function caseScore(row: MockCaseRow): number {
  if (row.asserts.length === 0) return row.status === "pass" ? 1 : 0;
  return round2(row.asserts.reduce((s, a) => s + a.score, 0) / row.asserts.length);
}

/** Project the internal row down to the lean wire shape `GET .../cases` returns
 *  (notably: no `tags` — see `CaseListItem`'s doc comment on the generated type).
 *  The matrix-cell identity columns (migration 3) are surfaced verbatim from the
 *  row: single-provider suites carry one constant provider with a null prompt;
 *  matrix suites carry the real grid coordinates. `stop_reason` stays null (the
 *  fixture never sets it). */
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
    provider_id: c.provider_id,
    prompt_id: c.prompt_id,
    test_id: c.test_id,
    repeat: c.repeat,
    score: caseScore(c),
    stop_reason: null,
    cached: c.cached,
    error: c.error,
    error_class: c.error_class,
    // Omitted rather than null when the case was not empty, matching the wire:
    // the field is `#[ts(optional)]`, and a reader must not read `null` as a
    // recorded "not empty".
    ...(c.empty_reason != null ? { empty_reason: c.empty_reason } : {}),
    // Same `#[ts(optional)]` treatment: omitted, not null, when the configured
    // provider answered — a reader must not read `null` as "no fallback was
    // configured", only as "this row never said".
    ...(c.answered_by_provider_id != null
      ? { answered_by_provider_id: c.answered_by_provider_id }
      : {}),
  };
}
