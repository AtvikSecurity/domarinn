import { expect, test } from "@playwright/test";
import {
  MONEY_RUN,
  MONEY_RUN_BASELINE,
  OUTPUT_CHANGED_CASE,
  caseParam,
} from "./helpers";

test.describe("Case drawer history timeline", () => {
  test("expands the timeline, marks the current run + an output change, and deep-links an older run", async ({
    page,
  }) => {
    // Open the drawer directly on a case that appears in every run of the suite
    // and whose output differs between the two latest runs (OUTPUT_CHANGED_CASE
    // is pinned in fixtures.test.ts / helpers.ts).
    await page.goto(`/runs/${MONEY_RUN}?case=${OUTPUT_CHANGED_CASE}`);

    const drawer = page.getByRole("dialog");
    await expect(drawer).toBeVisible();

    // The section is expanded by default; the history window loads directly.
    const toggle = drawer.getByRole("button", { name: "History" });
    await expect(toggle).toBeVisible();
    await expect(toggle).toHaveAttribute("aria-expanded", "true");

    // checkout-agent/regression has 12 runs carrying this case -> >= 2 squares.
    const squares = drawer.locator("[data-history-square]");
    await expect(squares.first()).toBeVisible();
    expect(await squares.count()).toBeGreaterThanOrEqual(2);

    // Exactly one square is ring-highlighted as the current run, and it is
    // MONEY_RUN's square.
    await expect(
      drawer.locator('[data-history-square][data-current="true"]'),
    ).toHaveCount(1);
    await expect(
      drawer.locator(`[data-history-square][data-run-id="${MONEY_RUN}"]`),
    ).toHaveAttribute("data-current", "true");

    // The output changed between consecutive runs -> at least one diamond marker.
    await expect(drawer.locator("[data-output-changed]").first()).toBeVisible();

    // Clicking an OLDER run's square (the baseline, second-newest) navigates to
    // that run with the same case still selected; the drawer re-mounts there.
    await drawer
      .locator(`[data-history-square][data-run-id="${MONEY_RUN_BASELINE}"]`)
      .click();

    await expect(page).toHaveURL(
      new RegExp(`/runs/${MONEY_RUN_BASELINE}\\?case=${OUTPUT_CHANGED_CASE}`),
    );
    await expect(page.getByRole("dialog")).toBeVisible();
    expect(caseParam(page)).toBe(OUTPUT_CHANGED_CASE);
  });
});
