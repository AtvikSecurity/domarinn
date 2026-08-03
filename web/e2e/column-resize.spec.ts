import { expect, test, type Page } from "@playwright/test";
import { MONEY_RUN } from "./helpers";

/**
 * The runs list renders one table per suite group, all sharing a single
 * preference, so a column id is not unique on the page. The first instance is
 * representative — that they move together is the point.
 */
const resizeHandle = (page: Page, columnId: string) =>
  page.locator(`[data-column-resizer="${columnId}"]`).first();

/** Width of the header cell owning a column, as laid out. */
async function headerWidth(page: Page, columnId: string): Promise<number> {
  const box = await resizeHandle(page, columnId).locator("xpath=..").boundingBox();
  return box?.width ?? 0;
}

async function dragHandle(page: Page, columnId: string, dx: number) {
  const handle = resizeHandle(page, columnId);
  const box = await handle.boundingBox();
  if (!box) throw new Error(`no resize handle for ${columnId}`);
  const y = box.y + box.height / 2;
  await page.mouse.move(box.x + box.width / 2, y);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2 + dx, y, { steps: 8 });
  await page.mouse.up();
}

test.describe("Column resizing", () => {
  test("a drag widens the column and the width survives a reload", async ({
    page,
  }) => {
    await page.goto(`/runs/${MONEY_RUN}`);
    await expect(page.getByRole("grid")).toBeVisible();

    const before = await headerWidth(page, "tokens");
    await dragHandle(page, "tokens", 80);
    await expect
      .poll(() => headerWidth(page, "tokens"))
      .toBeGreaterThan(before + 40);

    const widened = await headerWidth(page, "tokens");

    // A viewing habit, not part of what a shared link means.
    expect(page.url()).not.toContain("col");

    await page.reload();
    await expect(page.getByRole("grid")).toBeVisible();
    await expect.poll(() => headerWidth(page, "tokens")).toBeCloseTo(widened, 0);
  });

  // The virtualizer estimates every row at exactly 44px with no measurement,
  // so a cell that wrapped when narrowed would desync the scrollbar from the
  // content it is scrolling.
  test("narrowing a column never makes a row taller", async ({ page }) => {
    await page.goto(`/runs/${MONEY_RUN}`);
    await expect(page.getByRole("grid")).toBeVisible();

    await dragHandle(page, "name", -400);

    const row = page
      .getByRole("row")
      .filter({ hasNot: page.getByRole("columnheader") })
      .first();
    await expect.poll(async () => (await row.boundingBox())?.height).toBe(44);
  });

  test("the handle resizes without triggering the column's sort", async ({
    page,
  }) => {
    await page.goto(`/runs/${MONEY_RUN}`);
    await expect(page.getByRole("grid")).toBeVisible();

    const urlBefore = page.url();
    await dragHandle(page, "tokens", 60);

    // Sorting is encoded in the URL, so an unchanged URL proves the drag did
    // not bubble into the sort button sharing the header cell.
    expect(page.url()).toBe(urlBefore);
  });

  test("the handle is operable from the keyboard", async ({ page }) => {
    await page.goto(`/runs/${MONEY_RUN}`);
    await expect(page.getByRole("grid")).toBeVisible();

    const before = await headerWidth(page, "tokens");
    const slider = resizeHandle(page, "tokens").getByRole("slider");
    await slider.focus();
    for (let i = 0; i < 6; i++) await page.keyboard.press("ArrowRight");

    await expect.poll(() => headerWidth(page, "tokens")).toBeGreaterThan(before);

    // Home returns the column to the layout's own track.
    await page.keyboard.press("Home");
    await expect.poll(() => headerWidth(page, "tokens")).toBeCloseTo(before, 0);
  });

  test("double-clicking the handle resets that column", async ({ page }) => {
    await page.goto(`/runs/${MONEY_RUN}`);
    await expect(page.getByRole("grid")).toBeVisible();

    const before = await headerWidth(page, "tokens");
    await dragHandle(page, "tokens", 90);
    await expect
      .poll(() => headerWidth(page, "tokens"))
      .toBeGreaterThan(before + 40);

    await resizeHandle(page, "tokens").dblclick();
    await expect.poll(() => headerWidth(page, "tokens")).toBeCloseTo(before, 0);
  });
});

test.describe("Columns on a real <table>", () => {
  test("the runs list hides a column and its cells together", async ({
    page,
  }) => {
    await page.goto("/runs?cached=all");
    await expect(
      page.getByRole("columnheader", { name: "Tokens" }).first(),
    ).toBeVisible();

    await page.getByRole("button", { name: /Columns/ }).click();
    await page.getByRole("checkbox", { name: "Tokens" }).uncheck();

    // Header and cells go together — with a <colgroup> in play a stray cell
    // would shift every column after it out of alignment.
    await expect(page.getByRole("columnheader", { name: "Tokens" })).toHaveCount(
      0,
    );
    const headers = await page.getByRole("columnheader").count();
    const firstRowCells = await page
      .getByRole("row")
      .filter({ hasNot: page.getByRole("columnheader") })
      .first()
      .getByRole("cell")
      .count();
    const groups = await page.getByRole("table").count();
    expect(headers).toBe(firstRowCells * groups);
  });

  test("a runs column resizes and the width persists", async ({ page }) => {
    await page.goto("/runs?cached=all");
    const before = await headerWidth(page, "when");
    await dragHandle(page, "when", 70);
    await expect.poll(() => headerWidth(page, "when")).toBeGreaterThan(before);

    const widened = await headerWidth(page, "when");
    await page.reload();
    await expect.poll(() => headerWidth(page, "when")).toBeCloseTo(widened, 0);
  });
});

test.describe("Column preference migration", () => {
  // Users who had configured the case grid before the store was generalized
  // must not silently lose their columns on upgrade.
  test("adopts the pre-generalization storage key", async ({ page }) => {
    await page.addInitScript(() => {
      localStorage.setItem(
        "domarinn.grid.columns",
        JSON.stringify({ tokens: false }),
      );
    });
    await page.goto(`/runs/${MONEY_RUN}`);
    await expect(page.getByRole("grid")).toBeVisible();

    // Tokens is visible by default, so its absence is the migration working
    // rather than a default being observed.
    await expect(
      page.getByRole("columnheader", { name: /Tokens/ }),
    ).toHaveCount(0);
    // The picker agrees, and offers it back.
    await page.getByRole("button", { name: /Columns/ }).click();
    await expect(page.getByRole("checkbox", { name: "Tokens" })).not.toBeChecked();

    // The legacy key is left in place, so a rollback keeps the setting.
    expect(
      await page.evaluate(() => localStorage.getItem("domarinn.grid.columns")),
    ).not.toBeNull();
  });
});
