import { expect, test } from "@playwright/test";
import { MONEY_RUN, caseParam } from "./helpers";

test.describe("Run detail", () => {
  test("renders the virtualized cases grid and header stats", async ({ page }) => {
    await page.goto(`/runs/${MONEY_RUN}`);

    // Breadcrumb + run id header.
    await expect(
      page.getByRole("heading", { name: MONEY_RUN, exact: true }),
    ).toBeVisible();
    await expect(page.getByText("Pass rate")).toBeVisible();

    // The grid renders with its assert-label columns and data rows.
    const grid = page.getByRole("grid");
    await expect(grid).toBeVisible();
    await expect(
      page.getByRole("columnheader", { name: "answer_match" }),
    ).toBeVisible();

    const dataRows = grid.getByRole("row").filter({ has: page.getByRole("gridcell") });
    expect(await dataRows.count()).toBeGreaterThan(5);

    // Footer reports the 500-case total.
    await expect(page.getByText(/Showing \d+ of 500\+ cases/)).toBeVisible();
  });

  test("the output search box filters the grid", async ({ page }) => {
    await page.goto(`/runs/${MONEY_RUN}`);
    await expect(page.getByText(/Showing \d+ of 500\+ cases/)).toBeVisible();

    // Search narrows to a single deterministic case and reflects into ?q=.
    await page.getByRole("searchbox", { name: "Search cases" }).fill("case-0000");
    await expect(page).toHaveURL(/[?&]q=case-0000/);

    await expect(page.getByText("Showing 1 of 500+ cases")).toBeVisible();
    await expect(page.getByText("case-0000")).toBeVisible();
    await expect(page.getByText("case-0002")).toHaveCount(0);
  });

  test("clicking a case opens the drawer with prompt, output and assert reasoning; deep-linkable", async ({
    page,
  }) => {
    await page.goto(`/runs/${MONEY_RUN}`);

    // Restrict to passing cases so the opened case definitely has assertions.
    await page.getByRole("button", { name: "Pass", exact: true }).click();
    await expect(page).toHaveURL(/[?&]status=pass/);

    const grid = page.getByRole("grid");
    const firstRow = grid
      .getByRole("row")
      .filter({ has: page.getByRole("gridcell") })
      .first();
    await firstRow.click();

    // Drawer opens and deep-links via ?case=.
    await expect(page).toHaveURL(/[?&]case=case-/);
    const openedCase = caseParam(page);
    expect(openedCase).toBeTruthy();

    const drawer = page.getByRole("dialog");
    await expect(drawer).toBeVisible();

    // Rendered prompt.
    await expect(
      drawer.getByRole("heading", { name: "Rendered prompt" }),
    ).toBeVisible();
    await expect(drawer.getByText(/careful assistant/)).toBeVisible();

    // Output.
    await expect(drawer.getByRole("heading", { name: "Output" })).toBeVisible();
    await expect(drawer.getByText(/"intent"/)).toBeVisible();

    // Per-assert verdict reasoning.
    await expect(
      drawer.getByRole("heading", { name: "Assertions" }),
    ).toBeVisible();
    await expect(drawer.getByText(/satisfied/).first()).toBeVisible();

    // Deep-linkability: reloading the ?case= URL re-opens the same drawer.
    await page.reload();
    const reopened = page.getByRole("dialog");
    await expect(reopened).toBeVisible();
    await expect(
      reopened.getByRole("heading", { name: "Rendered prompt" }),
    ).toBeVisible();
    expect(caseParam(page)).toBe(openedCase);
  });
});
