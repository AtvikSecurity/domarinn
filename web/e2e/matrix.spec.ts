import { expect, test } from "@playwright/test";
import { MATRIX_RUN, MONEY_RUN, caseParam } from "./helpers";

// Deterministic cells in the matrix fixture (search-rerank/ndcg-eval-10), pinned
// by src/mocks/fixtures.test.ts. Columns are provider-major
// [gpt-5-mini, claude-sonnet, llama-70b] × [baseline, cot-v2]; `data-cell` keys a
// tile by `${test_id}:${original column index}`.
const CELL_1_OF_2 = "test-000:2"; // claude-sonnet · baseline → 1/2
const CELL_TWO_PROVIDER = "test-002:0"; // gpt-5-mini · baseline; only 2 of 3
// providers in that (test, baseline) group produced output → diff is offered.

test.describe("Matrix view (prompt × provider)", () => {
  test("toggles List → Matrix and writes ?view=matrix", async ({ page }) => {
    await page.goto(`/runs/${MATRIX_RUN}`);
    await expect(page.getByText("Showing 144 cases")).toBeVisible();

    const view = page.getByRole("radiogroup", { name: "View" });
    await expect(view).toBeVisible();
    await view.getByRole("radio", { name: "Matrix" }).click();

    await expect(page).toHaveURL(/[?&]view=matrix/);
    await expect(
      page.getByRole("table", { name: "Prompt by provider matrix" }),
    ).toBeVisible();
  });

  test("shows provider + prompt headers and pass@k cells", async ({ page }) => {
    await page.goto(`/runs/${MATRIX_RUN}?view=matrix`);

    const table = page.getByRole("table", { name: "Prompt by provider matrix" });
    await expect(table).toBeVisible();

    // Prompt-section headers span their providers.
    await expect(page.getByRole("columnheader", { name: "baseline" })).toBeVisible();
    await expect(page.getByRole("columnheader", { name: "cot-v2" })).toBeVisible();
    // Provider header cells (one per (provider, prompt) column).
    await expect(
      page.getByRole("columnheader", { name: "gpt-5-mini" }).first(),
    ).toBeVisible();
    await expect(
      page.getByRole("columnheader", { name: "claude-sonnet" }).first(),
    ).toBeVisible();

    // A repeated cell renders `passed/total`.
    await expect(page.locator(`[data-cell="${CELL_1_OF_2}"]`)).toContainText("1/2");

    // The list-only status chips + search are hidden in matrix mode.
    await expect(page.getByRole("group", { name: "Status" })).toHaveCount(0);
    await expect(page.getByRole("searchbox", { name: "Search cases" })).toHaveCount(0);
  });

  test("a cell popover deep-links a repeat into the case drawer, keeping ?view=matrix", async ({
    page,
  }) => {
    await page.goto(`/runs/${MATRIX_RUN}?view=matrix`);
    await expect(
      page.getByRole("table", { name: "Prompt by provider matrix" }),
    ).toBeVisible();

    await page.locator(`[data-cell="${CELL_1_OF_2}"]`).click();

    // Popover lists one row per repeat.
    const repeat0 = page.getByRole("button", { name: /#0/ });
    await expect(repeat0).toBeVisible();
    await expect(page.getByRole("button", { name: /#1/ })).toBeVisible();

    await repeat0.click();

    // Drawer opens; both ?case= and ?view=matrix are in the URL.
    const drawer = page.getByRole("dialog");
    await expect(drawer).toBeVisible();
    await expect(drawer.getByRole("heading", { name: "Output" })).toBeVisible();
    await expect(page).toHaveURL(/[?&]view=matrix/);
    await expect(page).toHaveURL(/[?&]case=case-/);
    expect(caseParam(page)).toBeTruthy();
  });

  test("Compare across providers opens a modal with ≥2 provider panels", async ({
    page,
  }) => {
    await page.goto(`/runs/${MATRIX_RUN}?view=matrix`);
    await expect(
      page.getByRole("table", { name: "Prompt by provider matrix" }),
    ).toBeVisible();

    await page.locator(`[data-cell="${CELL_1_OF_2}"]`).click();
    await page.getByRole("button", { name: "Compare across providers" }).click();

    const modal = page.getByRole("dialog");
    await expect(modal).toBeVisible();
    // One panel per provider that ran the test (fixture: 3).
    await expect(modal.getByText("gpt-5-mini")).toBeVisible();
    await expect(modal.getByText("llama-70b")).toBeVisible();
  });

  test("a two-provider comparison offers a word-diff toggle", async ({ page }) => {
    await page.goto(`/runs/${MATRIX_RUN}?view=matrix`);
    await expect(
      page.getByRole("table", { name: "Prompt by provider matrix" }),
    ).toBeVisible();

    // This (test, baseline) group has exactly two providers with output.
    await page.locator(`[data-cell="${CELL_TWO_PROVIDER}"]`).click();
    await page.getByRole("button", { name: "Compare across providers" }).click();

    const modal = page.getByRole("dialog");
    await expect(modal).toBeVisible();

    const toggle = modal.getByRole("radiogroup", { name: "Compare view" });
    await expect(toggle).toBeVisible();
    await toggle.getByRole("radio", { name: "Diff" }).click();

    // The side-by-side word diff renders.
    await expect(modal.locator("[data-diff-mode]")).toBeVisible();
  });
});

test.describe("Single-provider run shows no matrix toggle", () => {
  test("MONEY_RUN never offers the List | Matrix toggle", async ({ page }) => {
    await page.goto(`/runs/${MONEY_RUN}`);
    await expect(page.getByText(/Showing first \d+ of 500\+ cases/)).toBeVisible();
    await expect(page.getByRole("radiogroup", { name: "View" })).toHaveCount(0);

    // Even a deep-linked ?view=matrix silently falls back to the list.
    await page.goto(`/runs/${MONEY_RUN}?view=matrix`);
    await expect(page.getByText(/Showing first \d+ of 500\+ cases/)).toBeVisible();
    await expect(
      page.getByRole("table", { name: "Prompt by provider matrix" }),
    ).toHaveCount(0);
  });
});
