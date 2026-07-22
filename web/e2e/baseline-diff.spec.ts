import { expect, test } from "@playwright/test";
import { MONEY_RUN, MONEY_RUN_BASELINE, OUTPUT_CHANGED_CASE } from "./helpers";

test.describe("Case drawer baseline diff", () => {
  test("expands, renders a two-sided diff, switches mode, and deep-links to full compare", async ({
    page,
  }) => {
    // Open the drawer directly on a case whose output differs from the same
    // case in the suite's pinned baseline run (see OUTPUT_CHANGED_CASE).
    await page.goto(`/runs/${MONEY_RUN}?case=${OUTPUT_CHANGED_CASE}`);

    const drawer = page.getByRole("dialog");
    await expect(drawer).toBeVisible();

    // The section header names the (short-form) baseline run and starts collapsed.
    const toggle = drawer.getByRole("button", { name: /Diff vs baseline/ });
    await expect(toggle).toBeVisible();
    await expect(toggle).toHaveText(new RegExp(MONEY_RUN_BASELINE));
    await expect(toggle).toHaveAttribute("aria-expanded", "false");

    // Expand -> the baseline case is fetched and the side-by-side diff renders.
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-expanded", "true");

    const sidePane = drawer.locator('[data-diff-mode="side"]');
    await expect(sidePane).toBeVisible();
    // Both diff columns render, and the change is coloured (a struck-through
    // removed segment lives in the Base column).
    await expect(drawer.getByText(/^Base$/)).toBeVisible();
    await expect(drawer.getByText(/^Head$/)).toBeVisible();
    await expect(sidePane.locator(".line-through").first()).toBeVisible();

    // Switch to the unified diff via the segmented control.
    await drawer.getByRole("radio", { name: "Unified" }).click();
    await expect(drawer.locator('[data-diff-mode="lines"]')).toBeVisible();
    await expect(drawer.locator('[data-diff-mode="side"]')).toHaveCount(0);

    // The footer link opens the full compare view: baseline as base, current as
    // head, with the same case pre-selected.
    await drawer.getByRole("link", { name: /Open full compare/ }).click();
    await expect(page).toHaveURL(
      new RegExp(
        `/runs/${MONEY_RUN_BASELINE}/compare/${MONEY_RUN}\\?case=${OUTPUT_CHANGED_CASE}`,
      ),
    );
    await expect(page.getByRole("heading", { name: "Compare" })).toBeVisible();
    // The compare page auto-expands the deep-linked row.
    await expect(page.locator('button[aria-expanded="true"]')).toHaveCount(1);
  });

  test("the section is hidden when viewing the baseline run itself", async ({
    page,
  }) => {
    // MONEY_RUN_BASELINE is its suite's pinned baseline, so a case opened on it
    // has nothing to diff against — the section must not appear.
    await page.goto(`/runs/${MONEY_RUN_BASELINE}?case=${OUTPUT_CHANGED_CASE}`);

    const drawer = page.getByRole("dialog");
    await expect(drawer).toBeVisible();
    // The Output section confirms the drawer body loaded before we assert absence.
    await expect(drawer.getByText("Output", { exact: true })).toBeVisible();
    await expect(drawer.getByText(/Diff vs baseline/)).toHaveCount(0);
  });
});
