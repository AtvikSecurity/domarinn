import { expect, test } from "@playwright/test";

test.describe("Token-gated action", () => {
  test("pruning the cache without a token opens the token modal", async ({ page }) => {
    await page.goto("/cache");

    // No token is stored in this fresh context.
    expect(
      await page.evaluate(() => localStorage.getItem("measurellm.token")),
    ).toBeNull();

    // Kick off the admin-only prune (mock returns 401 without a bearer token).
    await page.getByRole("button", { name: /Prune cache/ }).click();
    await page.getByRole("button", { name: "Confirm prune" }).click();

    // The global 401 handler pops the token modal.
    const modal = page.getByRole("dialog");
    await expect(modal).toBeVisible();
    await expect(
      modal.getByRole("heading", { name: "Access token required" }),
    ).toBeVisible();

    // The page also surfaces the unauthorized error.
    await expect(page.getByText(/Unauthorized/)).toBeVisible();
  });
});
