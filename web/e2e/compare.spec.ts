import { expect, test } from "@playwright/test";
import {
  MONEY_RUN,
  MONEY_RUN_BASELINE,
  caseParam,
  deltaParam,
  diffParam,
} from "./helpers";

test.describe("Compare view", () => {
  test("reached from the runs list, the compare page labels the older run as base", async ({
    page,
  }) => {
    await page.goto("/");
    await expect(page.getByRole("heading", { name: "Eval runs" })).toBeVisible();

    // Select the baseline (older) and money (newer) runs and follow the
    // resulting "Compare 2 runs" link.
    await page
      .getByRole("checkbox", { name: `Select run ${MONEY_RUN_BASELINE}` })
      .check();
    await page.getByRole("checkbox", { name: `Select run ${MONEY_RUN}` }).check();
    await page.getByRole("link", { name: "Compare 2 runs" }).click();

    // The url puts the older run first (base), newer second (head) — the
    // real server's `Path((id, other))` contract.
    await expect(page).toHaveURL(
      new RegExp(`/runs/${MONEY_RUN_BASELINE}/compare/${MONEY_RUN}$`),
    );
    await expect(page.getByRole("heading", { name: "Compare" })).toBeVisible();
    await expect(page.getByRole("combobox", { name: "Base run" })).toHaveValue(
      MONEY_RUN_BASELINE,
    );
    await expect(page.getByRole("combobox", { name: "Head run" })).toHaveValue(
      MONEY_RUN,
    );
  });

  test("shows summary chips, filters the delta grid, and expands to a side-by-side diff", async ({
    page,
  }) => {
    // The real server route is `/runs/{base}/compare/{head}` with no
    // target-less form — navigate with an explicit older-as-base pair.
    await page.goto(`/runs/${MONEY_RUN_BASELINE}/compare/${MONEY_RUN}`);

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
    await page.goto(`/runs/${MONEY_RUN_BASELINE}/compare/${MONEY_RUN}`);
    await expect(page.getByRole("heading", { name: "Compare" })).toBeVisible();

    // The base selector lists the 12 regression runs and reflects the
    // base run from the url (the first segment, per the server contract).
    const baseSelect = page.getByRole("combobox", { name: "Base run" });
    await expect(baseSelect).toBeVisible();
    await expect(baseSelect).toHaveValue(MONEY_RUN_BASELINE);
    await expect(baseSelect.locator("option")).toHaveCount(12);

    // The head selector defaults to the run in the URL.
    const headSelect = page.getByRole("combobox", { name: "Head run" });
    await expect(headSelect).toBeVisible();
    await expect(headSelect).toHaveValue(MONEY_RUN);
    await expect(headSelect.locator("option")).toHaveCount(12);
  });

  test("picking a different head (and base) recomputes the comparison", async ({ page }) => {
    await page.goto(`/runs/${MONEY_RUN_BASELINE}/compare/${MONEY_RUN}`);
    await expect(page.getByRole("heading", { name: "Compare" })).toBeVisible();

    // Re-point the head run: the base (first url segment) stays put, only
    // the head (second segment) changes.
    const headSelect = page.getByRole("combobox", { name: "Head run" });
    await headSelect.selectOption("checkout-agent-regression-10");
    await expect(page).toHaveURL(
      /\/runs\/checkout-agent-regression-11\/compare\/checkout-agent-regression-10/,
    );
    await expect(headSelect).toHaveValue("checkout-agent-regression-10");

    // Re-point the base run independently; the head stays put.
    const baseSelect = page.getByRole("combobox", { name: "Base run" });
    await baseSelect.selectOption("checkout-agent-regression-09");
    await expect(page).toHaveURL(
      /\/runs\/checkout-agent-regression-09\/compare\/checkout-agent-regression-10/,
    );
    await expect(baseSelect).toHaveValue("checkout-agent-regression-09");
    await expect(headSelect).toHaveValue("checkout-agent-regression-10");

    // The delta grid still renders with summary chips for the new pair.
    await expect(
      page.getByRole("button", { name: /^Newly failing \d+$/ }),
    ).toBeVisible();
  });

  test("shows the aggregate head-vs-base delta row with a signed value", async ({
    page,
  }) => {
    await page.goto(`/runs/${MONEY_RUN_BASELINE}/compare/${MONEY_RUN}`);
    await expect(page.getByRole("heading", { name: "Compare" })).toBeVisible();

    const deltas = page.getByTestId("aggregate-deltas");
    await expect(deltas).toBeVisible();
    // The two runs differ in size/cost, so at least one metric is signed (+ or −).
    await expect(deltas.getByText(/[+−]/).first()).toBeVisible();
  });

  test("switches diff mode via the segmented control, reflected in ?diff=", async ({
    page,
  }) => {
    await page.goto(`/runs/${MONEY_RUN_BASELINE}/compare/${MONEY_RUN}`);
    await expect(page.getByRole("heading", { name: "Compare" })).toBeVisible();

    // Filter to output-changed rows so the diff has real content, then expand
    // the first one.
    await page.getByRole("button", { name: /^Output changed \d+$/ }).click();
    await page.locator('button[aria-expanded="false"]').first().click();
    await expect(page.locator('button[aria-expanded="true"]')).toHaveCount(1);

    // Default is the side-by-side pane with no ?diff= param.
    await expect(page.locator('[data-diff-mode="side"]')).toBeVisible();
    expect(diffParam(page)).toBeNull();

    // Unified: ?diff=lines and the unified pane replaces the two columns.
    await page.getByRole("radio", { name: "Unified" }).click();
    await expect(page).toHaveURL(/[?&]diff=lines/);
    expect(diffParam(page)).toBe("lines");
    await expect(page.locator('[data-diff-mode="lines"]')).toBeVisible();
    await expect(page.locator('[data-diff-mode="side"]')).toHaveCount(0);

    // Inline: ?diff=inline and a single interleaved pane.
    await page.getByRole("radio", { name: "Inline" }).click();
    await expect(page).toHaveURL(/[?&]diff=inline/);
    await expect(page.locator('[data-diff-mode="inline"]')).toBeVisible();
  });

  test("expanding a row sets ?case=, and reloading the URL restores it", async ({
    page,
  }) => {
    await page.goto(`/runs/${MONEY_RUN_BASELINE}/compare/${MONEY_RUN}`);
    await expect(page.getByRole("heading", { name: "Compare" })).toBeVisible();

    // No row is expanded initially (no ?case=).
    expect(caseParam(page)).toBeNull();

    await page.locator('button[aria-expanded="false"]').first().click();
    await expect(page.locator('button[aria-expanded="true"]')).toHaveCount(1);

    const caseKey = caseParam(page);
    expect(caseKey).toBeTruthy();

    // Deep-load the same URL: the row auto-expands from ?case=.
    await page.reload();
    await expect(page.getByRole("heading", { name: "Compare" })).toBeVisible();
    await expect(page.locator('button[aria-expanded="true"]')).toHaveCount(1);
    expect(caseParam(page)).toBe(caseKey);

    // Deep-load a far-down case directly: the virtualized row is scrolled into
    // view and expanded (it would otherwise be outside the rendered window).
    await page.goto(
      `/runs/${MONEY_RUN_BASELINE}/compare/${MONEY_RUN}?case=case-0400`,
    );
    await expect(page.getByRole("heading", { name: "Compare" })).toBeVisible();
    await expect(page.locator('button[aria-expanded="true"]')).toHaveCount(1);
  });

  test("shows the McNemar significance panel with regression/fix counts and Wilson bars", async ({
    page,
  }) => {
    await page.goto(`/runs/${MONEY_RUN_BASELINE}/compare/${MONEY_RUN}`);
    await expect(page.getByRole("heading", { name: "Compare" })).toBeVisible();

    const panel = page.getByTestId("mcnemar-panel");
    await expect(panel).toBeVisible();
    await expect(panel.getByText("Significance")).toBeVisible();
    // McNemar counts + statistic labels.
    await expect(panel.getByText("Regressions")).toBeVisible();
    await expect(panel.getByText("Fixes")).toBeVisible();
    await expect(panel.getByText("χ² statistic")).toBeVisible();
    // A significance badge is always shown (either state).
    await expect(
      panel.getByText(/^(Statistically significant|Not significant)$/),
    ).toBeVisible();
    // Wilson CI bars for both runs, labelled `rate% (lower–upper)`.
    await expect(panel.getByText("Base pass rate")).toBeVisible();
    await expect(panel.getByText("Head pass rate")).toBeVisible();
    await expect(panel.getByText(/^\d+\.\d% \(\d+\.\d–\d+\.\d\)$/).first()).toBeVisible();
  });

  test("shows a signed per-row Δ score in the delta table", async ({ page }) => {
    await page.goto(`/runs/${MONEY_RUN_BASELINE}/compare/${MONEY_RUN}`);
    await expect(page.getByRole("heading", { name: "Compare" })).toBeVisible();

    // The Δ score column header renders.
    const table = page.getByTestId("delta-table");
    await expect(table.getByText("Δ score")).toBeVisible();

    // Newly-failing cases regress a case pass→fail, so their score drops — a
    // guaranteed signed (2-dec) Δ score in view once the grid is filtered.
    await page.getByRole("button", { name: /^Newly failing \d+$/ }).click();
    await expect(table.getByText(/^[+−]\d\.\d{2}$/).first()).toBeVisible();
  });

  test("opens the config-drift panel from the header chip and shows a structured + raw diff", async ({
    page,
  }) => {
    // The money pair straddles the regression suite's config bump, so the
    // config-drift badge lights up (see CONFIG_DRIFT_SUITE in fixtures.ts).
    await page.goto(`/runs/${MONEY_RUN_BASELINE}/compare/${MONEY_RUN}`);
    await expect(page.getByRole("heading", { name: "Compare" })).toBeVisible();

    const chip = page.getByRole("button", { name: "Config changed" });
    await expect(chip).toBeVisible();

    // No panel until the chip is clicked.
    await expect(page.getByTestId("config-drift")).toHaveCount(0);
    await chip.click();
    await expect(page).toHaveURL(/[?&]config=1/);

    const drift = page.getByTestId("config-drift");
    await expect(drift).toBeVisible();
    // Digest transition line (short forms).
    await expect(drift.getByText(/^blake3:/).first()).toBeVisible();
    // A changed scalar path row.
    await expect(drift.getByText("params.temperature")).toBeVisible();
    // The prompt-path change carries a "prompt" chip (exact match avoids the
    // `prompt.system` path text).
    await expect(drift.getByText("prompt", { exact: true })).toBeVisible();

    // The raw toggle swaps the structured diff for a unified line diff.
    await drift.getByRole("radio", { name: "Raw" }).click();
    await expect(drift.locator('[data-diff-mode="lines"]')).toBeVisible();

    // Toggling the chip again closes the panel and clears the param.
    await chip.click();
    await expect(page.getByTestId("config-drift")).toHaveCount(0);
    await expect(page).not.toHaveURL(/[?&]config=1/);
  });

  test("an expanded newly-failing row shows the assertion transitions", async ({
    page,
  }) => {
    await page.goto(`/runs/${MONEY_RUN_BASELINE}/compare/${MONEY_RUN}`);
    await expect(page.getByRole("heading", { name: "Compare" })).toBeVisible();

    // Newly-failing cases flip a case pass→fail; the first such row is a
    // pass→fail case whose culprit assertion flipped too (deterministic fixture).
    await page.getByRole("button", { name: /^Newly failing \d+$/ }).click();
    await page.locator('button[aria-expanded="false"]').first().click();
    await expect(page.locator('button[aria-expanded="true"]')).toHaveCount(1);

    const transitions = page.getByTestId("assert-transitions");
    await expect(transitions).toBeVisible();
    await expect(transitions.getByText("Assertion changes")).toBeVisible();
    // base → head status badges for the flipped assertion.
    await expect(transitions.getByText("Pass").first()).toBeVisible();
    await expect(transitions.getByText("Fail").first()).toBeVisible();
  });
});
