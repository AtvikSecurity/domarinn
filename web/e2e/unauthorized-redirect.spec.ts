import { expect, test } from "@playwright/test";

test.describe("Unauthorized redirect", () => {
  test("a 401 on a protected action redirects to /login, not a token popup", async ({
    page,
  }) => {
    await page.goto("/cache");

    // No token is stored in this fresh context.
    expect(
      await page.evaluate(() => localStorage.getItem("domarinn.token")),
    ).toBeNull();

    // Kick off the admin-only prune (mock returns 401 without a bearer token).
    await page.getByRole("button", { name: /Prune cache/ }).click();
    await page.getByRole("button", { name: "Confirm prune" }).click();

    // The global 401 handler routes to the login page instead of opening the
    // legacy "Access token required" dialog.
    await expect(page).toHaveURL(/\/login$/);
    await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();

    // The old token-paste modal is gone.
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(
      page.getByRole("heading", { name: "Access token required" }),
    ).toHaveCount(0);
  });
});
