import { describe, expect, it } from "vitest";
import {
  flatten,
  formatLeaf,
  isPromptPath,
  jsonDiff,
  type JsonDiffEntry,
} from "./jsonDiff";

/** Small helper: index the diff by path for order-independent assertions. */
function byPath(entries: JsonDiffEntry[]): Record<string, JsonDiffEntry> {
  return Object.fromEntries(entries.map((e) => [e.path, e]));
}

describe("flatten", () => {
  it("flattens nested objects with dotted paths", () => {
    const m = flatten({ a: 1, b: { c: 2, d: { e: 3 } } });
    expect([...m.entries()].sort()).toEqual([
      ["a", 1],
      ["b.c", 2],
      ["b.d.e", 3],
    ]);
  });

  it("indexes arrays by position", () => {
    const m = flatten({ xs: ["a", "b"], nested: [{ k: 1 }] });
    expect(m.get("xs.0")).toBe("a");
    expect(m.get("xs.1")).toBe("b");
    expect(m.get("nested.0.k")).toBe(1);
  });

  it("treats empty objects/arrays and every primitive as leaves", () => {
    const m = flatten({ empty: {}, arr: [], n: null, t: true, s: "x", z: 0 });
    expect(m.get("empty")).toEqual({});
    expect(m.get("arr")).toEqual([]);
    expect(m.get("n")).toBeNull();
    expect(m.get("t")).toBe(true);
    expect(m.get("s")).toBe("x");
    expect(m.get("z")).toBe(0);
  });

  it("flattens a scalar top level under the empty path", () => {
    expect(flatten(42).get("")).toBe(42);
  });
});

describe("jsonDiff", () => {
  it("returns nothing for identical (deeply equal) values", () => {
    expect(jsonDiff({ a: 1, b: { c: [1, 2] } }, { a: 1, b: { c: [1, 2] } })).toEqual([]);
  });

  it("classifies added, removed, and changed leaves", () => {
    const before = { keep: 1, drop: 2, move: "old" };
    const after = { keep: 1, add: 3, move: "new" };
    const d = byPath(jsonDiff(before, after));

    expect(d["add"]).toMatchObject({ kind: "added", before: undefined, after: 3 });
    expect(d["drop"]).toMatchObject({ kind: "removed", before: 2, after: undefined });
    expect(d["move"]).toMatchObject({ kind: "changed", before: "old", after: "new" });
    expect(d["keep"]).toBeUndefined();
  });

  it("detects a type change (number vs string) as changed", () => {
    const d = jsonDiff({ n: 1 }, { n: "1" });
    expect(d).toHaveLength(1);
    expect(d[0]).toMatchObject({ path: "n", kind: "changed", before: 1, after: "1" });
  });

  it("detects null <-> value transitions", () => {
    const d = byPath(jsonDiff({ a: null, b: 1 }, { a: 2, b: null }));
    expect(d["a"]).toMatchObject({ kind: "changed", before: null, after: 2 });
    expect(d["b"]).toMatchObject({ kind: "changed", before: 1, after: null });
  });

  it("diffs nested objects and arrays by path/index", () => {
    const before = { cfg: { asserts: [{ kind: "contains" }, { kind: "regex" }] } };
    const after = { cfg: { asserts: [{ kind: "contains" }, { kind: "similar" }] } };
    const d = jsonDiff(before, after);
    expect(d).toHaveLength(1);
    expect(d[0]).toMatchObject({
      path: "cfg.asserts.1.kind",
      kind: "changed",
      before: "regex",
      after: "similar",
    });
  });

  it("reports array growth/shrink as added/removed indices", () => {
    const grow = byPath(jsonDiff({ xs: [1] }, { xs: [1, 2, 3] }));
    expect(grow["xs.1"]).toMatchObject({ kind: "added", after: 2 });
    expect(grow["xs.2"]).toMatchObject({ kind: "added", after: 3 });

    const shrink = byPath(jsonDiff({ xs: [1, 2] }, { xs: [1] }));
    expect(shrink["xs.1"]).toMatchObject({ kind: "removed", before: 2 });
  });

  it("treats [] vs {} as a change (empty-container shape differs)", () => {
    const d = jsonDiff({ x: [] }, { x: {} });
    expect(d).toHaveLength(1);
    expect(d[0]).toMatchObject({ path: "x", kind: "changed" });
  });

  it("sorts entries by path for a stable render", () => {
    const d = jsonDiff({ z: 1, a: 1, m: 1 }, { z: 2, a: 2, m: 2 });
    expect(d.map((e) => e.path)).toEqual(["a", "m", "z"]);
  });

  it("diffs boolean flips", () => {
    const d = jsonDiff({ on: true }, { on: false });
    expect(d[0]).toMatchObject({ path: "on", kind: "changed", before: true, after: false });
  });
});

describe("isPromptPath", () => {
  it("matches prompt/template/message/system paths, case-insensitively", () => {
    expect(isPromptPath("prompt.system")).toBe(true);
    expect(isPromptPath("prompt.template")).toBe(true);
    expect(isPromptPath("messages.0.content")).toBe(true);
    expect(isPromptPath("System")).toBe(true);
    expect(isPromptPath("params.temperature")).toBe(false);
    expect(isPromptPath("model")).toBe(false);
  });
});

describe("formatLeaf", () => {
  it("renders strings verbatim, non-strings as JSON, undefined as em-dash", () => {
    expect(formatLeaf("hello")).toBe("hello");
    expect(formatLeaf(1)).toBe("1");
    expect(formatLeaf(true)).toBe("true");
    expect(formatLeaf(null)).toBe("null");
    expect(formatLeaf({})).toBe("{}");
    expect(formatLeaf(undefined)).toBe("—");
  });
});
