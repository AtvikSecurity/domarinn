import { expect, test } from "@playwright/test";
import { MATRIX_RUN, MONEY_RUN } from "./helpers";

/**
 * The app shell owns the viewport, so exactly one thing scrolls at a time.
 *
 * Run detail is the reason this matters: the case grid must scroll
 * horizontally, and `overflow-x: auto` forces the other axis into a scrollport
 * too, so the grid is unavoidably its own vertical scroller. When the page
 * scrolled as well, the wheel went to whichever scroller was under the pointer
 * and the row count below the grid could only be reached by first moving the
 * pointer off it.
 */
test.describe("App shell scrolling", () => {
  for (const [label, path] of [
    ["run detail", `/runs/${MONEY_RUN}`],
    ["matrix", `/runs/${MATRIX_RUN}?view=matrix`],
    ["runs list", "/"],
  ] as const) {
    test(`${label}: the header stays put and only one element scrolls`, async ({
      page,
    }) => {
      await page.goto(path);
      // `banner`, not `locator("header")`: suite cards have their own
      // `<header>`, and only the outermost one is the landmark.
      const appHeader = page.getByRole("banner");
      await expect(appHeader).toBeVisible();

      const headerTop = async () =>
        appHeader.evaluate((el) => el.getBoundingClientRect().top);
      expect(await headerTop()).toBe(0);

      await page.mouse.move(720, 500);
      await page.mouse.wheel(0, 600);
      await page.waitForTimeout(300);

      // The document never scrolls: the chrome cannot be dragged off-screen.
      expect(await headerTop()).toBe(0);

      const scrolled = await page.evaluate(
        () => [...document.querySelectorAll("*")].filter((el) => el.scrollTop > 0).length,
      );
      expect(scrolled).toBeLessThanOrEqual(1);
    });
  }

  test("run detail: the grid scrolls, not the page body", async ({ page }) => {
    await page.goto(`/runs/${MONEY_RUN}`);
    await expect(page.getByRole("grid")).toBeVisible();

    await page.mouse.move(720, 600);
    await page.mouse.wheel(0, 600);
    await page.waitForTimeout(300);

    const { gridScrolled, mainScrolled } = await page.evaluate(() => ({
      gridScrolled: (document.querySelector('[role="grid"]')?.scrollTop ?? 0) > 0,
      mainScrolled: (document.querySelector("main")?.scrollTop ?? 0) > 0,
    }));
    expect(gridScrolled).toBe(true);
    expect(mainScrolled).toBe(false);
  });
});
