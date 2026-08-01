import { expect, test } from "@playwright/test";
import { MONEY_RUN, caseParam } from "./helpers";

test.describe("Run detail", () => {
  test("renders the virtualized cases grid and header stats", async ({ page }) => {
    await page.goto(`/runs/${MONEY_RUN}`);

    // Breadcrumb + run id header.
    await expect(
      page.getByRole("heading", { name: MONEY_RUN, exact: true }),
    ).toBeVisible();
    await expect(page.getByText("Pass rate")).toBeVisible();

    // The grid renders with data rows, the numeric columns, and the combined
    // assert strip. Per-assertion columns are opt-in (see the picker test):
    // as a union across the run they are mostly empty per row, and they used to
    // push tokens/cost/latency/score off the right edge of every screen.
    const grid = page.getByRole("grid");
    await expect(grid).toBeVisible();
    await expect(
      page.getByRole("columnheader", { name: "Asserts" }),
    ).toBeVisible();
    await expect(
      page.getByRole("columnheader", { name: "Latency" }),
    ).toBeVisible();
    await expect(
      page.getByRole("columnheader", { name: "contains" }),
    ).toHaveCount(0);

    const dataRows = grid.getByRole("row").filter({ has: page.getByRole("gridcell") });
    expect(await dataRows.count()).toBeGreaterThan(5);

    await expect(page.getByText(/Showing first \d+ of 500\+ cases/)).toBeVisible();
  });

  test("the column picker restores a per-assertion column and persists it", async ({
    page,
  }) => {
    await page.goto(`/runs/${MONEY_RUN}`);
    await expect(page.getByRole("grid")).toBeVisible();
    await expect(
      page.getByRole("columnheader", { name: "contains" }),
    ).toHaveCount(0);

    await page.getByRole("button", { name: /Columns/ }).click();
    await page.getByRole("checkbox", { name: "contains" }).check();
    await page.keyboard.press("Escape");

    await expect(
      page.getByRole("columnheader", { name: "contains" }),
    ).toBeVisible();

    // The choice is a viewing habit, so it survives a reload (localStorage)
    // without polluting the shareable URL.
    await page.reload();
    await expect(
      page.getByRole("columnheader", { name: "contains" }),
    ).toBeVisible();
    expect(new URL(page.url()).search).not.toContain("contains");

    // Reset puts the defaults back.
    await page.getByRole("button", { name: /Columns/ }).click();
    await page.getByRole("button", { name: "Reset" }).click();
    await page.keyboard.press("Escape");
    await expect(
      page.getByRole("columnheader", { name: "contains" }),
    ).toHaveCount(0);
  });

  test("the status column stays pinned while the grid scrolls right", async ({
    page,
  }) => {
    await page.goto(`/runs/${MONEY_RUN}`);
    const grid = page.getByRole("grid");
    await expect(grid).toBeVisible();

    const status = page.getByRole("columnheader", { name: "Status" });
    const before = await status.boundingBox();
    await grid.evaluate((el) => el.scrollBy(400, 0));
    await expect
      .poll(async () => (await status.boundingBox())?.x)
      .toBe(before?.x);
  });

  test("the output search box filters the grid", async ({ page }) => {
    await page.goto(`/runs/${MONEY_RUN}`);
    await expect(page.getByText(/Showing first \d+ of 500\+ cases/)).toBeVisible();

    // Search narrows to a single deterministic case and reflects into ?q=.
    await page.getByRole("searchbox", { name: "Search cases" }).fill("case-0000");
    await expect(page).toHaveURL(/[?&]q=case-0000/);

    await expect(page.getByText("Showing 1 case")).toBeVisible();
    await expect(page.getByText("case-0000")).toBeVisible();
    await expect(page.getByText("case-0002")).toHaveCount(0);
  });

  test("surfaces each case's output preview in the grid", async ({ page }) => {
    await page.goto(`/runs/${MONEY_RUN}`);
    await expect(page.getByText(/Showing first \d+ of 500\+ cases/)).toBeVisible();

    // The Preview column exists...
    await expect(
      page.getByRole("columnheader", { name: /Preview/ }),
    ).toBeVisible();

    // ...and renders the output preview for a specific case. Narrow to a single
    // deterministic case; its preview is one of the three fixture shapes
    // (pass/fail text, an error line, or "(skipped)").
    await page.getByRole("searchbox", { name: "Search cases" }).fill("case-0000");
    await expect(page.getByText("Showing 1 case")).toBeVisible();
    await expect(
      page
        .getByText(/produced revision r\d+|provider returned 502|\(skipped\)/)
        .first(),
    ).toBeVisible();
  });

  test("clicking a sortable header sorts the grid and round-trips through ?sort", async ({
    page,
  }) => {
    await page.goto(`/runs/${MONEY_RUN}`);
    await expect(page.getByText(/Showing first \d+ of 500\+ cases/)).toBeVisible();

    const grid = page.getByRole("grid");
    const firstRowKey = () =>
      grid
        .getByRole("row")
        .filter({ has: page.getByRole("gridcell") })
        .first()
        .getByText(/^case-\d{4}$/)
        .first();

    const initial = await firstRowKey().textContent();

    const latency = page.getByRole("columnheader", { name: /Latency/ });

    // First click -> ascending.
    await latency.getByRole("button").click();
    await expect(page).toHaveURL(/[?&]sort=latency(&|$)/);
    await expect(latency).toHaveAttribute("aria-sort", "ascending");
    const afterAsc = await firstRowKey().textContent();
    expect(afterAsc).not.toBe(initial);

    // Second click -> descending, and the top row changes again.
    await latency.getByRole("button").click();
    await expect(page).toHaveURL(/[?&]sort=-latency(&|$)/);
    await expect(latency).toHaveAttribute("aria-sort", "descending");
    const afterDesc = await firstRowKey().textContent();
    expect(afterDesc).not.toBe(afterAsc);

    // Third click clears the sort.
    await latency.getByRole("button").click();
    await expect(page).not.toHaveURL(/[?&]sort=/);
    await expect(latency).toHaveAttribute("aria-sort", "none");
  });

  test("deep-loads a sort from the URL", async ({ page }) => {
    await page.goto(`/runs/${MONEY_RUN}?sort=-latency`);
    await expect(page.getByText(/Showing first \d+ of 500\+ cases/)).toBeVisible();
    await expect(
      page.getByRole("columnheader", { name: /Latency/ }),
    ).toHaveAttribute("aria-sort", "descending");
  });

  test("the case drawer exposes a copy-link permalink", async ({ page }) => {
    await page.goto(`/runs/${MONEY_RUN}?case=case-0000`);

    const drawer = page.getByRole("dialog");
    await expect(drawer).toBeVisible();

    const copyLink = drawer.getByRole("button", { name: "Copy link" });
    await expect(copyLink).toBeVisible();

    // Clicking copies the permalink and flips the button to its "Copied" state.
    await copyLink.click();
    await expect(drawer.getByText("Copied")).toBeVisible();
  });

  test("clicking a case opens the drawer with output and assert reasoning; deep-linkable", async ({
    page,
  }) => {
    await page.goto(`/runs/${MONEY_RUN}`);

    // Restrict to passing cases so the opened case definitely has assertions.
    await page.getByRole("button", { name: "Pass", exact: true }).click();
    await expect(page).toHaveURL(/[?&]status=pass/);

    const grid = page.getByRole("grid");
    const firstRow = grid
      .getByRole("row")
      .filter({ has: page.getByRole("gridcell") })
      .first();
    await firstRow.click();

    // Drawer opens and deep-links via ?case=.
    await expect(page).toHaveURL(/[?&]case=case-/);
    const openedCase = caseParam(page);
    expect(openedCase).toBeTruthy();

    const drawer = page.getByRole("dialog");
    await expect(drawer).toBeVisible();

    // Output. Note: the real case-detail endpoint returns the stored
    // `CaseResult` verbatim, which has no "rendered prompt" field, so the
    // drawer no longer renders one (see generated CaseResult.ts).
    await expect(drawer.getByRole("heading", { name: "Output" })).toBeVisible();
    await expect(drawer.getByText(/"intent"/)).toBeVisible();

    // Per-assert verdict reasoning.
    await expect(
      drawer.getByRole("heading", { name: "Assertions" }),
    ).toBeVisible();
    await expect(drawer.getByText(/satisfied/).first()).toBeVisible();

    // Deep-linkability: reloading the ?case= URL re-opens the same drawer.
    await page.reload();
    const reopened = page.getByRole("dialog");
    await expect(reopened).toBeVisible();
    await expect(
      reopened.getByRole("heading", { name: "Output" }),
    ).toBeVisible();
    expect(caseParam(page)).toBe(openedCase);
  });

  /// The stat row is a fixed-column grid whose width must track how many stats
  /// the run actually has. It did not: adding two silently wrapped the row,
  /// pushing the cases grid down a full row of header — a regression whose only
  /// symptom was an app-shell scroll test failing somewhere else entirely.
  test("the header stats stay on one row, whatever the run reports", async ({
    page,
  }) => {
    await page.goto(`/runs/${MONEY_RUN}`);
    await expect(page.getByText("Pass rate")).toBeVisible();

    const rows = await page.evaluate(() => {
      const label = [...document.querySelectorAll("*")].find(
        (el) => el.textContent?.trim() === "Pass rate",
      );
      // The stat cells are the grid's direct children; walk up to the grid.
      const grid = label?.closest("div.grid");
      if (!grid) return null;
      const tops = new Set(
        [...grid.children].map((el) => Math.round(el.getBoundingClientRect().top)),
      );
      return { rows: tops.size, stats: grid.children.length };
    });
    expect(rows).not.toBeNull();
    expect(rows!.stats).toBeGreaterThan(6);
    expect(rows!.rows).toBe(1);
  });

  test("the breadcrumb ends at the run and links up into its set", async ({
    page,
  }) => {
    await page.goto(`/runs/${MONEY_RUN}`);
    const crumbs = page.getByRole("navigation", { name: "Breadcrumb" });

    // Ending at the suite marked the suite as the current page while we are
    // in fact on a run. The run id terminates the trail instead.
    await expect(crumbs.getByText("checkout-agent-regression-12")).toHaveAttribute(
      "aria-current",
      "page",
    );

    await crumbs.getByRole("link", { name: "regression" }).click();
    await expect(page).toHaveURL(/\/sets\/checkout-agent\/regression$/);

    await page.goBack();
    await crumbs.getByRole("link", { name: "checkout-agent" }).click();
    await expect(page).toHaveURL(/\/sets\/checkout-agent$/);
  });
});
