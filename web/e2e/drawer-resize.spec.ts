import { expect, test } from "@playwright/test";
import { MONEY_RUN } from "./helpers";

/**
 * The case drawer is resizable.
 *
 * A fixed 44rem panel is wrong in both directions: too narrow for a long
 * rendered prompt or a side-by-side diff, too wide when you want to glance at a
 * verdict while keeping the grid visible.
 */

async function openDrawer(page: import("@playwright/test").Page) {
  await page.goto(`/runs/${MONEY_RUN}`);
  await page.getByRole("row").nth(1).click();
  await expect(page.getByRole("dialog")).toBeVisible();
}

const drawerWidth = (page: import("@playwright/test").Page) =>
  page.getByRole("dialog").evaluate((el) => el.getBoundingClientRect().width);

test.describe("Case drawer resizing", () => {
  test("the expand button widens it and collapses it back", async ({ page }) => {
    await openDrawer(page);
    const initial = await drawerWidth(page);

    await page.getByRole("button", { name: "Toggle panel width" }).click();
    const expanded = await drawerWidth(page);
    expect(expanded).toBeGreaterThan(initial);

    await page.getByRole("button", { name: "Toggle panel width" }).click();
    expect(await drawerWidth(page)).toBeCloseTo(initial, 0);
  });

  // Drag-only would put the width out of reach for anyone not using a mouse,
  // so the handle is a focusable separator following the WAI-ARIA splitter
  // pattern.
  test("the resize handle is keyboard operable", async ({ page }) => {
    await openDrawer(page);
    const initial = await drawerWidth(page);

    const handle = page.getByRole("separator", { name: "Resize panel" });
    await expect(handle).toBeVisible();
    await handle.focus();
    // Left grows it: the drawer is right-anchored, so its edge moves left as
    // it widens.
    await page.keyboard.press("ArrowLeft");
    await page.keyboard.press("ArrowLeft");

    expect(await drawerWidth(page)).toBeGreaterThan(initial);
  });

  test("the chosen width persists to the next case", async ({ page }) => {
    await openDrawer(page);
    await page.getByRole("button", { name: "Toggle panel width" }).click();
    const expanded = await drawerWidth(page);

    // Close, open a different case: resizing on every case would be worse than
    // not having the control at all.
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await page.getByRole("row").nth(2).click();
    await expect(page.getByRole("dialog")).toBeVisible();

    expect(await drawerWidth(page)).toBeCloseTo(expanded, 0);
  });
});
