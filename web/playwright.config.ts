import { defineConfig } from "@playwright/test";
import { resolveChromeExecutable } from "./playwright.shared";

/**
 * E2E config for the domarinn web UI.
 *
 * How mock mode is served
 * -----------------------
 * `import.meta.env.VITE_MOCK` is inlined by Vite at *build* time, so mock mode
 * must be baked into the bundle that `vite preview` serves. The `webServer`
 * block therefore runs a mock build (`VITE_MOCK=1 vite build`) and then previews
 * the resulting `dist/` on a fixed port. This makes the deterministic fixture in
 * `src/mocks/` active with no Rust backend required.
 *
 * Browser selection: see `resolveChromeExecutable` in ./playwright.shared.ts,
 * shared with the docs-screenshot config.
 */

const PORT = 4173;
const BASE_URL = process.env.PLAYWRIGHT_BASE_URL ?? `http://localhost:${PORT}`;

const chromeExecutable = resolveChromeExecutable();

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 1 : undefined,
  timeout: 45_000,
  expect: { timeout: 10_000 },
  reporter: process.env.CI
    ? [["list"], ["html", { open: "never" }]]
    : [["list"]],
  use: {
    baseURL: BASE_URL,
    trace: "on-first-retry",
    screenshot: "only-on-failure",
    video: "off",
  },
  projects: [
    {
      name: "chromium",
      use: {
        browserName: "chromium",
        viewport: { width: 1280, height: 800 },
        launchOptions: {
          ...(chromeExecutable ? { executablePath: chromeExecutable } : {}),
          // Keep headless launches robust inside containers / sandboxes.
          args: ["--no-sandbox", "--disable-dev-shm-usage"],
        },
      },
    },
  ],
  webServer: {
    // Build with the mock fixture inlined, then serve the static build.
    command: "pnpm run e2e:serve",
    url: BASE_URL,
    reuseExistingServer: !process.env.CI,
    timeout: 180_000,
    stdout: "pipe",
    stderr: "pipe",
  },
});
