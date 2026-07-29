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
  retries: 0,
  workers: 1,
  timeout: 45_000,
  expect: { timeout: 10_000 },
  reporter: [["list"]],
  use: {
    baseURL: BASE_URL,
    trace: "off",
    screenshot: "off",
    video: "off",
    viewport: { width: 1280, height: 800 },
    deviceScaleFactor: 1,
    launchOptions: {
      ...(chromeExecutable ? { executablePath: chromeExecutable } : {}),
      args: ["--no-sandbox", "--disable-dev-shm-usage"],
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
