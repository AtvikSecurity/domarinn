import { expect, test } from "@playwright/test";
import { MONEY_RUN } from "./helpers";

// Latest run of each other suite (see src/mocks/fixtures.ts run counts).
const SEARCH_RERANK_RUN = "search-rerank-ndcg-eval-10";
const SUPPORT_BOT_RUN = "support-bot-tone-and-safety-09";
const SMOKE_RUN = "checkout-agent-smoke-08";

test.describe("Runs list", () => {
  test("renders suites grouped with pass-rate, and lists runs", async ({ page }) => {
    await page.goto("/");

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

  test("project filter narrows the list and writes the URL param", async ({ page }) => {
    await page.goto("/");

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
    await page.goto("/?project=checkout-agent");

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
    await page.goto("/?project=checkout-agent");

    await expect(page.getByRole("combobox", { name: "Project" })).toHaveValue(
      "checkout-agent",
    );
    await expect(
      page.getByRole("link", { name: SEARCH_RERANK_RUN, exact: true }),
    ).toHaveCount(0);
  });
});
