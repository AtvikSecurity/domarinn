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
    // The run id is a link; following it must land on that run.
    await page.locator("a[href^='/runs/']").first().click();
    await expect(page).toHaveURL(/\/runs\/[^/?]+$/);
  });
});
