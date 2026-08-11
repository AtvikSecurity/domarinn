import { describe, expect, it } from "vitest";
import { compareStatus, parseSort, serializeSort, STATUS_RANK } from "./sort";

describe("parseSort", () => {
  it("parses an ascending column", () => {
    expect(parseSort("latency")).toEqual([{ id: "latency", desc: false }]);
  });

  it("parses a descending column via the '-' prefix", () => {
    expect(parseSort("-latency")).toEqual([{ id: "latency", desc: true }]);
  });

  it("returns empty for null / empty / whitespace / a bare '-'", () => {
    expect(parseSort(null)).toEqual([]);
    expect(parseSort("")).toEqual([]);
    expect(parseSort("   ")).toEqual([]);
    expect(parseSort("-")).toEqual([]);
    expect(parseSort("  -  ")).toEqual([]);
  });
});

describe("serializeSort", () => {
  it("serializes ascending and descending", () => {
    expect(serializeSort([{ id: "cost", desc: false }])).toBe("cost");
    expect(serializeSort([{ id: "cost", desc: true }])).toBe("-cost");
  });

  it("returns null for an empty sort", () => {
    expect(serializeSort([])).toBeNull();
  });

  it("only represents the primary column", () => {
    expect(
      serializeSort([
        { id: "status", desc: true },
        { id: "name", desc: false },
      ]),
    ).toBe("-status");
  });
});

describe("parse <-> serialize round-trip", () => {
  it.each([
    "name",
    "-name",
    "status",
    "-status",
    "tokens",
    "-cost",
    "score",
    // Matrix-shaped grid columns (Task 11) are sortable too.
    "provider",
    "-provider",
    "prompt",
    "-prompt",
  ])("round-trips %s", (param) => {
    expect(serializeSort(parseSort(param))).toBe(param);
  });

  it("round-trips an empty sort as null -> []", () => {
    expect(serializeSort(parseSort(""))).toBeNull();
    expect(parseSort(serializeSort([]))).toEqual([]);
  });
});

describe("status rank comparator", () => {
  it("ranks fail > error > pass > skip", () => {
    expect(STATUS_RANK.fail).toBeGreaterThan(STATUS_RANK.xpass);
    // xpass floats next to fail (it fails the gate); xfail sinks below
    // pass (it is expected and unremarkable) but above skip (it was graded).
    expect(STATUS_RANK.xpass).toBeGreaterThan(STATUS_RANK.error);
    expect(STATUS_RANK.error).toBeGreaterThan(STATUS_RANK.pass);
    expect(STATUS_RANK.pass).toBeGreaterThan(STATUS_RANK.xfail);
    expect(STATUS_RANK.xfail).toBeGreaterThan(STATUS_RANK.skip);
  });

  it("sorts ascending as skip < pass < error < fail", () => {
    const asc = (["error", "skip", "fail", "pass"] as const)
      .slice()
      .sort(compareStatus);
    expect(asc).toEqual(["skip", "pass", "error", "fail"]);
  });

  it("descending (the reverse) floats failures then errors to the top", () => {
    const desc = (["pass", "skip", "fail", "error"] as const)
      .slice()
      .sort((a, b) => compareStatus(b, a));
    expect(desc).toEqual(["fail", "error", "pass", "skip"]);
  });

  it("is sign-symmetric and reflexive", () => {
    expect(Math.sign(compareStatus("fail", "skip"))).toBe(1);
    expect(Math.sign(compareStatus("skip", "fail"))).toBe(-1);
    expect(compareStatus("pass", "pass")).toBe(0);
  });
});
