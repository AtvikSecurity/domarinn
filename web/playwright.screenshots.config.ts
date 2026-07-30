import { defineConfig } from "@playwright/test";
import { resolveChromeExecutable } from "./playwright.shared";

/**
 * Screenshot config for the docs pipeline (`mise run screenshots` ->
 * scripts/docs-screenshots.sh -> `pnpm -C web run screenshots`).
 *
 * Unlike playwright.config.ts (the e2e suite, which builds and serves the
 * *mock* UI itself), this config points at a REAL, already-running domarinn
 * server — scripts/docs-screenshots.sh starts it and seeds it with real runs
 * before this ever launches a browser. There is no `webServer` block here:
 * standing up the server is that script's job, not Playwright's.
 *
 * Two projects, chained: `setup` logs in once via the real API and saves the
 * session cookie to storageState; `capture` depends on it and reuses that
 * storageState for every screenshot spec, so only one login happens per run.
 */

const BASE_URL = process.env.PLAYWRIGHT_BASE_URL ?? "http://localhost:8322";
const AUTH_STATE = "screenshots/.auth/session.json";

const chromeExecutable = resolveChromeExecutable();

export default defineConfig({
  testDir: "./screenshots",
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  // One retry, unlike the e2e suite's zero. A retry here cannot hide a
  // regression the way it can in a test suite: the artifact is the PNG, and a
  // re-shot page is either right or visibly wrong. It exists because
  // `Page.captureScreenshot` intermittently fails outright ("Unable to capture
  // screenshot") on the first capture after a browser launch at this window
  // size, which would otherwise abort a whole pipeline run over a compositor
  // race.
  retries: 1,
  workers: 1,
  timeout: 45_000,
  expect: { timeout: 10_000 },
  reporter: [["list"]],
  use: {
    baseURL: BASE_URL,
    trace: "off",
    screenshot: "off",
    video: "off",
    // Full-size desktop window. The docs' screenshots are read at page width on
    // a wide screen, and a 1280x800 capture of a data-dense table view crops
    // columns the surrounding prose then describes. Keep deviceScaleFactor at 1:
    // a 2x capture would double the byte size of every committed PNG for no
    // gain at the width the site renders them.
    viewport: { width: 1920, height: 1080 },
    deviceScaleFactor: 1,
    launchOptions: {
      ...(chromeExecutable ? { executablePath: chromeExecutable } : {}),
      // `--window-size` matches the OS window to the viewport above. Without
      // it the window keeps its default size and the page is rendered into a
      // smaller compositor surface first, which shows up as scrollbar gutters
      // and re-layout artifacts in the captured PNG.
      args: ["--no-sandbox", "--disable-dev-shm-usage", "--window-size=1920,1080"],
    },
  },
  projects: [
    {
      name: "setup",
      testMatch: /auth\.setup\.ts/,
      use: { browserName: "chromium" },
    },
    {
      name: "capture",
      testMatch: /capture\.spec\.ts/,
      dependencies: ["setup"],
      use: {
        browserName: "chromium",
        storageState: AUTH_STATE,
      },
    },
  ],
});
