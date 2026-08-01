import { expect, test } from "@playwright/test";

/**
 * The status surface: "what is the state of everything right now?"
 *
 * Separate from the runs list because they sort incompatibly — the stream is
 * newest-first by definition, while this must not reorder because somebody
 * pushed a local run.
 */
test.describe("Overview", () => {
  test("is the root route and shows a card per suite", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByRole("heading", { name: "Overview" })).toBeVisible();
    // Each card names its suite as a heading, so the page is navigable by
    // structure rather than by reading tinted borders.
    const cards = page.getByRole("heading", { level: 2 });
    expect(await cards.count()).toBeGreaterThan(0);
  });

  test("the runs stream lives at /runs and both are reachable from the nav", async ({
    page,
  }) => {
    await page.goto("/");
    await page.getByRole("link", { name: "Runs", exact: true }).click();
    await expect(page).toHaveURL(/\/runs$/);
    await expect(page.getByRole("heading", { name: "Eval runs" })).toBeVisible();

    await page.getByRole("link", { name: "Overview", exact: true }).click();
    await expect(page).toHaveURL(/\/$/);
    await expect(page.getByRole("heading", { name: "Overview" })).toBeVisible();
  });

  test("a card drills through to that suite's runs", async ({ page }) => {
    await page.goto("/");
    await page.getByRole("link", { name: /runs? loaded/ }).first().click();
    await expect(page).toHaveURL(/\/runs\?project=/);
    await expect(page).toHaveURL(/suite=/);
  });

  test("the canonical run links to its detail page", async ({ page }) => {
    await page.goto("/");
    // The run id is a link; following it must land on that run. This also
    // pins the card overlay's stacking: the whole card is one stretched
    // anchor, and if these inner links were not raised above it Playwright
    // would fail here with the overlay intercepting the click.
    await page.locator("a[href^='/runs/']").first().click();
    await expect(page).toHaveURL(/\/runs\/[^/?]+$/);
  });

  test("a card's heading links to that set", async ({ page }) => {
    await page.goto("/");
    // The accessible name is assembled from separate spans, so match loosely.
    await page
      .getByRole("link", { name: /checkout-agent\s*\/\s*regression/ })
      .click();
    await expect(page).toHaveURL("/sets/checkout-agent/regression");
  });

  test("clicking the body of a card opens its set", async ({ page }) => {
    await page.goto("/");
    // The point of the stretched link: a card is a set, so what used to be
    // dead space in one is not dead. Clicking the card's centre — the
    // pass-rate and sparkline region, which contains no link of its own —
    // must land on the set.
    const card = page
      .getByRole("heading", {
        level: 2,
        name: /checkout-agent\s*\/\s*regression/,
      })
      .locator("xpath=ancestor::div[contains(@class,'rounded-xl')][1]");
    await card.click();
    await expect(page).toHaveURL("/sets/checkout-agent/regression");
  });
});
