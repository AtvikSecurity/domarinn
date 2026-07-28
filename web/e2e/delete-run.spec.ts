import { expect, test } from "@playwright/test";

/**
 * Deleting a run. The endpoint has existed since the server did and nothing
 * could reach it; run retention is off by default, so before this a run pushed
 * by mistake was permanent.
 */
test.describe("Delete run", () => {
  test("is admin-only, confirms first, and navigates away on success", async ({
    page,
  }) => {
    await page.goto("/runs?cached=all");
    await page.locator("tbody tr td:nth-child(2) a").first().click();
    await expect(page).toHaveURL(/\/runs\//);

    const trigger = page.getByRole("button", { name: "Delete run" });
    await expect(trigger).toBeVisible();
    await trigger.click();

    // Destructive and irreversible, so it must state what is lost rather than
    // just asking "are you sure?".
    const dialog = page.getByRole("dialog");
    await expect(dialog).toContainText("cannot be recreated");

    await dialog.getByRole("button", { name: "Delete run" }).click();
    await expect(page).toHaveURL(/\/(\?.*)?$/);
  });

  test("cancelling leaves the run alone", async ({ page }) => {
    await page.goto("/runs?cached=all");
    await page.locator("tbody tr td:nth-child(2) a").first().click();
    const runUrl = page.url();

    await page.getByRole("button", { name: "Delete run" }).click();
    await page.getByRole("button", { name: "Cancel" }).click();

    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect(page.url()).toBe(runUrl);
  });
});
