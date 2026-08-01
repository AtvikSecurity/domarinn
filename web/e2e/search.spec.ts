import { expect, test } from "@playwright/test";

test.describe("Global search", () => {
  test("dropdown shows grouped hits and quick-jumps to a run", async ({ page }) => {
    await page.goto("/runs");

    const input = page.getByRole("combobox", { name: "Search sets, runs and cases" });
    await input.fill("checkout");

    // Debounced dropdown. "checkout" matches the checkout-agent project's run
    // metadata full-text, so a Runs group is present alongside the Sets one.
    await expect(page.getByText("Runs", { exact: true })).toBeVisible();
    await page.locator('[data-search-hit="run"]').first().click();
    await expect(page).toHaveURL(/\/runs\//);
  });

  test("Enter opens /search; a case hit opens the run's case drawer", async ({ page }) => {
    await page.goto("/runs");

    const input = page.getByRole("combobox", { name: "Search sets, runs and cases" });
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

  test("jumps straight to a set by typing its project name", async ({ page }) => {
    await page.goto("/runs");

    const input = page.getByRole("combobox", {
      name: "Search sets, runs and cases",
    });
    await input.fill("checkout");

    // The server search indexes run and case contents only, so a project was
    // the one thing the search box could not find. It is matched client-side
    // against /sets and offered above the full-text groups.
    await expect(page.getByText("Sets", { exact: true })).toBeVisible();
    await page.locator('[data-search-hit="set"]').first().click();
    await expect(page).toHaveURL(/\/sets\/checkout-agent$/);
  });

  test("the full results page carries the same Sets group", async ({ page }) => {
    // A dropdown that offers a group "See all results" then omits would be
    // worse than not offering it.
    await page.goto("/search?q=checkout");
    await expect(page.getByText(/^Sets \(\d+\)$/)).toBeVisible();
    await page.getByRole("link", { name: /checkout-agent/ }).first().click();
    await expect(page).toHaveURL(/\/sets\/checkout-agent$/);
  });

  test("junk queries return a friendly empty state, not an error", async ({ page }) => {
    await page.goto('/search?q=xyzzynothingmatchesthis');
    await expect(page.getByText("No matches")).toBeVisible();
  });
});
