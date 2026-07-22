import { expect, test } from "@playwright/test";

test.describe("Global search", () => {
  test("dropdown shows grouped hits and quick-jumps to a run", async ({ page }) => {
    await page.goto("/");

    const input = page.getByRole("combobox", { name: "Search runs and cases" });
    await input.fill("checkout");

    // Debounced dropdown with the Runs group ("checkout" matches the
    // checkout-agent project's run metadata).
    await expect(page.getByText("Runs", { exact: true })).toBeVisible();
    await page.locator('[data-search-hit="run"]').first().click();
    await expect(page).toHaveURL(/\/runs\//);
  });

  test("Enter opens /search; a case hit opens the run's case drawer", async ({ page }) => {
    await page.goto("/");

    const input = page.getByRole("combobox", { name: "Search runs and cases" });
    // "coupon" comes from the fixture case vocabulary (names/outputs).
    await input.fill("coupon");
    await input.press("Enter");

    await expect(page).toHaveURL(/\/search\?q=coupon/);
    await expect(page.getByText(/^Cases \(\d+\)$/)).toBeVisible();

    // Snippets highlight the matched token.
    await expect(page.locator("mark").first()).toBeVisible();

    await page.locator('a[href*="?case="]').first().click();
    await expect(page).toHaveURL(/\/runs\/.+\?case=/);
    await expect(page.getByRole("dialog")).toBeVisible();
  });

  test("junk queries return a friendly empty state, not an error", async ({ page }) => {
    await page.goto('/search?q=xyzzynothingmatchesthis');
    await expect(page.getByText("No matches")).toBeVisible();
  });
});
