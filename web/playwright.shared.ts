import { existsSync } from "node:fs";
import { execSync } from "node:child_process";

/**
 * Resolve which Chrome/Chromium binary Playwright should launch.
 *
 * Shared by every Playwright config in this package (the e2e config and the
 * docs-screenshot config): both need the same "use the bundled chromium on
 * CI, fall back to a system browser everywhere else" behavior, so this lives
 * in one place rather than being copy-pasted.
 *
 * How it picks
 * ------------
 * On CI (or any machine with Playwright's browsers installed) this returns
 * `undefined`, so Playwright launches its own bundled chromium. On systems
 * where the CDN-downloaded chromium can't run (e.g. NixOS, whose prebuilt
 * binaries fail to find system shared libraries), it points at an
 * already-installed Google Chrome / Chromium instead. Set
 * `PLAYWRIGHT_CHROME_PATH` to override detection.
 */
export function resolveChromeExecutable(): string | undefined {
  const explicit = process.env.PLAYWRIGHT_CHROME_PATH ?? process.env.CHROME_PATH;
  if (explicit && existsSync(explicit)) return explicit;

  // On CI the bundled chromium is installed deliberately and is version-matched
  // to the driver. The runner image also ships a system Google Chrome, which the
  // detection below would otherwise prefer — silently swapping a matched browser
  // for one that can drift from the driver's supported protocol. An explicit
  // override above still wins.
  if (process.env.CI) return undefined;

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
