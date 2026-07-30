import { expect, test } from "@playwright/test";

/**
 * The cache browser, driven through real layout.
 *
 * The grid is virtualized, so this is where row rendering and scrolling are
 * exercised against a browser that actually computes heights — the unit tests
 * shim jsdom's missing layout, which is enough for logic but not for this.
 */
test.describe("Cache entries", () => {
  test("is reachable from the stats page and opens an entry", async ({ page }) => {
    await page.goto("/cache");
    await page.getByRole("link", { name: /Browse entries/ }).click();

    await expect(page).toHaveURL(/\/cache\/entries/);
    await expect(
      page.getByRole("heading", { name: "Cache entries" }),
    ).toBeVisible();

    const grid = page.getByRole("grid");
    await expect(grid).toBeVisible();

    // Row 1 is the header; the first body row is rowindex 2.
    const firstRow = grid.getByRole("row").filter({ hasNot: page.getByRole("columnheader") }).first();
    await firstRow.click();

    await expect(page).toHaveURL(/entry=sha256%3A/);
    await expect(page.getByRole("dialog")).toBeVisible();

    // Esc closes it and clears the selection, so the URL stays shareable.
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toBeHidden();
    await expect(page).not.toHaveURL(/entry=/);
  });

  test("deep-links straight into an entry", async ({ page }) => {
    await page.goto("/cache/entries");
    const grid = page.getByRole("grid");
    await expect(grid).toBeVisible();

    const key = await grid
      .getByRole("row")
      .nth(1)
      .getAttribute("aria-label");
    expect(key).toContain("Cache entry");

    await page.goto("/cache/entries");
    await page.getByRole("grid").getByRole("row").nth(1).click();
    const url = page.url();

    // A fresh load of the same URL must land in the same place.
    await page.goto(url);
    await expect(page.getByRole("dialog")).toBeVisible();
  });

  test("sorting writes the url and search narrows the grid", async ({ page }) => {
    await page.goto("/cache/entries");
    await expect(page.getByRole("grid")).toBeVisible();

    await page.getByRole("columnheader", { name: /Size/ }).getByRole("button").click();
    await expect(page).toHaveURL(/sort=-?size/);

    await page.getByPlaceholder("request or output text").fill("refund");
    // Debounced at 300ms.
    await expect(page).toHaveURL(/q=refund/, { timeout: 3000 });
    await expect(page.getByRole("button", { name: /Clear \d+ filter/ })).toBeVisible();
  });

  test("an entry that has not been indexed says so rather than showing nothing", async ({
    page,
  }) => {
    await page.goto("/cache/entries?kind=unindexed");
    await expect(page.getByRole("grid")).toBeVisible();
    // "we have not looked yet" is a different statement from "there is nothing
    // there", and the row has to make that difference visible.
    await expect(page.getByText("indexing…").first()).toBeVisible();
  });

  test("no match shows a way back rather than a dead end", async ({ page }) => {
    await page.goto("/cache/entries?q=zzzznotpresentzzzz");
    await expect(page.getByText(/No entries match these filters/)).toBeVisible();
    await page.getByRole("button", { name: "Clear filters" }).click();
    await expect(page.getByRole("grid")).toBeVisible();
  });
});

test.describe("Cache tiers", () => {
  test("switching tier changes what is listed and what search means", async ({
    page,
  }) => {
    await page.goto("/cache/entries");
    await expect(page.getByRole("grid")).toBeVisible();
    await expect(page.getByPlaceholder("request or output text")).toBeVisible();

    await page.getByRole("radiogroup", { name: "Cache tier" }).getByText("Local disk").click();
    await expect(page).toHaveURL(/tier=local/);

    // The tier has no full-text index, and the box says so rather than
    // implying a capability it does not have.
    await expect(page.getByPlaceholder("substring match")).toBeVisible();
    await expect(page.getByText(/reading Local disk/)).toBeVisible();
  });
});
