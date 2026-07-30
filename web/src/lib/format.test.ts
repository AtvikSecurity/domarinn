import { describe, expect, it } from "vitest";
import {
  formatBytes,
  formatCost,
  formatDate,
  formatPercent,
  formatRelative,
  formatTokens,
  parseTimestamp,
  passRate,
  shortCacheKey,
} from "./format";

describe("formatTokens", () => {
  it("formats raw, thousands, and millions", () => {
    expect(formatTokens(950)).toBe("950");
    expect(formatTokens(1500)).toBe("1.5k");
    expect(formatTokens(2_400_000)).toBe("2.40M");
    expect(formatTokens(undefined)).toBe("-");
  });
});

describe("formatCost", () => {
  it("uses more precision for sub-cent values", () => {
    expect(formatCost(0)).toBe("$0.00");
    expect(formatCost(0.0034)).toBe("$0.0034");
    expect(formatCost(1.239)).toBe("$1.24");
  });
});

describe("formatBytes", () => {
  it("scales to the right unit", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1024)).toBe("1.0 KB");
    expect(formatBytes(268_435_456)).toBe("256.0 MB");
  });
});

describe("passRate / formatPercent", () => {
  it("returns null when nothing was evaluated", () => {
    expect(passRate(0, 0, 0)).toBeNull();
    expect(formatPercent(null)).toBe("-");
  });
  it("computes over pass+fail+error", () => {
    expect(passRate(90, 8, 2)).toBeCloseTo(0.9);
    expect(formatPercent(0.9)).toBe("90.0%");
  });
});

// Regression guard for the RFC3339-vs-epoch-millis drift bug: the real server
// emits `created_at`/`last_used_at`/etc. as RFC3339 strings (see the generated
// `RunListItem`/`ApiKeyView`/... types), but the hand-written types + mocks
// used to model them as epoch-millis numbers. Date math against a real
// RFC3339 payload silently produced NaN. These lock in that the date helpers
// take RFC3339 strings end to end.
describe("parseTimestamp", () => {
  it("parses an RFC3339 string (as the real server sends) to epoch millis", () => {
    expect(parseTimestamp("2026-07-19T15:00:00Z")).toBe(
      Date.UTC(2026, 6, 19, 15, 0, 0),
    );
    expect(parseTimestamp("2026-07-19T15:00:00.500+00:00")).toBe(
      Date.UTC(2026, 6, 19, 15, 0, 0, 500),
    );
  });

  it("is usable for date-math (sorting/deltas) without producing NaN", () => {
    const older = parseTimestamp("2026-07-18T00:00:00Z");
    const newer = parseTimestamp("2026-07-19T00:00:00Z");
    expect(Number.isNaN(older)).toBe(false);
    expect(Number.isNaN(newer)).toBe(false);
    expect(newer - older).toBe(86_400_000);
  });
});

describe("formatDate / formatRelative on RFC3339 strings", () => {
  it("formatDate renders a real RFC3339 timestamp, not NaN/Invalid Date", () => {
    const rendered = formatDate("2026-07-19T15:00:00Z");
    expect(rendered).not.toBe("-");
    expect(rendered).not.toMatch(/NaN|Invalid/);
  });

  it("formatDate handles absent timestamps gracefully", () => {
    expect(formatDate(undefined)).toBe("-");
    expect(formatDate(null)).toBe("-");
  });

  it("formatRelative computes a relative offset from an RFC3339 string", () => {
    const now = Date.UTC(2026, 6, 19, 18, 0, 0);
    expect(formatRelative("2026-07-19T15:00:00Z", now)).toBe("3h ago");
    expect(formatRelative("2026-07-19T17:59:00Z", now)).toBe("1m ago");
  });

  it("formatRelative handles absent/invalid timestamps gracefully", () => {
    expect(formatRelative(undefined)).toBe("-");
    expect(formatRelative(null)).toBe("-");
    expect(formatRelative("not-a-real-timestamp")).toBe("-");
  });
});

describe("shortCacheKey", () => {
  it("keeps both ends so two keys stay distinguishable", () => {
    const key = `sha256:${"ab".repeat(32)}`;
    expect(shortCacheKey(key)).toBe("ababab…abab");
  });

  it("returns anything that is not a well-formed key untouched", () => {
    // Better to show the odd value than a tidy fiction about it.
    expect(shortCacheKey("md5:abc")).toBe("md5:abc");
    expect(shortCacheKey("sha256:short")).toBe("sha256:short");
    expect(shortCacheKey("")).toBe("");
  });
});
