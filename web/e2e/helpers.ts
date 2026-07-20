// Shared constants + tiny helpers for the E2E specs. Values here mirror the
// deterministic fixture in src/mocks/fixtures.ts.

import type { Page } from "@playwright/test";

/** The featured ~500-case "money" run (latest run of checkout-agent/regression). */
export const MONEY_RUN = "checkout-agent-regression-12";

/** Assert labels rendered as columns for the regression suite. */
export const REGRESSION_ASSERT_LABELS = [
  "schema_valid",
  "answer_match",
  "no_pii",
  "tone",
  "latency_budget",
] as const;

/** Read the `?case=` value out of the current URL, if present. */
export function caseParam(page: Page): string | null {
  return new URL(page.url()).searchParams.get("case");
}

/** Read the `?delta=` value out of the current URL, if present. */
export function deltaParam(page: Page): string | null {
  return new URL(page.url()).searchParams.get("delta");
}
