import { defineConfig } from "@playwright/test";
import { existsSync } from "node:fs";
import { execSync } from "node:child_process";

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
 * Browser selection
 * -----------------
 * On CI (or any machine with Playwright's browsers installed) this uses the
 * bundled chromium. On systems where the CDN-downloaded chromium can't run
 * (e.g. NixOS, whose prebuilt binaries fail to find system shared libraries),
 * it points at an already-installed Google Chrome / Chromium instead. Set
 * `PLAYWRIGHT_CHROME_PATH` to override detection.
 */

const PORT = 4173;
const BASE_URL = process.env.PLAYWRIGHT_BASE_URL ?? `http://localhost:${PORT}`;

function resolveChromeExecutable(): string | undefined {
  const explicit = process.env.PLAYWRIGHT_CHROME_PATH ?? process.env.CHROME_PATH;
  if (explicit && existsSync(explicit)) return explicit;

  const home = process.env.HOME ?? "";
  const user = process.env.USER ?? "";
  const candidates = [
    `${home}/.nix-profile/bin/google-chrome-stable`,
    `/etc/profiles/per-user/${user}/bin/google-chrome-stable`,
    `/etc/profiles/per-user/${user}/bin/google-chrome`,
    "/run/current-system/sw/bin/google-chrome-stable",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/google-chrome",
    "/opt/google/chrome/chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "/snap/bin/chromium",
  ];
  for (const c of candidates) {
    if (c && existsSync(c)) return c;
  }

  for (const bin of ["google-chrome-stable", "google-chrome", "chromium", "chromium-browser"]) {
    try {
      const p = execSync(`command -v ${bin} 2>/dev/null`, { encoding: "utf8" }).trim();
      if (p && existsSync(p)) return p;
    } catch {
      /* not on PATH */
    }
  }
  return undefined;
}

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
