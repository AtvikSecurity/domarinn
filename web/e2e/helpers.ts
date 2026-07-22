// Shared constants + tiny helpers for the E2E specs. Values here mirror the
// deterministic fixture in src/mocks/fixtures.ts.

import type { Page } from "@playwright/test";

/** The featured ~500-case "money" run (latest run of checkout-agent/regression). */
export const MONEY_RUN = "checkout-agent-regression-12";

/** Latest run of the one matrix-shaped suite (search-rerank/ndcg-eval): 3
 *  providers × 2 prompts × 12 tests × 2 repeats = 144 cases. Its provider/prompt
 *  filter chips + grid columns render; single-provider runs like MONEY_RUN show
 *  neither. See SUITE_DEFS[matrix] in src/mocks/fixtures.ts. */
export const MATRIX_RUN = "search-rerank-ndcg-eval-10";
export const MATRIX_PROVIDERS = ["gpt-5-mini", "claude-sonnet", "llama-70b"] as const;
export const MATRIX_PROMPTS = ["baseline", "cot-v2"] as const;

/** The run immediately before MONEY_RUN in checkout-agent/regression — also
 *  its default compare baseline (see BASELINE_BY_SUITE in
 *  src/mocks/fixtures.ts). Real server route is `Path((id, other))` with no
 *  target-less form, so compare navigation always needs an explicit pair. */
export const MONEY_RUN_BASELINE = "checkout-agent-regression-11";

/** A case whose output differs between MONEY_RUN and MONEY_RUN_BASELINE while
 *  both sides still render output (pass→pass, differing revision) — the drawer
 *  baseline diff therefore shows a real two-sided diff. Determinism is pinned by
 *  the "output-changed case" assertion in src/mocks/fixtures.test.ts. */
export const OUTPUT_CHANGED_CASE = "case-0024";

/** The one fixture suite whose case details carry schema-v2 fields (rendered
 *  prompt, stop_reason, raw metadata) — see V2_SUITE_KEY in src/mocks/fixtures.ts.
 *  Its cases are pinned by the "schema-v2 case-detail fixture" tests there. */
export const V2_RUN = "support-bot-tone-and-safety-09";
/** A v2 case with a role-tagged system+user prompt, a clean `end_turn` stop, and
 *  raw provider metadata. */
export const V2_MESSAGES_CASE = "case-0000";
/** A v2 case whose stop_reason is the truncating `max_tokens`. */
export const V2_TRUNCATED_CASE = "case-0001";

/** Assert labels (real AssertName kinds) rendered as columns for the
 *  regression suite — see SUITE_DEFS in src/mocks/fixtures.ts. */
export const REGRESSION_ASSERT_LABELS = [
  "is-json",
  "contains",
  "llm-rubric",
  "latency",
  "cost",
] as const;

/** Read the `?case=` value out of the current URL, if present. */
export function caseParam(page: Page): string | null {
  return new URL(page.url()).searchParams.get("case");
}

/** Read the `?delta=` value out of the current URL, if present. */
export function deltaParam(page: Page): string | null {
  return new URL(page.url()).searchParams.get("delta");
}

/** Read the `?diff=` (compare diff mode) value out of the current URL. */
export function diffParam(page: Page): string | null {
  return new URL(page.url()).searchParams.get("diff");
}
