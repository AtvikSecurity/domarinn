import { expect, test } from "@playwright/test";
import { MONEY_RUN, deltaParam } from "./helpers";

test.describe("Compare view", () => {
  test("shows summary chips, filters the delta grid, and expands to a side-by-side diff", async ({
    page,
  }) => {
    await page.goto(`/runs/${MONEY_RUN}/compare`);

    await expect(page.getByRole("heading", { name: "Compare" })).toBeVisible();

    // Each chip's accessible name is exactly "<label> <count>". Delta-grid rows
    // echo the label inside a longer name, so an anchored regex targets the chip
    // (e.g. "Newly failing 54") without matching a row.
    const chip = (label: string) =>
      page.getByRole("button", { name: new RegExp(`^${label} \\d+$`) });

    const outputChanged = chip("Output changed");
    await expect(outputChanged).toBeVisible();
    await expect(chip("Newly failing")).toBeVisible();
    await expect(chip("Newly passing")).toBeVisible();
    await expect(chip("Still failing")).toBeVisible();

    // The featured comparison always has output-changed cases.
    const count = Number((await outputChanged.innerText()).replace(/\D/g, ""));
    expect(count).toBeGreaterThan(0);

    // Clicking a chip filters the grid (URL + rows).
    await outputChanged.click();
    await expect(page).toHaveURL(/[?&]delta=output_changed/);
    expect(deltaParam(page)).toBe("output_changed");
    await expect(page.getByText("No cases in this delta group")).toHaveCount(0);

    // Every filtered row is flagged "changed" in the Output column.
    await expect(page.getByText("changed").first()).toBeVisible();

    // Expanding a row reveals the side-by-side / word diff. Only row buttons
    // carry the aria-expanded attribute (chips use aria-pressed), so this
    // attribute selector targets a row precisely.
    const firstRow = page.locator('button[aria-expanded="false"]').first();
    await firstRow.click();
    await expect(page.locator('button[aria-expanded="true"]')).toHaveCount(1);

    // The diff renders "Base"/"Head" columns in addition to the delta-table
    // column headers, so each label now appears twice.
    await expect(page.getByText(/^Base$/)).toHaveCount(2);
    await expect(page.getByText(/^Head$/)).toHaveCount(2);
    // Both diff columns are <pre> blocks.
    expect(await page.locator("pre").count()).toBeGreaterThanOrEqual(2);
  });

  test("base and head selectors are populated with the suite's runs", async ({ page }) => {
    await page.goto(`/runs/${MONEY_RUN}/compare`);
    await expect(page.getByRole("heading", { name: "Compare" })).toBeVisible();

    // The base selector lists the 12 regression runs and defaults to the
    // baseline (the previous regression run).
    const baseSelect = page.getByRole("combobox", { name: "Base run" });
    await expect(baseSelect).toBeVisible();
    await expect(baseSelect).toHaveValue("checkout-agent-regression-11");
    await expect(baseSelect.locator("option")).toHaveCount(12);

    // The head selector defaults to the run in the URL.
    const headSelect = page.getByRole("combobox", { name: "Head run" });
    await expect(headSelect).toBeVisible();
    await expect(headSelect).toHaveValue(MONEY_RUN);
    await expect(headSelect.locator("option")).toHaveCount(12);
  });

  test("picking a different head (and base) recomputes the comparison", async ({ page }) => {
    await page.goto(`/runs/${MONEY_RUN}/compare`);
    await expect(page.getByRole("heading", { name: "Compare" })).toBeVisible();

    // Re-point the head run: the route + selection follow.
    const headSelect = page.getByRole("combobox", { name: "Head run" });
    await headSelect.selectOption("checkout-agent-regression-10");
    await expect(page).toHaveURL(
      /\/runs\/checkout-agent-regression-10\/compare\/checkout-agent-regression-11/,
    );
    await expect(headSelect).toHaveValue("checkout-agent-regression-10");

    // Re-point the base run independently; the head stays put.
    const baseSelect = page.getByRole("combobox", { name: "Base run" });
    await baseSelect.selectOption("checkout-agent-regression-09");
    await expect(page).toHaveURL(
      /\/runs\/checkout-agent-regression-10\/compare\/checkout-agent-regression-09/,
    );
    await expect(baseSelect).toHaveValue("checkout-agent-regression-09");
    await expect(headSelect).toHaveValue("checkout-agent-regression-10");

    // The delta grid still renders with summary chips for the new pair.
    await expect(
      page.getByRole("button", { name: /^Newly failing \d+$/ }),
    ).toBeVisible();
  });
});
