import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import { newestRunId, newestRunIds, prepareForCapture, THEMES, waitForFonts } from "./helpers";
import type { Theme } from "./helpers";

/**
 * Captures the docs' reference screenshots in both themes from a real,
 * already-seeded domarinn server (scripts/docs-screenshots.sh runs
 * scripts/seed-docs-runs.sh before this ever launches). Every shot is a
 * light/dark pair, consumed by docs/reference/web-ui.md through Material's
 * `#only-light` / `#only-dark` image convention.
 *
 * Run ids are resolved through the real API via `page.request` (see
 * ./helpers.ts), never by clicking through the UI — the one exception is the
 * case-drawer shot, which opens a matrix cell because that interaction is
 * itself part of what the docs need to show.
 *
 * One shot (`run-detail-local`) needs a suite only the Ollama-backed seed
 * block produces, so it skips itself under SKIP_LLM=1 rather than failing the
 * capture; its committed PNGs then simply keep their previous contents.
 */

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const OUT_DIR = path.resolve(__dirname, "..", "..", "docs", "assets", "screenshots");

function outPath(name: string, theme: Theme): string {
  return path.join(OUT_DIR, `${name}-${theme}.png`);
}

async function shoot(page: import("@playwright/test").Page, name: string, theme: Theme) {
  await waitForFonts(page);
  await page.screenshot({ path: outPath(name, theme) });
}

/**
 * A run-detail page that has finished rendering its case list.
 *
 * Deliberately NOT `getByRole("link", { name: "Runs" })`: this page carries two
 * of those — the primary nav's and the breadcrumb's — so that locator is a
 * strict-mode violation the moment both have mounted, and passes only by
 * winning a race with React. The case-count line appears once, and only after
 * the cases query has resolved, which is also exactly what the shot needs.
 *
 * `(first )?` and no trailing noun because `formatCaseCount` (web/src/lib/
 * format.ts) has three forms — "Showing N cases", "Showing first N cases" and
 * "Showing first N of M+ cases" — and which one a seeded run lands on depends
 * on whether its case list has a next page. Matching only the first form makes
 * this helper fail on a run that grew past one page.
 */
async function expectRunDetailRendered(page: import("@playwright/test").Page) {
  await expect(page.locator("h1").first()).toBeVisible();
  await expect(page.getByText(/Showing (first )?\d+/)).toBeVisible();
}

for (const theme of THEMES) {
  test.describe(`${theme} theme`, () => {
    test.beforeEach(async ({ page }) => {
      await prepareForCapture(page, theme);
    });

    test.describe("unauthenticated", () => {
      // The `capture` project's storageState is a logged-in session; the
      // login screenshot needs the opposite, so this nested describe drops it
      // for just this one test.
      test.use({ storageState: { cookies: [], origins: [] } });

      test("login", async ({ page }) => {
        await page.goto("/login");
        await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();
        await shoot(page, "login", theme);
      });
    });

    test("overview", async ({ page }) => {
      await page.goto("/");
      await expect(page.getByRole("heading", { name: "Overview" })).toBeVisible();
      await shoot(page, "overview", theme);
    });

    test("runs", async ({ page }) => {
      // `?cached=all` on purpose. The seed replays against a cache directory
      // that survives between runs, so most seeded runs are 100% cached — and
      // the list hides fully-cached passing runs by default (RunsList.tsx's
      // `cached_hidden` affordance). The default view of THIS server is
      // therefore two suites and a "14 fully cached runs hidden" line, which
      // documents the seed rather than the page. The filter lives in the URL,
      // which is what the page itself says about its filters.
      await page.goto("/runs?cached=all");
      await expect(page.getByRole("heading", { name: "Eval runs" })).toBeVisible();
      await shoot(page, "runs", theme);
    });

    test("run-detail", async ({ page }) => {
      const runId = await newestRunId(page, "baselines-and-diff");
      await page.goto(`/runs/${runId}`);
      await expectRunDetailRendered(page);
      await shoot(page, "run-detail", theme);
    });

    test("run-detail-local", async ({ page }) => {
      // The local-LLM guide's one shot: a run whose `llm-rubric` was graded by
      // the Ollama endpoint on loopback (examples/38's grader block resolves
      // OPENAI_BASE_URL/OPENAI_MODEL the same way examples/33 documents). Only
      // the Ollama-backed seed block produces it.
      const [runId] = await newestRunIds(page, "annotated-reference", 1);
      if (runId === undefined) {
        test.skip(
          true,
          "no annotated-reference run seeded (SKIP_LLM=1) — its llm-rubric needs a live judge",
        );
        return;
      }
      await page.goto(`/runs/${runId}`);
      await expectRunDetailRendered(page);
      await shoot(page, "run-detail-local", theme);
    });

    test("run-matrix", async ({ page }) => {
      // NOT suite "matrix" (examples/08-matrix-sweeps): that suite's `matrix:`
      // is a per-test VAR sweep under one provider, so
      // web/src/lib/matrix.ts's distinctProviders()/distinctPrompts() both
      // return <=1 entry for it and RunDetail.tsx's `matrixShaped` gate keeps
      // `?view=matrix` silently on the list view (see its comment: "deep-loaded
      // on a single-provider run silently falls back to list"). The UI's
      // matrix VIEW pivots on provider x prompt columns instead, so it needs a
      // suite with >1 provider: "tags-and-filters" (providers `fast`/`careful`).
      const runId = await newestRunId(page, "tags-and-filters");
      await page.goto(`/runs/${runId}?view=matrix`);
      await expect(
        page.getByRole("table", { name: "Prompt by provider matrix" }),
      ).toBeVisible();
      await shoot(page, "run-matrix", theme);
    });

    test("case-drawer", async ({ page }) => {
      const runId = await newestRunId(page, "tags-and-filters");
      await page.goto(`/runs/${runId}?view=matrix`);
      const table = page.getByRole("table", { name: "Prompt by provider matrix" });
      await expect(table).toBeVisible();

      // The one legitimate UI interaction: click a grid cell open, then a
      // repeat row inside its popover, to reach the case drawer.
      await page.locator("button[data-cell]").first().click();
      await page.getByRole("button", { name: /#0/ }).click();

      const drawer = page.getByRole("dialog");
      await expect(drawer).toBeVisible();
      await shoot(page, "case-drawer", theme);
    });

    test("compare", async ({ page }) => {
      const [newer, older] = await newestRunIds(page, "matrix", 2);
      await page.goto(`/runs/${newer}/compare/${older}`);
      await expect(page.getByRole("heading", { name: "Compare" })).toBeVisible();
      await expect(page.getByRole("combobox", { name: "Base run" })).toBeVisible();
      await shoot(page, "compare", theme);
    });

    // NOT the `/sets` root: this seed has exactly one project (every shipped
    // example declares `project: examples`), so the root listing is a
    // one-row table that documents the seed rather than the page. One level
    // down is where the suites, the trends and the flags are.
    test("set-project", async ({ page }) => {
      await page.goto("/sets/examples");
      await expect(page.getByRole("heading", { name: "examples" })).toBeVisible();
      // The rows, not just the heading: the table mounts after the query
      // resolves, so a shot taken on the heading alone races an empty card.
      await expect(page.getByTestId("suite-row-hello")).toBeVisible();
      await shoot(page, "set-project", theme);
    });

    test("set-suite", async ({ page }) => {
      // The suite scripts/seed-docs-runs.sh restricts and grants — every other
      // suite's access list is empty, which is a picture of nothing.
      // `?cached=all` for the same reason the runs shot above uses it: the
      // seed replays from a cache that survives between runs, so both of this
      // suite's runs are fully cached and the default view is the
      // "2 fully cached runs hidden" line rather than the table.
      await page.goto("/sets/examples/baselines-and-diff?cached=all");
      await expect(
        page.getByRole("heading", { name: "baselines-and-diff" }),
      ).toBeVisible();
      await expect(page.getByRole("link", { name: /^01/ }).first()).toBeVisible();
      await shoot(page, "set-suite", theme);
    });

    test("set-access", async ({ page }) => {
      await page.goto("/sets/examples/baselines-and-diff");
      await page.getByRole("button", { name: "Access" }).click();
      const panel = page.getByRole("dialog");
      await expect(panel).toBeVisible();
      // Wait for the grants, not just the modal: the panel renders its spinner
      // first, and the whole point of the shot is who is in the list.
      await expect(panel.getByTestId("grant-row-qa-lead")).toBeVisible();
      await shoot(page, "set-access", theme);
    });

    test("search", async ({ page }) => {
      // "Hello" is example 01's assertion word (the output literally contains
      // it), so it is guaranteed to have hits once the offline block is seeded.
      // `?cached=all` for the same reason as the runs and set-suite shots: the
      // seeded runs are cache replays, and hits are filtered by the cache
      // provenance of the run that owns them.
      await page.goto("/search?q=Hello&cached=all");
      await expect(page.getByText(/Cases \(/)).toBeVisible();
      await shoot(page, "search", theme);
    });

    test("cache", async ({ page }) => {
      await page.goto("/cache");
      // exact: true — the page also has an h2 "Prune cache", which Playwright's
      // default substring match would otherwise also resolve to.
      await expect(
        page.getByRole("heading", { name: "Cache", exact: true }),
      ).toBeVisible();
      await shoot(page, "cache", theme);
    });

    test("cache-entries", async ({ page }) => {
      // `?tier=local` on purpose, and it is the honest view rather than a
      // convenient one. These suites cache to disk like every default setup,
      // so the SERVER tier is empty by construction — that is exactly what the
      // stats shot above documents. The entries worth showing are the ones the
      // seeded runs actually wrote, which docs-screenshots.sh mounts as the
      // read-only local tier.
      await page.goto("/cache/entries?tier=local");
      await expect(
        page.getByRole("heading", { name: "Cache entries" }),
      ).toBeVisible();
      // The grid, not just the heading: the page renders its header and filter
      // bar before the entries query resolves, so a shot taken on the heading
      // alone races an empty table.
      await expect(page.getByRole("grid")).toBeVisible();
      await expect(page.getByRole("row").nth(1)).toBeVisible();
      await shoot(page, "cache-entries", theme);
    });

    test("settings", async ({ page }) => {
      await page.goto("/settings");
      await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();
      await shoot(page, "settings", theme);
    });

    test("keys", async ({ page }) => {
      await page.goto("/keys");
      await expect(page.getByRole("heading", { name: "API keys" })).toBeVisible();
      await shoot(page, "keys", theme);
    });

    test("admin", async ({ page }) => {
      await page.goto("/admin");
      await expect(page.getByRole("heading", { name: "Users" })).toBeVisible();
      await shoot(page, "admin", theme);
    });
  });
}
