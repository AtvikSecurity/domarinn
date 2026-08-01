import { expect, test } from "@playwright/test";
import { MONEY_RUN } from "./helpers";

/** iPhone-class viewport. The narrowest thing this UI claims to support. */
const WIDTH = 390;

test.use({ viewport: { width: WIDTH, height: 844 } });

/**
 * Phone-width navigation.
 *
 * Every other spec runs at 1280×800, so nothing here was covered before: the
 * header once pushed the document to 508px at this width — the one place the
 * page body itself scrolled sideways — and no assertion would have caught it
 * coming back.
 */
test.describe("Mobile navigation", () => {
  for (const [label, path] of [
    ["overview", "/"],
    ["runs list", "/runs"],
    ["run detail", `/runs/${MONEY_RUN}`],
    ["sets", "/sets"],
    ["set project", "/sets/checkout-agent"],
    ["set suite", "/sets/checkout-agent/regression"],
    ["cache entries", "/cache/entries"],
  ] as const) {
    test(`${label}: the page body never scrolls sideways`, async ({ page }) => {
      await page.goto(path);
      await expect(page.getByRole("banner")).toBeVisible();

      // The document, not a scrollport inside it. Grids and tables are allowed
      // their own `overflow-x: auto`; the page is not.
      const documentWidth = await page.evaluate(() =>
        Math.max(
          document.documentElement.scrollWidth,
          document.body.scrollWidth,
        ),
      );
      expect(documentWidth).toBeLessThanOrEqual(WIDTH);
    });
  }
});
