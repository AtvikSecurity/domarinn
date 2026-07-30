import type { Page } from "@playwright/test";

export type Theme = "light" | "dark";
export const THEMES: readonly Theme[] = ["light", "dark"];

/**
 * Set `domarinn.theme` in localStorage and disable CSS animations/transitions,
 * both before the page's own scripts ever run.
 *
 * `page.addInitScript` registers code that runs on every subsequent document
 * in this page, ahead of the app bundle — so by the time index.html's own
 * inline theme script (and then React) runs, localStorage already holds the
 * value we want. Call this once per test, before the first `page.goto`.
 */
export async function prepareForCapture(page: Page, theme: Theme): Promise<void> {
  await page.addInitScript((t: string) => {
    try {
      localStorage.setItem("domarinn.theme", t);
    } catch {
      /* ignore (e.g. storage disabled) */
    }
    const disableMotion = () => {
      const style = document.createElement("style");
      style.textContent =
        "*, *::before, *::after { animation: none !important; transition: none !important; }";
      (document.head ?? document.documentElement).appendChild(style);
    };
    if (document.readyState === "loading") {
      document.addEventListener("DOMContentLoaded", disableMotion, { once: true });
    } else {
      disableMotion();
    }
  }, theme);
}

/** Wait for web fonts to finish loading before a screenshot is taken.
 *  Deliberately not `networkidle` — a long-lived real server never goes
 *  network-idle the way a static mock build does. */
export async function waitForFonts(page: Page): Promise<void> {
  await page.evaluate(() => document.fonts.ready);
}

interface RunSummary {
  id: string;
}
interface RunListResponse {
  runs: RunSummary[];
}

/** The newest run of `suite`, resolved through the real API (never by
 *  clicking through the UI) so the spec doesn't have to know run ids. */
export async function newestRunId(page: Page, suite: string): Promise<string> {
  const ids = await newestRunIds(page, suite, 1);
  const [id] = ids;
  if (!id) throw new Error(`no runs found for suite "${suite}"`);
  return id;
}

/** The `count` newest runs of `suite`, newest first. */
export async function newestRunIds(
  page: Page,
  suite: string,
  count: number,
): Promise<string[]> {
  const res = await page.request.get(
    `/api/v1/runs?suite=${encodeURIComponent(suite)}&limit=${count}`,
  );
  if (!res.ok()) {
    throw new Error(`GET /api/v1/runs?suite=${suite} failed: ${res.status()} ${await res.text()}`);
  }
  const body = (await res.json()) as RunListResponse;
  return body.runs.map((r) => r.id);
}
