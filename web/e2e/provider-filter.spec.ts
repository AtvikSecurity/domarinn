import { expect, test } from "@playwright/test";
import { MATRIX_RUN, MONEY_RUN } from "./helpers";

test.describe("Provider / prompt filters + columns (matrix-shaped run)", () => {
  test("shows provider + prompt chips and grid columns", async ({ page }) => {
    await page.goto(`/runs/${MATRIX_RUN}`);
    await expect(page.getByText("Showing 144 cases")).toBeVisible();

    // Chip groups (scoped by their accessible group label) render, each with the
    // fixture's distinct values.
    const providerChips = page.getByRole("group", { name: "Provider" });
    await expect(providerChips).toBeVisible();
    for (const p of ["gpt-5-mini", "claude-sonnet", "llama-70b"]) {
      await expect(providerChips.getByRole("button", { name: p })).toBeVisible();
    }
    const promptChips = page.getByRole("group", { name: "Prompt" });
    await expect(promptChips).toBeVisible();
    for (const p of ["baseline", "cot-v2"]) {
      await expect(promptChips.getByRole("button", { name: p })).toBeVisible();
    }

    // The grid gains provider + prompt columns.
    await expect(page.getByRole("columnheader", { name: /Provider/ })).toBeVisible();
    await expect(page.getByRole("columnheader", { name: /Prompt/ })).toBeVisible();
  });

  test("clicking a provider chip filters the grid and writes ?provider=", async ({
    page,
  }) => {
    await page.goto(`/runs/${MATRIX_RUN}`);
    await expect(page.getByText("Showing 144 cases")).toBeVisible();

    await page
      .getByRole("group", { name: "Provider" })
      .getByRole("button", { name: "gpt-5-mini" })
      .click();

    // Server filter -> URL + a smaller, deterministic result set (48 = 2 prompts
    // × 12 tests × 2 repeats).
    await expect(page).toHaveURL(/[?&]provider=gpt-5-mini/);
    await expect(page.getByText("Showing 48 of 144+ cases")).toBeVisible();

    // Reset via the group's "All" chip restores the full list.
    await page
      .getByRole("group", { name: "Provider" })
      .getByRole("button", { name: "All" })
      .click();
    await expect(page).not.toHaveURL(/[?&]provider=/);
    await expect(page.getByText("Showing 144 cases")).toBeVisible();
  });

  test("clicking a prompt chip filters the grid and writes ?prompt=", async ({
    page,
  }) => {
    await page.goto(`/runs/${MATRIX_RUN}`);
    await expect(page.getByText("Showing 144 cases")).toBeVisible();

    await page
      .getByRole("group", { name: "Prompt" })
      .getByRole("button", { name: "baseline" })
      .click();

    // 72 = 3 providers × 12 tests × 2 repeats.
    await expect(page).toHaveURL(/[?&]prompt=baseline/);
    await expect(page.getByText("Showing 72 of 144+ cases")).toBeVisible();
  });

  test("deep-loads a provider filter from the URL", async ({ page }) => {
    await page.goto(`/runs/${MATRIX_RUN}?provider=llama-70b`);
    await expect(page.getByText("Showing 48 of 144+ cases")).toBeVisible();
    // The active chip reflects the URL.
    await expect(
      page.getByRole("group", { name: "Provider" }).getByRole("button", { name: "llama-70b" }),
    ).toBeVisible();
  });
});

test.describe("Single-provider run (MONEY_RUN) shows no matrix affordances", () => {
  test("renders neither provider/prompt chips nor columns", async ({ page }) => {
    await page.goto(`/runs/${MONEY_RUN}`);
    await expect(page.getByText(/Showing \d+ of 500\+ cases/)).toBeVisible();

    await expect(page.getByRole("group", { name: "Provider" })).toHaveCount(0);
    await expect(page.getByRole("group", { name: "Prompt" })).toHaveCount(0);
    await expect(page.getByRole("columnheader", { name: /Provider/ })).toHaveCount(0);
    await expect(page.getByRole("columnheader", { name: /Prompt/ })).toHaveCount(0);
  });
});
