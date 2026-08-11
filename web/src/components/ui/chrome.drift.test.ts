import { existsSync, readdirSync, readFileSync } from "node:fs";
import { relative, resolve } from "node:path";
import { describe, expect, it } from "vitest";

const SRC = resolve(process.cwd(), "src");
const RECIPE = resolve(SRC, "components/ui/chrome.ts");
const RECIPE_MARKERS = [
  "border-chrome-border",
  "shadow-[inset_0_1px_0_0_var(--color-chrome-highlight)]",
  // The tab rule. Two components carry it now — the segmented control and run
  // detail's filter group — and they differ in size and in the role they
  // claim, which makes re-inlining the states the tempting shortcut.
  "border-b-2 border-transparent",
  "hover:border-border-strong hover:text-fg",
] as const;

function sourceFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = resolve(dir, entry.name);
    if (entry.isDirectory()) return sourceFiles(path);
    if (!/\.(?:css|ts|tsx)$/.test(entry.name)) return [];
    if (entry.name.includes(".test.")) return [];
    return [path];
  });
}

describe("chrome frame recipe", () => {
  it("has one source of truth for the border and inset highlight", () => {
    expect(existsSync(RECIPE)).toBe(true);
    const sources = sourceFiles(SRC).map((path) => ({
      path: relative(SRC, path),
      text: readFileSync(path, "utf8"),
    }));

    for (const marker of RECIPE_MARKERS) {
      const owners = sources
        .filter((source) => source.text.includes(marker))
        .map((source) => source.path);
      expect(owners).toEqual(["components/ui/chrome.ts"]);
    }
  });
});
