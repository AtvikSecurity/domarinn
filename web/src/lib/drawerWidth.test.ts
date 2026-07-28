import { describe, expect, it } from "vitest";
import {
  clampWidth,
  DEFAULT_WIDTH,
  maxWidth,
  MIN_WIDTH,
  parseStoredWidth,
  toggledWidth,
} from "./drawerWidth";

describe("clampWidth", () => {
  it("keeps a sensible width unchanged", () => {
    expect(clampWidth(800, 1920)).toBe(800);
  });

  it("never goes below the minimum", () => {
    expect(clampWidth(50, 1920)).toBe(MIN_WIDTH);
  });

  // A drawer covering the whole viewport is a page, and the overlay behind it
  // stops being a visible way out.
  it("always leaves a seam at the edge", () => {
    expect(clampWidth(99999, 1000)).toBeLessThan(1000);
  });

  // A width chosen on an external monitor must not push the drawer — and its
  // drag handle — off a laptop screen.
  it("shrinks a stored width to fit a smaller viewport", () => {
    expect(clampWidth(1600, 800)).toBeLessThanOrEqual(800);
  });

  // The minimum wins over the fraction on a very narrow viewport: an unusably
  // thin drawer is worse than one that overflows slightly.
  it("prefers the minimum over the fraction when they conflict", () => {
    expect(clampWidth(100, 200)).toBe(MIN_WIDTH);
  });

  it("falls back to the default for a non-finite width", () => {
    expect(clampWidth(Number.NaN, 1920)).toBe(DEFAULT_WIDTH);
  });
});

describe("parseStoredWidth", () => {
  it("reads a stored number", () => {
    expect(parseStoredWidth("820")).toBe(820);
  });

  // localStorage is user-writable and survives upgrades; junk must not brick
  // the drawer.
  it.each([null, "", "wide", "-40", "0"])("falls back for %o", (raw) => {
    expect(parseStoredWidth(raw)).toBe(DEFAULT_WIDTH);
  });
});

describe("toggledWidth", () => {
  it("expands to the maximum from the default", () => {
    const expanded = toggledWidth(DEFAULT_WIDTH, 1920);
    expect(expanded).toBeGreaterThan(DEFAULT_WIDTH);
    expect(expanded).toBe(maxWidth(1920));
  });

  it("collapses back to the default when already expanded", () => {
    const expanded = maxWidth(1920);
    expect(toggledWidth(expanded, 1920)).toBe(DEFAULT_WIDTH);
  });

  // A drag that lands a few pixels short of the edge should still read as
  // expanded, or the next click would widen it imperceptibly instead of
  // collapsing.
  it("treats near-maximum as expanded", () => {
    const almost = maxWidth(1920) - 2;
    expect(toggledWidth(almost, 1920)).toBe(DEFAULT_WIDTH);
  });

  it("round-trips", () => {
    const once = toggledWidth(DEFAULT_WIDTH, 1440);
    expect(toggledWidth(once, 1440)).toBe(DEFAULT_WIDTH);
  });
});
