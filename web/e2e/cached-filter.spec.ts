import { expect, test, type Page } from "@playwright/test";

/** The filter bar's cached facet — a segmented control, not a select. */
const cachedFacet = (page: Page) =>
  page.getByRole("radiogroup", { name: "Cached runs" });

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

  test("the filter bar narrows to only cached runs", async ({ page }) => {
    await page.goto("/runs");

    await cachedFacet(page).getByRole("radio", { name: "Only" }).click();
    await expect(page).toHaveURL(/cached=only/);
    await expect(
      page.getByRole("link", { name: CANARY_CACHED, exact: true }),
    ).toBeVisible();
    // The fresh first run is not fully cached, so it drops out.
    await expect(
      page.getByRole("link", { name: CANARY_FRESH, exact: true }),
    ).toBeHidden();
  });

  test("the revealed view offers a way back", async ({ page }) => {
    await page.goto("/runs?cached=all");

    await expect(page.getByText(/Showing cached runs/)).toBeVisible();
    await page.getByRole("button", { name: "Hide", exact: true }).click();
    // An explicit token rather than a bare URL: the view has to survive being
    // shared with someone whose own preference is to show them.
    await expect(page).toHaveURL(/cached=exclude/);
    await expect(page.getByText(/6 fully cached runs hidden/)).toBeVisible();
  });

  test("the filter bar sets a preference that outlives the URL", async ({
    page,
  }) => {
    await page.goto("/runs");
    await cachedFacet(page).getByRole("radio", { name: "Shown" }).click();
    await expect(page).toHaveURL(/cached=all/);

    // Arrive with nothing in the URL: the stored preference decides, which is
    // what carries the choice to every other run surface.
    await page.goto("/runs");
    await expect(page).not.toHaveURL(/cached=/);
    await expect(
      page.getByRole("link", { name: CANARY_CACHED, exact: true }),
    ).toBeVisible();
    await expect(page.getByText(/fully cached runs hidden/)).toBeHidden();
  });

  test("an explicit URL beats a stored preference", async ({ page }) => {
    await page.goto("/runs");
    await cachedFacet(page).getByRole("radio", { name: "Shown" }).click();

    // A shared link has to mean the same thing to whoever opens it, whatever
    // they normally prefer.
    await page.goto("/runs?cached=exclude");
    await expect(page.getByText(/6 fully cached runs hidden/)).toBeVisible();
    await expect(
      page.getByRole("link", { name: CANARY_CACHED, exact: true }),
    ).toBeHidden();
  });

  test("the per-view toggle does not retrain the preference", async ({
    page,
  }) => {
    await page.goto("/runs");
    await page.getByRole("button", { name: "Show", exact: true }).click();
    await expect(page).toHaveURL(/cached=all/);

    // Revealing once is a one-view override, not a standing choice...
    expect(
      await page.evaluate(() => localStorage.getItem("domarinn.cached.mode")),
    ).toBeNull();

    // ...so the next visit without a param hides them again.
    await page.goto("/runs");
    await expect(page.getByText(/6 fully cached runs hidden/)).toBeVisible();
  });

  // Two surfaces deliberately never hide cached runs, because hiding would
  // cost them information rather than noise. Both are easy to "fix" into a
  // regression by someone making the app consistent, so both are pinned.
  test("the overview marks a fully cached headline instead of hiding it", async ({
    page,
  }) => {
    await page.goto("/");
    // A suite whose newest CI run was fully cached still has a real status.
    // Were it hidden, an older run would be promoted to "latest" and the card
    // would state a stale number as current — so the chip's presence is the
    // proof the run survived.
    await expect(page.getByText("cached", { exact: true }).first()).toBeVisible();
  });

  test("the compare pickers offer fully cached runs, labelled", async ({
    page,
  }) => {
    await page.goto(`/runs/${CANARY_FRESH}/compare/${CANARY_CACHED}`);

    // A run missing from a picker is indistinguishable from one that never
    // happened, and comparing against a fully-cached baseline is exactly how
    // you check a config change against known-identical inputs.
    const base = page.getByRole("combobox", { name: "Base run" });
    await expect(
      base.locator("option", { hasText: "· cached" }).first(),
    ).toBeAttached();
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
