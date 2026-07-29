import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import { newestRunId, newestRunIds, prepareForCapture, THEMES, waitForFonts } from "./helpers";
import type { Theme } from "./helpers";

/**
 * Captures the docs' 12 reference screenshots in both themes (24 PNGs total)
 * from a real, already-seeded domarinn server (scripts/docs-screenshots.sh
 * runs scripts/seed-docs-runs.sh before this ever launches).
 *
 * Run ids are resolved through the real API via `page.request` (see
 * ./helpers.ts), never by clicking through the UI — the one exception is the
 * case-drawer shot, which opens a matrix cell because that interaction is
 * itself part of what the docs need to show.
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
      await page.goto("/runs");
      await expect(page.getByRole("heading", { name: "Eval runs" })).toBeVisible();
      await shoot(page, "runs", theme);
    });

    test("run-detail", async ({ page }) => {
      const runId = await newestRunId(page, "baselines-and-diff");
      await page.goto(`/runs/${runId}`);
      await expect(page.getByRole("link", { name: "Runs" })).toBeVisible();
      await expect(page.locator("h1").first()).toBeVisible();
      await shoot(page, "run-detail", theme);
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

    test("search", async ({ page }) => {
      // "Hello" is example 01's assertion word (the output literally contains
      // it), so it is guaranteed to have hits once the offline block is seeded.
      await page.goto("/search?q=Hello");
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
