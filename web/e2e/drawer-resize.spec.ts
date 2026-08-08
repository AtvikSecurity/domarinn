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
  // Scoped and filtered to data rows, matching run-detail.spec.ts: an
  // unscoped index breaks the moment another table or a header row appears,
  // and this page now also renders the error breakdown.
  await page.getByRole("row").filter({ has: page.getByRole("gridcell") }).first().click();
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
    await page
      .getByRole("row")
      .filter({ has: page.getByRole("gridcell") })
      .nth(1)
      .click();
    await expect(page.getByRole("dialog")).toBeVisible();

    expect(await drawerWidth(page)).toBeCloseTo(expanded, 0);
  });

  test("the visible grip sits on the leading edge and the drawer retracts before unmount", async ({
    page,
  }) => {
    await openDrawer(page);
    const dialog = page.getByRole("dialog");
    const grip = page.getByTestId("drawer-resize-grip");

    await expect(grip).toBeVisible();
    // Both rects in one frame. Two `boundingBox()` round-trips can straddle a
    // frame of the 160ms slide-in and disagree by most of the drawer's travel,
    // which fails on timing rather than on placement. The grip rides inside the
    // panel, so their offset is what this is actually about and it holds at
    // every frame of the animation.
    const offsets = await page.evaluate(() => {
      const panel = document.querySelector<HTMLElement>('[role="dialog"]');
      const tab = document.querySelector<HTMLElement>('[data-testid="drawer-resize-grip"]');
      if (!panel || !tab) return null;
      const p = panel.getBoundingClientRect();
      const t = tab.getBoundingClientRect();
      return {
        fromLeadingEdge: Math.abs(t.right - p.left),
        fromCentre: Math.abs(t.top + t.height / 2 - (p.top + p.height / 2)),
      };
    });
    expect(offsets).not.toBeNull();
    expect(offsets!.fromLeadingEdge).toBeLessThanOrEqual(2);
    expect(offsets!.fromCentre).toBeLessThanOrEqual(2);

    await page.getByRole("button", { name: "Close case drawer" }).click();
    await expect(dialog).toHaveAttribute("data-state", "closed");
    expect(await dialog.evaluate((el) => getComputedStyle(el).animationName)).toContain(
      "drawer-out",
    );
    await expect(dialog).toHaveCount(0);
  });
});
