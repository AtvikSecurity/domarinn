import { expect, test } from "@playwright/test";

// The stable suite (SuiteDef.stable in src/mocks/fixtures/suites.ts): run 01
// is fresh, runs 02-07 are fully cached and all-pass — the six runs the
// default `cached=exclude` view hides.
const CANARY_FRESH = "checkout-agent-canary-01";
const CANARY_CACHED = "checkout-agent-canary-07";
// A partially cached run (22 hits / 18 misses per the deterministic fixtures):
// the run-detail Fresh-only chip only renders for mixed runs like this one.
const PARTIAL_RUN = "support-bot-tone-and-safety-09";

test.describe("Cached-runs filter", () => {
  test("hides fully cached passing runs by default, with a reveal affordance", async ({
    page,
  }) => {
    await page.goto("/runs");

    // The suppression is announced, never silent.
    await expect(page.getByText(/6 fully cached runs hidden/)).toBeVisible();

    // Cached canary re-runs are hidden; the fresh first run stays.
    await expect(
      page.getByRole("link", { name: CANARY_FRESH, exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("link", { name: CANARY_CACHED, exact: true }),
    ).toBeHidden();

    // One click reveals them, encoded in the shareable URL.
    await page.getByRole("button", { name: "Show", exact: true }).click();
    await expect(page).toHaveURL(/cached=all/);
    const cachedRow = page.getByRole("row").filter({
      has: page.getByRole("link", { name: CANARY_CACHED, exact: true }),
    });
    await expect(cachedRow).toBeVisible();
    // Revealed cached rows are visibly labeled.
    await expect(cachedRow.getByText("cached", { exact: true })).toBeVisible();
  });

  test("the filter bar select narrows to only cached runs", async ({ page }) => {
    await page.goto("/runs");

    await page
      .getByRole("combobox", { name: "Cached runs" })
      .selectOption("only");
    await expect(page).toHaveURL(/cached=only/);
    await expect(
      page.getByRole("link", { name: CANARY_CACHED, exact: true }),
    ).toBeVisible();
    // The fresh first run is not fully cached, so it drops out.
    await expect(
      page.getByRole("link", { name: CANARY_FRESH, exact: true }),
    ).toBeHidden();
  });

  test("a fully cached run's detail page shows the cache tile and per-case pills", async ({
    page,
  }) => {
    await page.goto(`/runs/${CANARY_CACHED}`);

    await expect(page.getByText("100% cached")).toBeVisible();
    // Every case row carries the muted cached pill (scope to the grid so the
    // tile's "% cached" text can't satisfy the assertion).
    await expect(
      page.getByRole("row").getByText("cached", { exact: true }).first(),
    ).toBeVisible();
  });

  test("a partially cached run offers the Fresh-only case filter", async ({
    page,
  }) => {
    await page.goto(`/runs/${PARTIAL_RUN}`);

    const group = page.getByRole("group", { name: "Cached" });
    await expect(group).toBeVisible();
    await group.getByRole("button", { name: "Fresh only" }).click();
    await expect(page).toHaveURL(/cached=false/);

    // Only fresh responses remain: no cached pill anywhere in the case rows.
    await expect(page.getByRole("row").getByText("cached", { exact: true })).toHaveCount(0);
  });

  test("a fully fresh run shows neither chip group nor cached pills", async ({
    page,
  }) => {
    await page.goto(`/runs/${CANARY_FRESH}`);

    await expect(page.getByText("0% cached")).toBeVisible();
    await expect(page.getByRole("group", { name: "Cached" })).toBeHidden();
    await expect(
      page.getByRole("row").getByText("cached", { exact: true }),
    ).toHaveCount(0);
  });
});
