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

  test("the header strip is replaced by a menu", async ({ page }) => {
    await page.goto("/runs");
    // A scrolling strip with no scroll affordance silently hid API keys,
    // Admin and Settings off the right edge.
    await expect(
      page.getByRole("navigation", { name: "Primary" }),
    ).not.toBeVisible();
    await expect(page.getByRole("button", { name: "Open menu" })).toBeVisible();
  });

  test("the menu navigates and closes behind itself", async ({ page }) => {
    await page.goto("/runs");
    await page.getByRole("button", { name: "Open menu" }).click();

    const menu = page.getByRole("navigation", { name: "Main menu" });
    // Every destination, including the ones the strip used to hide.
    for (const label of ["Overview", "Runs", "Sets", "Cache", "Settings"]) {
      await expect(menu.getByRole("link", { name: label })).toBeVisible();
    }

    await menu.getByRole("link", { name: "Sets" }).click();
    await expect(page).toHaveURL(/\/sets$/);
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });

  test("Escape closes the menu and returns focus to the trigger", async ({
    page,
  }) => {
    await page.goto("/runs");
    await page.getByRole("button", { name: "Open menu" }).click();
    await expect(page.getByRole("dialog")).toBeVisible();

    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect(
      await page.evaluate(() =>
        document.activeElement?.getAttribute("aria-label"),
      ),
    ).toBe("Open menu");
  });

  test("the menu carries the search that the header cannot show here", async ({
    page,
  }) => {
    await page.goto("/runs");
    // The header bar is `hidden md:flex`, so at this width there was no way to
    // search at all until the sheet carried one.
    await page.getByRole("button", { name: "Open menu" }).click();

    const input = page.getByRole("combobox", {
      name: "Search sets, runs and cases",
    });
    await input.fill("checkout");
    await page.locator('[data-search-hit="set"]').first().click();

    await expect(page).toHaveURL(/\/sets\/checkout-agent$/);
    // Navigating must take the sheet with it.
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });

  test("Escape dismisses the suggestions before the menu", async ({ page }) => {
    await page.goto("/runs");
    await page.getByRole("button", { name: "Open menu" }).click();
    const input = page.getByRole("combobox", {
      name: "Search sets, runs and cases",
    });
    await input.fill("checkout");
    // By data attribute, not the "Sets" label — inside the sheet that string
    // is also a nav link.
    const suggestions = page.locator("[data-search-hit]");
    await expect(suggestions.first()).toBeVisible();

    // One press, one dismissal: the sheet must survive closing the dropdown.
    await input.press("Escape");
    await expect(suggestions).toHaveCount(0);
    await expect(page.getByRole("dialog")).toBeVisible();

    // The next one is the sheet's.
    await input.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });

  test("an anonymous visitor in closed mode gets no menu button", async ({
    page,
  }) => {
    // The only reachable page is /login, so a menu would be entirely dead
    // links. The header stays bare, as it does at desktop widths.
    await page.addInitScript(() => {
      try {
        localStorage.setItem("domarinn.mock.authmode", "closed");
      } catch {
        /* ignore */
      }
    });
    await page.goto("/");
    await expect(page).toHaveURL(/\/login$/);
    await expect(page.getByRole("button", { name: "Open menu" })).toHaveCount(0);
  });

  test("opening the menu does not shift the header", async ({ page }) => {
    // Radix locks body scroll and compensates for a vanishing scrollbar with
    // padding; the shell already owns overflow, so there is nothing to
    // compensate for and the chrome must not move.
    await page.goto("/runs");
    // Measured through the DOM rather than by role: an open Radix dialog
    // makes the rest of the tree inert, so the banner role is — correctly —
    // no longer exposed while the sheet is up. `querySelector` takes the
    // first header in document order, which is the app's own.
    const headerTop = () =>
      page.evaluate(
        () => document.querySelector("header")!.getBoundingClientRect().top,
      );

    const before = await headerTop();
    await page.getByRole("button", { name: "Open menu" }).click();
    await expect(page.getByRole("dialog")).toBeVisible();
    expect(await headerTop()).toBe(before);
  });
});
