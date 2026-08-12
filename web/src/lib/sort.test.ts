import { describe, expect, it } from "vitest";
import {
  compareStatus,
  compareValues,
  cycleSort,
  parseSort,
  serializeSort,
  sortRows,
  STATUS_RANK,
} from "./sort";

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

describe("cycleSort", () => {
  it("cycles a column asc -> desc -> cleared", () => {
    const asc = cycleSort([], "when");
    expect(asc).toEqual([{ id: "when", desc: false }]);
    const desc = cycleSort(asc, "when");
    expect(desc).toEqual([{ id: "when", desc: true }]);
    expect(cycleSort(desc, "when")).toEqual([]);
  });

  it("cycles desc -> asc -> cleared with descFirst", () => {
    const desc = cycleSort([], "size", true);
    expect(desc).toEqual([{ id: "size", desc: true }]);
    const asc = cycleSort(desc, "size", true);
    expect(asc).toEqual([{ id: "size", desc: false }]);
    expect(cycleSort(asc, "size", true)).toEqual([]);
  });

  it("clicking a different column starts its cycle fresh", () => {
    expect(cycleSort([{ id: "when", desc: true }], "cost")).toEqual([
      { id: "cost", desc: false },
    ]);
  });
});

describe("compareValues", () => {
  it("compares numbers numerically, not lexically", () => {
    expect(compareValues(9, 10)).toBeLessThan(0);
    expect(compareValues(10, 9)).toBeGreaterThan(0);
    expect(compareValues(3, 3)).toBe(0);
  });

  it("compares strings with localeCompare", () => {
    expect(compareValues("alpha", "beta")).toBeLessThan(0);
    expect(compareValues("beta", "alpha")).toBeGreaterThan(0);
  });
});

describe("sortRows", () => {
  interface Row {
    name: string;
    cost: number | null;
  }
  const rows: readonly Row[] = [
    { name: "c", cost: 2 },
    { name: "a", cost: null },
    { name: "b", cost: 1 },
  ];
  const fields = {
    name: (r: Row) => r.name,
    cost: (r: Row) => r.cost,
  };

  it("returns the input unchanged (same reference) when unsorted", () => {
    expect(sortRows(rows, [], fields)).toBe(rows);
  });

  it("returns the input unchanged for an unknown column id", () => {
    expect(sortRows(rows, [{ id: "bogus", desc: false }], fields)).toBe(rows);
  });

  it("sorts ascending and descending without mutating the input", () => {
    const asc = sortRows(rows, [{ id: "name", desc: false }], fields);
    expect(asc.map((r) => r.name)).toEqual(["a", "b", "c"]);
    const desc = sortRows(rows, [{ id: "name", desc: true }], fields);
    expect(desc.map((r) => r.name)).toEqual(["c", "b", "a"]);
    expect(rows.map((r) => r.name)).toEqual(["c", "a", "b"]);
  });

  it("puts null values last in BOTH directions", () => {
    const asc = sortRows(rows, [{ id: "cost", desc: false }], fields);
    expect(asc.map((r) => r.name)).toEqual(["b", "c", "a"]);
    const desc = sortRows(rows, [{ id: "cost", desc: true }], fields);
    expect(desc.map((r) => r.name)).toEqual(["c", "b", "a"]);
  });

  it("is stable: equal keys keep their incoming order", () => {
    const tied: readonly Row[] = [
      { name: "x", cost: 1 },
      { name: "y", cost: 1 },
      { name: "z", cost: 0 },
    ];
    const sorted = sortRows(tied, [{ id: "cost", desc: false }], fields);
    expect(sorted.map((r) => r.name)).toEqual(["z", "x", "y"]);
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
