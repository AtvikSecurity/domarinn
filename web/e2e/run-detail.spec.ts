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

    // The grid renders with its assert-label columns and data rows.
    const grid = page.getByRole("grid");
    await expect(grid).toBeVisible();
    await expect(
      page.getByRole("columnheader", { name: "contains" }),
    ).toBeVisible();

    const dataRows = grid.getByRole("row").filter({ has: page.getByRole("gridcell") });
    expect(await dataRows.count()).toBeGreaterThan(5);

    // Footer reports the 500-case total.
    await expect(page.getByText(/Showing \d+ of 500\+ cases/)).toBeVisible();
  });

  test("the output search box filters the grid", async ({ page }) => {
    await page.goto(`/runs/${MONEY_RUN}`);
    await expect(page.getByText(/Showing \d+ of 500\+ cases/)).toBeVisible();

    // Search narrows to a single deterministic case and reflects into ?q=.
    await page.getByRole("searchbox", { name: "Search cases" }).fill("case-0000");
    await expect(page).toHaveURL(/[?&]q=case-0000/);

    await expect(page.getByText("Showing 1 of 500+ cases")).toBeVisible();
    await expect(page.getByText("case-0000")).toBeVisible();
    await expect(page.getByText("case-0002")).toHaveCount(0);
  });

  test("surfaces each case's output preview in the grid", async ({ page }) => {
    await page.goto(`/runs/${MONEY_RUN}`);
    await expect(page.getByText(/Showing \d+ of 500\+ cases/)).toBeVisible();

    // The Preview column exists...
    await expect(
      page.getByRole("columnheader", { name: /Preview/ }),
    ).toBeVisible();

    // ...and renders the output preview for a specific case. Narrow to a single
    // deterministic case; its preview is one of the three fixture shapes
    // (pass/fail text, an error line, or "(skipped)").
    await page.getByRole("searchbox", { name: "Search cases" }).fill("case-0000");
    await expect(page.getByText("Showing 1 of 500+ cases")).toBeVisible();
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
    await expect(page.getByText(/Showing \d+ of 500\+ cases/)).toBeVisible();

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
    await expect(page.getByText(/Showing \d+ of 500\+ cases/)).toBeVisible();
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
});
