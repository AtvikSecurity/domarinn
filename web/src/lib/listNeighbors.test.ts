import { describe, expect, it } from "vitest";
import { listNeighbors } from "./listNeighbors";

const rows = [{ key: "a" }, { key: "b" }, { key: "c" }];
const keyOf = (r: { key: string }) => r.key;

describe("listNeighbors", () => {
  it("finds both neighbours in the middle of the list", () => {
    expect(listNeighbors(rows, "b", keyOf)).toEqual({
      prevKey: "a",
      nextKey: "c",
      position: { index: 2, total: 3 },
    });
  });

  it("has no previous at the first row", () => {
    const n = listNeighbors(rows, "a", keyOf);
    expect(n.prevKey).toBeUndefined();
    expect(n.nextKey).toBe("b");
  });

  // The boundary that matters: the grid pages, so the last loaded row is not
  // the last row. Stopping here is what keeps stepping from triggering a fetch.
  it("has no next at the last LOADED row", () => {
    const n = listNeighbors(rows, "c", keyOf);
    expect(n.prevKey).toBe("b");
    expect(n.nextKey).toBeUndefined();
  });

  it("reports a 1-based position over loaded rows", () => {
    expect(listNeighbors(rows, "c", keyOf).position).toEqual({ index: 3, total: 3 });
  });

  // A deep link into a filtered page, or a row scrolled past the loaded
  // window: disable the nav rather than claim a position it cannot step from.
  it("yields nothing when the selection is not loaded", () => {
    expect(listNeighbors(rows, "zzz", keyOf)).toEqual({
      prevKey: undefined,
      nextKey: undefined,
      position: undefined,
    });
  });

  it("yields nothing when there is no selection", () => {
    expect(listNeighbors(rows, undefined, keyOf).position).toBeUndefined();
  });

  it("handles an empty list", () => {
    expect(listNeighbors([], "a", keyOf)).toEqual({
      prevKey: undefined,
      nextKey: undefined,
      position: undefined,
    });
  });

  it("gives a lone row a position but no neighbours", () => {
    expect(listNeighbors([{ key: "only" }], "only", keyOf)).toEqual({
      prevKey: undefined,
      nextKey: undefined,
      position: { index: 1, total: 1 },
    });
  });
});
