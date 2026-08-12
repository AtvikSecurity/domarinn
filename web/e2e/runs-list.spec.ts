import { expect, test } from "@playwright/test";
import { MONEY_RUN, MONEY_RUN_BASELINE } from "./helpers";

// Latest run of each other suite (see src/mocks/fixtures.ts run counts).
const SEARCH_RERANK_RUN = "search-rerank-ndcg-eval-10";
const SUPPORT_BOT_RUN = "support-bot-tone-and-safety-09";
const SMOKE_RUN = "checkout-agent-smoke-08";

test.describe("Runs list", () => {
  test("renders suites grouped with pass-rate, and lists runs", async ({ page }) => {
    await page.goto("/runs");

    // Page shell.
    await expect(page.getByRole("heading", { name: "Eval runs" })).toBeVisible();
    // The mock build advertises itself so we know we're on the fixture.
    await expect(page.getByText("mock data")).toBeVisible();

    // Suites are grouped and show a pass-rate trend + per-run pass-rate badge.
    await expect(page.getByText("pass-rate trend").first()).toBeVisible();
    await expect(page.getByText(/^\d+(\.\d+)?%$/).first()).toBeVisible();

    // Featured + other-project runs are all listed as links to their detail pages.
    await expect(
      page.getByRole("link", { name: MONEY_RUN, exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("link", { name: SEARCH_RERANK_RUN, exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("link", { name: SUPPORT_BOT_RUN, exact: true }),
    ).toBeVisible();
  });

  test("clicking a header sorts the runs and round-trips through ?sort", async ({
    page,
  }) => {
    await page.goto("/runs");
    await expect(page.getByRole("heading", { name: "Eval runs" })).toBeVisible();

    // Every suite group repeats the same header row; the sort is page-level,
    // so exercising the first group's header speaks for all of them.
    const when = page.getByRole("columnheader", { name: /When/ }).first();

    await when.getByRole("button").click();
    await expect(page).toHaveURL(/[?&]sort=when(&|$)/);
    await expect(when).toHaveAttribute("aria-sort", "ascending");

    await when.getByRole("button").click();
    await expect(page).toHaveURL(/[?&]sort=-when(&|$)/);
    await expect(when).toHaveAttribute("aria-sort", "descending");

    await when.getByRole("button").click();
    await expect(page).not.toHaveURL(/[?&]sort=/);
    await expect(when).toHaveAttribute("aria-sort", "none");
  });

  test("deep-loads a sort from the URL", async ({ page }) => {
    await page.goto("/runs?sort=-cost");
    await expect(page.getByRole("heading", { name: "Eval runs" })).toBeVisible();
    await expect(
      page.getByRole("columnheader", { name: /Cost/ }).first(),
    ).toHaveAttribute("aria-sort", "descending");
  });

  test("the suite group header links to that set", async ({ page }) => {
    await page.goto("/runs");
    // Every group is named by the set it belongs to; that name is the only
    // route from the stream back up into /sets.
    await page
      .getByRole("link", { name: /checkout-agent\s*\/\s*regression/ })
      .first()
      .click();
    await expect(page).toHaveURL(/\/sets\/checkout-agent\/regression$/);
  });

  test("the whole run row navigates to that run", async ({ page }) => {
    await page.goto("/runs");
    const row = page
      .getByRole("row")
      .filter({ has: page.getByRole("link", { name: MONEY_RUN, exact: true }) });
    // The "When" cell: inside the row, holds no link.
    await row.getByRole("cell").nth(2).click();
    await expect(page).toHaveURL(new RegExp(`/runs/${MONEY_RUN}$`));
  });

  test("selecting and comparing runs never navigates to a run", async ({ page }) => {
    await page.goto("/runs");
    const row = page
      .getByRole("row")
      .filter({ has: page.getByRole("link", { name: MONEY_RUN, exact: true }) });

    // The checkbox owns its click: picking runs to compare must not fire the
    // row's navigation out from under the selection.
    await row.getByRole("checkbox").check();
    await expect(row.getByRole("checkbox")).toBeChecked();
    await expect(page).toHaveURL(/\/runs$/);

    // And the per-row Compare link goes to the comparison, not the run.
    await row.getByRole("link", { name: "Compare" }).click();
    await expect(page).toHaveURL(/\/compare\//);
  });

  test("marks the suite's pinned baseline run with a chip", async ({ page }) => {
    await page.goto("/runs");

    // The default baseline for checkout-agent/regression is the run before the
    // latest (see BASELINE_BY_SUITE in src/mocks/fixtures.ts) — its row carries
    // a "baseline" chip surfaced from the server's suite summary.
    const baselineRow = page
      .getByRole("row")
      .filter({
        has: page.getByRole("link", { name: MONEY_RUN_BASELINE, exact: true }),
      });
    await expect(baselineRow.getByText("baseline")).toBeVisible();
  });

  test("project filter narrows the list and writes the URL param", async ({ page }) => {
    await page.goto("/runs");

    // Runs from every project are present initially.
    await expect(
      page.getByRole("link", { name: SEARCH_RERANK_RUN, exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("link", { name: SUPPORT_BOT_RUN, exact: true }),
    ).toBeVisible();

    // Selecting a project via the filter bar narrows the list...
    await page.getByRole("combobox", { name: "Project" }).selectOption("checkout-agent");

    // ...and is reflected in the URL.
    await expect(page).toHaveURL(/[?&]project=checkout-agent/);

    // Only the selected project's runs remain.
    await expect(
      page.getByRole("link", { name: SEARCH_RERANK_RUN, exact: true }),
    ).toHaveCount(0);
    await expect(
      page.getByRole("link", { name: SUPPORT_BOT_RUN, exact: true }),
    ).toHaveCount(0);
    await expect(
      page.getByRole("link", { name: MONEY_RUN, exact: true }),
    ).toBeVisible();
  });

  test("suite filter narrows within a project", async ({ page }) => {
    await page.goto("/runs?project=checkout-agent");

    // Both regression and smoke suites belong to checkout-agent.
    await expect(page.getByRole("link", { name: MONEY_RUN, exact: true })).toBeVisible();
    await expect(
      page.getByRole("link", { name: SMOKE_RUN, exact: true }),
    ).toBeVisible();

    await page.getByRole("combobox", { name: "Suite" }).selectOption("regression");
    await expect(page).toHaveURL(/[?&]suite=regression/);

    // Smoke runs are gone; regression remains.
    await expect(
      page.getByRole("link", { name: SMOKE_RUN, exact: true }),
    ).toHaveCount(0);
    await expect(page.getByRole("link", { name: MONEY_RUN, exact: true })).toBeVisible();
  });

  test("a URL filter param is reflected in the controls", async ({ page }) => {
    await page.goto("/runs?project=checkout-agent");

    await expect(page.getByRole("combobox", { name: "Project" })).toHaveValue(
      "checkout-agent",
    );
    await expect(
      page.getByRole("link", { name: SEARCH_RERANK_RUN, exact: true }),
    ).toHaveCount(0);
  });
});
