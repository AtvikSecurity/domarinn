import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  CACHED_PREF_KEY,
  __resetCachedPref,
  getCachedPref,
  setCachedPref,
  subscribeCachedPref,
} from "./cachedPref";

beforeEach(() => {
  localStorage.clear();
  __resetCachedPref();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("getCachedPref", () => {
  // An install that has never touched the control must behave exactly as the
  // product did before there was a preference at all.
  it("hides cached runs when nothing has been stored", () => {
    expect(getCachedPref()).toBe("exclude");
  });

  it("reads a stored preference", () => {
    localStorage.setItem(CACHED_PREF_KEY, "only");
    __resetCachedPref();
    expect(getCachedPref()).toBe("only");
  });

  it("ignores a stored value that is not a filter token", () => {
    localStorage.setItem(CACHED_PREF_KEY, "banana");
    __resetCachedPref();
    expect(getCachedPref()).toBe("exclude");
  });

  // Private browsing and sandboxed frames throw on access. A preference is not
  // worth breaking the page over.
  it("survives localStorage throwing", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("denied");
    });
    expect(() => __resetCachedPref()).not.toThrow();
    expect(getCachedPref()).toBe("exclude");
  });
});

describe("setCachedPref", () => {
  it("updates the snapshot and persists it", () => {
    setCachedPref("all");
    expect(getCachedPref()).toBe("all");
    expect(localStorage.getItem(CACHED_PREF_KEY)).toBe("all");
  });

  it("notifies subscribers", () => {
    const seen: string[] = [];
    const unsubscribe = subscribeCachedPref(() => seen.push(getCachedPref()));
    setCachedPref("only");
    setCachedPref("all");
    unsubscribe();
    setCachedPref("exclude");
    expect(seen).toEqual(["only", "all"]);
  });

  // useSyncExternalStore re-runs getSnapshot on every notify; emitting for a
  // no-op write would churn every subscribed surface.
  it("does not notify when the value is unchanged", () => {
    const listener = vi.fn();
    const unsubscribe = subscribeCachedPref(listener);
    setCachedPref("exclude");
    unsubscribe();
    expect(listener).not.toHaveBeenCalled();
  });

  it("survives localStorage throwing", () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("quota");
    });
    expect(() => setCachedPref("all")).not.toThrow();
    // The in-memory value still moves, so this tab stays coherent.
    expect(getCachedPref()).toBe("all");
  });
});

describe("cross-tab sync", () => {
  it("adopts a preference set in another tab", () => {
    const listener = vi.fn();
    const unsubscribe = subscribeCachedPref(listener);
    localStorage.setItem(CACHED_PREF_KEY, "only");
    window.dispatchEvent(
      new StorageEvent("storage", { key: CACHED_PREF_KEY, newValue: "only" }),
    );
    unsubscribe();
    expect(getCachedPref()).toBe("only");
    expect(listener).toHaveBeenCalled();
  });

  it("ignores storage events for other keys", () => {
    const listener = vi.fn();
    const unsubscribe = subscribeCachedPref(listener);
    window.dispatchEvent(
      new StorageEvent("storage", { key: "domarinn.theme", newValue: "dark" }),
    );
    unsubscribe();
    expect(listener).not.toHaveBeenCalled();
  });
});
