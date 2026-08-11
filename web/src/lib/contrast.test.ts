import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

/**
 * Contrast guard for the semantic colour tokens in `index.css`.
 *
 * These tones are used as 11-12px *text* throughout (StatusBadge, PassRateBadge,
 * the run-header pass/fail/err counts, assert rows, matrix popovers, the chat
 * role chips), and almost never on the bare surface — the project's chip formula
 * paints them on `bg-<tone>/12`, and `DiffView` uses `/15`. A token that clears
 * 4.5:1 on white can still fail on its own tint, so both are asserted here.
 *
 * The tokens are parsed out of the stylesheet rather than duplicated, so this
 * fails if someone edits a value without checking it.
 */

// Read the stylesheet from disk rather than importing it: vitest stubs `.css`
// imports (including `?raw`), so the source text is only reachable via fs.
// vitest's root is `web/`.
const CSS = readFileSync(resolve(process.cwd(), "src/index.css"), "utf8");

const TONES = ["pass", "fail", "error", "skip", "amber", "xfail", "xpass"] as const;
const AA_TEXT = 4.5;

function block(selector: string): string {
  // `:root { … }` / `.dark { … }` — the first brace-delimited block only.
  const start = CSS.indexOf(selector);
  if (start === -1) throw new Error(`no ${selector} block in index.css`);
  const open = CSS.indexOf("{", start);
  const close = CSS.indexOf("}", open);
  return CSS.slice(open, close);
}

function token(scope: string, name: string): string {
  const m = new RegExp(`--${name}:\\s*(#[0-9a-fA-F]{6})`).exec(block(scope));
  if (!m?.[1]) throw new Error(`--${name} not found in ${scope}`);
  return m[1].toLowerCase();
}

function channels(hex: string): [number, number, number] {
  const h = hex.replace("#", "");
  return [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16)) as [
    number,
    number,
    number,
  ];
}

function relativeLuminance(hex: string): number {
  const [r, g, b] = channels(hex).map((c) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  }) as [number, number, number];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: string, b: string): number {
  const [hi, lo] = [relativeLuminance(a), relativeLuminance(b)].sort(
    (x, y) => y - x,
  ) as [number, number];
  return (hi + 0.05) / (lo + 0.05);
}

/** The composite a `bg-<tone>/<alpha>` tint produces over `surface`. */
function tint(tone: string, surface: string, alpha: number): string {
  const f = channels(tone);
  const b = channels(surface);
  return `#${f
    .map((c, i) => Math.round(c * alpha + b[i]! * (1 - alpha)))
    .map((c) => c.toString(16).padStart(2, "0"))
    .join("")}`;
}

describe.each([
  ["light", ":root"],
  ["dark", ".dark"],
])("%s mode semantic tones", (_mode, scope) => {
  const surface = token(scope, "surface");

  it.each(TONES)("--%s reads as text on the surface", (name) => {
    expect(contrast(token(scope, name), surface)).toBeGreaterThanOrEqual(
      AA_TEXT,
    );
  });

  // The chip formula (`bg-<tone>/12 text-<tone>`) and DiffView (`/15`) are how
  // these tones are actually rendered, and are strictly harder than bare
  // surface — a token that passes above can still fail here.
  it.each(TONES)("--%s reads as text on its own /12 and /15 tint", (name) => {
    const tone = token(scope, name);
    expect(contrast(tone, tint(tone, surface, 0.12))).toBeGreaterThanOrEqual(
      AA_TEXT,
    );
    expect(contrast(tone, tint(tone, surface, 0.15))).toBeGreaterThanOrEqual(
      AA_TEXT,
    );
  });
});

it("keeps --error distinct from --amber in both themes", () => {
  // They were the same hex in dark mode, which made an `error` status badge
  // indistinguishable from a truncation / flake marker.
  for (const scope of [":root", ".dark"]) {
    expect(token(scope, "error")).not.toBe(token(scope, "amber"));
  }
});
