import { expect, test } from "@playwright/test";

/**
 * The facet that makes a shared board usable once CI and developers both push:
 * the canonical stream is separable from iteration, and a run can be traced to
 * a person. Both are SERVER filters — the runs list is cursor-paginated, so
 * filtering in the browser would silently apply only to loaded pages.
 *
 * Every assertion here is an auto-retrying `expect(locator)` rather than a
 * manual `.count()` comparison: the list re-renders after a filter change, and
 * two separate `.count()` reads can straddle that render and compare numbers
 * that were never simultaneously true.
 *
 * Row assertions are scoped to `tbody`, because the Origin control itself
 * renders an option labelled "CI" — an unscoped text query matches the filter
 * you just clicked and never reaches zero.
 */

/** The run rows, excluding the filter bar's own controls. */
const rows = (page: import("@playwright/test").Page) => page.locator("tbody");

test.describe("Origin + actor filters", () => {
  test("origin=ci hides developer runs and round-trips through the URL", async ({
    page,
  }) => {
    await page.goto("/runs");
    await expect(page.getByRole("heading", { name: "Eval runs" })).toBeVisible();
    // Precondition: the unfiltered list holds both kinds, so what follows
    // proves a filter rather than an already-empty set.
    await expect(rows(page).getByText("local", { exact: true }).first()).toBeVisible();

    await page.getByRole("radio", { name: "CI", exact: true }).click();
    await expect(page).toHaveURL(/origin=ci/);

    await expect(rows(page).getByText("local", { exact: true })).toHaveCount(0);
    await expect(rows(page).getByText("CI", { exact: true }).first()).toBeVisible();
  });

  test("origin=local hides CI runs", async ({ page }) => {
    await page.goto("/runs?origin=local");
    await expect(page.getByRole("radio", { name: "Local" })).toBeChecked();

    await expect(rows(page).getByText("CI", { exact: true })).toHaveCount(0);
    await expect(rows(page).getByText("local", { exact: true }).first()).toBeVisible();
  });

  test("a deep-linked actor filter is reflected in the control and the rows", async ({
    page,
  }) => {
    await page.goto("/runs?actor=alice");
    await expect(page.getByLabel("Actor")).toHaveValue("alice");

    await expect(rows(page).getByText("alice", { exact: true }).first()).toBeVisible();
    // Nobody else's runs survive the filter.
    for (const other of ["bob", "dana", "erik"]) {
      await expect(rows(page).getByText(other, { exact: true })).toHaveCount(0);
    }
  });

  test("clearing filters removes the origin and actor params too", async ({
    page,
  }) => {
    await page.goto("/runs?origin=ci&actor=alice");
    await page.getByRole("button", { name: /Clear 2 filters/ }).click();
    await expect(page).not.toHaveURL(/origin=/);
    await expect(page).not.toHaveURL(/actor=/);
  });

  test("a dirty worktree is marked on the run's commit", async ({ page }) => {
    // `git_dirty` has been a stored column since the first migration and was
    // rendered nowhere. It is the one signal that says a result cannot be
    // reproduced from the commit shown beside it.
    await page.goto("/runs?cached=all");
    await expect(page.getByLabel("dirty worktree").first()).toBeVisible();
  });
});
