import type { AssertName } from "@/api";

// ---------------------------------------------------------------------------
// Static shape of the world.
// ---------------------------------------------------------------------------

export const NOW = Date.UTC(2026, 6, 19, 15, 0, 0); // fixed reference time
export const DAY = 86_400_000;
export const RESULT_SCHEMA_VERSION = 1;

/** A suite whose cases are shaped as a provider × prompt × test × repeat grid
 *  (migration-3 matrix identity). When present on a `SuiteDef`, every run in the
 *  suite emits `providers.length × prompts.length × tests × repeats` cases whose
 *  `provider_id`/`prompt_id`/`test_id`/`repeat` carry real, >1-distinct values —
 *  the fixture data Task 12's matrix view (and this task's provider/prompt
 *  filters + columns) run against. Suites without a `matrix` are single-provider:
 *  one distinct `provider_id`, a null `prompt_id`, `test_id === case_key`,
 *  `repeat === 0` (what real single-provider runs look like). */
export interface MatrixSpec {
  providers: string[];
  prompts: string[];
  /** Distinct tests (matrix rows) per run. */
  tests: number;
  /** Repeats of each test × provider × prompt cell. */
  repeats: number;
}

export interface SuiteDef {
  project: string;
  suite: string;
  /** Real `AssertName` kinds (label === kind on the wire, per CaseAssertLean's
   *  doc comment) always evaluated for every case in this suite. */
  labels: AssertName[];
  runs: number;
  featured?: boolean;
  /** When set, the suite is matrix-shaped (see `MatrixSpec`). */
  matrix?: MatrixSpec;
}

export const SUITE_DEFS: SuiteDef[] = [
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
    // The one matrix-shaped suite: 3 providers × 2 prompts × 12 tests × 2
    // repeats = 144 cases per run. Its runs are the fixtures the matrix view and
    // the provider/prompt filters exercise. Not referenced by the MONEY_RUN e2e
    // constants, so its per-run case counts are free to differ from the others.
    project: "search-rerank",
    suite: "ndcg-eval",
    labels: ["is-json", "contains", "cost", "llm-rubric"],
    runs: 10,
    matrix: {
      providers: ["gpt-5-mini", "claude-sonnet", "llama-70b"],
      prompts: ["baseline", "cot-v2"],
      tests: 12,
      repeats: 2,
    },
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

export const VERBS = [
  "handles", "rejects", "renders", "summarizes", "classifies", "extracts",
  "refuses", "escalates", "validates", "retries", "parses", "ranks",
];
export const NOUNS = [
  "empty cart", "expired coupon", "duplicate order", "unicode address",
  "partial refund", "gift card", "tax exemption", "backorder item",
  "fraud signal", "loyalty tier", "split shipment", "price override",
  "malformed json", "PII in prompt", "toxic request", "ambiguous intent",
];
export const BRANCHES = ["main", "main", "main", "feat/new-grader", "fix/tokenizer", "chore/deps"];
export const TAG_POOL = ["nightly", "pr", "release", "canary", "regression", "smoke"];
