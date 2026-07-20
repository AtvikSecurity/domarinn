// Shared constants + tiny helpers for the E2E specs. Values here mirror the
// deterministic fixture in src/mocks/fixtures.ts.

import type { Page } from "@playwright/test";

/** The featured ~500-case "money" run (latest run of checkout-agent/regression). */
export const MONEY_RUN = "checkout-agent-regression-12";

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
