import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

/**
 * Guard for the `.scroll-hint` edge gradients in `index.css`.
 *
 * The recipe pins a grey "there is more this way" shadow to each end of a
 * scrolling table and parks a background-coloured cover over it, so the shadow
 * only shows once you have scrolled away from that end. Whether the cover
 * actually hides the shadow is a question of gradient stops, and getting it
 * wrong is invisible in review and easy to misread on screen: the original
 * faded the cover from 0, which cancelled the shadow only partly and left a
 * faint grey band down both edges of every table, forever.
 *
 * jsdom runs with css disabled and cannot composite gradients, so this asserts
 * the invariant on the source instead: the cover must stay fully opaque across
 * at least the shadow's whole width.
 */
const CSS = readFileSync(resolve(process.cwd(), "src/index.css"), "utf8");

function scrollHintBlock(): string {
  const start = CSS.indexOf(".scroll-hint {");
  if (start === -1) throw new Error("no .scroll-hint block in index.css");
  const end = CSS.indexOf("}", start);
  return CSS.slice(start, end);
}

describe(".scroll-hint", () => {
  const block = scrollHintBlock();
  // Strip newlines/indentation so the assertions do not depend on wrapping.
  const flat = block.replace(/\s+/g, " ");

  it("pins the shadows to the box and scrolls the covers with the content", () => {
    // This is the whole no-JS trick: `scroll` shadows stay put, `local` covers
    // move with the content and so uncover the shadow exactly when you scroll.
    expect(flat.match(/no-repeat local/g)).toHaveLength(2);
    expect(flat.match(/no-repeat scroll/g)).toHaveLength(2);
  });

  // `var(--scroll-hint-bg, var(--surface))` nests, so these match through to the
  // doubled closing paren rather than the first one.
  const SOLID_STOP = /--scroll-hint-bg[^;]*?\)\)\s*0\s*(\d+)px/g;
  const FADES_FROM_ZERO = /--scroll-hint-bg[^;]*?\)\),\s*transparent\)/;

  it("keeps each cover opaque across the full width of the shadow it hides", () => {
    const shadowWidth = 14;
    const covers = [...flat.matchAll(SOLID_STOP)];
    expect(covers).toHaveLength(2);
    for (const [, solidTo] of covers) {
      expect(Number(solidTo)).toBeGreaterThanOrEqual(shadowWidth);
    }
  });

  it("does not let a cover fade straight from its start", () => {
    // `linear-gradient(to right, COLOR, transparent)` — no solid stop — is the
    // shape that leaked. Reject it explicitly so the fix cannot be undone by a
    // tidy-up that "simplifies" the stops away.
    expect(flat).not.toMatch(FADES_FROM_ZERO);
  });

  it("would actually catch the regression it is guarding against", () => {
    // A negative assertion that never matches anything is worse than no test:
    // it reads as coverage while proving nothing. Point the same pattern at the
    // exact shape that shipped the bug and confirm it fires.
    const leaky =
      "linear-gradient(to right, var(--scroll-hint-bg, var(--surface)), transparent) 0 0 / 28px 100% no-repeat local";
    expect(leaky).toMatch(FADES_FROM_ZERO);
    expect([...leaky.matchAll(SOLID_STOP)]).toHaveLength(0);
  });
});
