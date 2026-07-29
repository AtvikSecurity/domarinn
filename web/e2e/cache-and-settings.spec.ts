import { expect, test } from "@playwright/test";

test.describe("Cache stats page", () => {
  test("renders cache metrics", async ({ page }) => {
    await page.goto("/cache");

    await expect(page.getByRole("heading", { name: "Cache", exact: true })).toBeVisible();

    // Tiles from the fixture cacheStats() (assert unique values so labels and
    // repeated counts don't collide under strict mode).
    await expect(page.getByText("Entries", { exact: true })).toBeVisible();
    await expect(page.getByText("Total size", { exact: true })).toBeVisible();
    await expect(page.getByText("256.0 MB")).toBeVisible();
    await expect(page.getByText("Lookup hit rate", { exact: true })).toBeVisible();
    await expect(page.getByText("80.0%")).toBeVisible();
    await expect(page.getByText("Hits", { exact: true })).toBeVisible();
    await expect(page.getByText("19,233")).toBeVisible();

    await expect(page.getByRole("heading", { name: "Prune cache" })).toBeVisible();
  });
});

test.describe("Settings page", () => {
  test("shows server meta and stores a token in localStorage", async ({ page }) => {
    await page.goto("/settings");

    await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();

    // Server metadata from GET /meta (scoped to the Server card).
    const serverCard = page
      .locator("section")
      .filter({ has: page.getByRole("heading", { name: "Server" }) });
    await expect(serverCard.getByText("domarinn")).toBeVisible();
    await expect(serverCard.getByText("0.1.0-mock")).toBeVisible();
    await expect(serverCard.getByText("open")).toBeVisible();
    await expect(serverCard.getByText("mock fixture")).toBeVisible();

    // No token initially.
    await expect(page.getByText("No token is set.")).toBeVisible();
    expect(
      await page.evaluate(() => localStorage.getItem("domarinn.token")),
    ).toBeNull();

    // Set a token via the Access token card.
    await page.getByPlaceholder("paste token").fill("e2e-secret-token");
    await page.getByRole("button", { name: "Save", exact: true }).click();

    await expect(page.getByText("A token is currently set.")).toBeVisible();
    expect(
      await page.evaluate(() => localStorage.getItem("domarinn.token")),
    ).toBe("e2e-secret-token");
  });
});
