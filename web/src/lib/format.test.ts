import { describe, expect, it } from "vitest";
import {
  formatBytes,
  formatCost,
  formatPercent,
  formatTokens,
  passRate,
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
