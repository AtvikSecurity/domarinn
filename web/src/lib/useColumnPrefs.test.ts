import { beforeEach, describe, expect, it, vi } from "vitest";
import { COLUMNS_KEY, LEGACY_GRID_COLUMNS_KEY, serializePrefs } from "./tableColumns";
import {
  __resetColumnPrefs,
  getColumnPrefs,
  resetColumns,
  setColumnVisible,
  setColumnWidth,
  subscribeColumnPrefs,
} from "./useColumnPrefs";

beforeEach(() => {
  localStorage.clear();
  __resetColumnPrefs();
});

describe("getColumnPrefs", () => {
  it("starts empty for a table nobody has configured", () => {
    expect(getColumnPrefs("runs")).toEqual({ visible: {}, width: {} });
  });

  // useSyncExternalStore calls getSnapshot on every render and loops if the
  // reference changes without a mutation.
  it("returns a stable reference until that table changes", () => {
    const first = getColumnPrefs("runs");
    expect(getColumnPrefs("runs")).toBe(first);

    setColumnVisible("cases", "cost", false);
    expect(getColumnPrefs("runs")).toBe(first);

    setColumnVisible("runs", "cost", false);
    expect(getColumnPrefs("runs")).not.toBe(first);
  });

  it("reads a stored blob", () => {
    localStorage.setItem(
      COLUMNS_KEY,
      serializePrefs({ runs: { visible: { tags: false }, width: { cost: 120 } } }),
    );
    __resetColumnPrefs();
    expect(getColumnPrefs("runs")).toEqual({
      visible: { tags: false },
      width: { cost: 120 },
    });
  });

  it("adopts the case grid's legacy key on first read", () => {
    localStorage.setItem(LEGACY_GRID_COLUMNS_KEY, '{"cost":false}');
    __resetColumnPrefs();
    expect(getColumnPrefs("cases").visible).toEqual({ cost: false });
    // The legacy key survives, so a rollback keeps the setting.
    expect(localStorage.getItem(LEGACY_GRID_COLUMNS_KEY)).toBe('{"cost":false}');
  });
});

describe("mutations", () => {
  it("persists a visibility change and notifies", () => {
    const listener = vi.fn();
    const unsubscribe = subscribeColumnPrefs(listener);
    setColumnVisible("runs", "tags", false);
    unsubscribe();

    expect(getColumnPrefs("runs").visible).toEqual({ tags: false });
    expect(listener).toHaveBeenCalled();
    expect(localStorage.getItem(COLUMNS_KEY)).toContain("tags");
  });

  it("persists a width and keeps tables independent", () => {
    setColumnWidth("runs", "cost", 140);
    setColumnWidth("cases", "cost", 200);
    expect(getColumnPrefs("runs").width).toEqual({ cost: 140 });
    expect(getColumnPrefs("cases").width).toEqual({ cost: 200 });
  });

  it("resets one table without touching the others", () => {
    setColumnVisible("runs", "tags", false);
    setColumnVisible("cases", "cost", false);
    resetColumns("runs");
    expect(getColumnPrefs("runs")).toEqual({ visible: {}, width: {} });
    expect(getColumnPrefs("cases").visible).toEqual({ cost: false });
  });

  it("does not notify for a no-op write", () => {
    setColumnVisible("runs", "tags", false);
    const listener = vi.fn();
    const unsubscribe = subscribeColumnPrefs(listener);
    setColumnVisible("runs", "tags", false);
    unsubscribe();
    expect(listener).not.toHaveBeenCalled();
  });

  it("survives localStorage throwing", () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("quota");
    });
    expect(() => setColumnWidth("runs", "cost", 140)).not.toThrow();
    // The in-memory value still moves, so this tab stays coherent.
    expect(getColumnPrefs("runs").width).toEqual({ cost: 140 });
    vi.restoreAllMocks();
  });
});

describe("cross-tab sync", () => {
  it("adopts a change made in another tab", () => {
    const listener = vi.fn();
    const unsubscribe = subscribeColumnPrefs(listener);
    localStorage.setItem(
      COLUMNS_KEY,
      serializePrefs({ runs: { visible: { tags: false }, width: {} } }),
    );
    window.dispatchEvent(new StorageEvent("storage", { key: COLUMNS_KEY }));
    unsubscribe();

    expect(getColumnPrefs("runs").visible).toEqual({ tags: false });
    expect(listener).toHaveBeenCalled();
  });

  it("ignores storage events for other keys", () => {
    const listener = vi.fn();
    const unsubscribe = subscribeColumnPrefs(listener);
    window.dispatchEvent(new StorageEvent("storage", { key: "domarinn.theme" }));
    unsubscribe();
    expect(listener).not.toHaveBeenCalled();
  });
});
