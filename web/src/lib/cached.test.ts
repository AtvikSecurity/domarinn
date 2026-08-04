import { describe, expect, it } from "vitest";
import {
  hiddenByCachedExclude,
  isCachedFilter,
  isFullyCached,
  resolveCached,
} from "./cached";

/** A run whose every provider call was served from cache, and which passed. */
function cachedPassing() {
  return { cache_hits: 12, cache_misses: 0, fail_count: 0, error_count: 0 };
}

describe("isFullyCached", () => {
  it("is true when every provider call was a cache hit", () => {
    expect(isFullyCached({ cache_hits: 12, cache_misses: 0 })).toBe(true);
  });

  it("is false when any call missed the cache", () => {
    expect(isFullyCached({ cache_hits: 11, cache_misses: 1 })).toBe(false);
  });

  // Zero of both is a run that made no provider calls at all. It is not
  // "cached" in any useful sense, and treating it as such would hide runs
  // whose cases all errored before reaching a provider.
  it("is false when the run made no provider calls", () => {
    expect(isFullyCached({ cache_hits: 0, cache_misses: 0 })).toBe(false);
  });

  // Mirrors the server rule: a row we cannot classify is never treated as
  // cached, so it is never hidden. Legacy pre-backfill rows arrive as null.
  it("is false for legacy rows with unknown counters", () => {
    expect(isFullyCached({ cache_hits: null, cache_misses: null })).toBe(false);
  });

  it("is false when only one counter is known", () => {
    expect(isFullyCached({ cache_hits: 5, cache_misses: null })).toBe(false);
    expect(isFullyCached({ cache_hits: null, cache_misses: 0 })).toBe(false);
  });

  // `clean_cache_count` maps the -1 undecodable-blob sentinel to null before
  // it reaches the wire, so this should be unreachable — but if one ever leaks
  // through, "not cached" is the safe direction: it shows the run rather than
  // hiding it.
  it("is false for the -1 backfill sentinel", () => {
    expect(isFullyCached({ cache_hits: -1, cache_misses: -1 })).toBe(false);
  });
});

describe("hiddenByCachedExclude", () => {
  it("hides a fully cached run that passed", () => {
    expect(hiddenByCachedExclude(cachedPassing())).toBe(true);
  });

  // A verdict does not save a replay from being hidden. The rule used to spare
  // failing ones, on the reasoning that grader verdicts are not cached so a
  // replay could carry a fresh regression — which 0.5.0 had already made
  // false by putting graders in the same cache as provider calls. What it did
  // do was fire on every run of any suite whose failures are not rare, so the
  // filter hid nothing at all.
  it("hides a fully cached run that failed", () => {
    expect(
      hiddenByCachedExclude({ ...cachedPassing(), fail_count: 1 }),
    ).toBe(true);
  });

  it("hides a fully cached run that errored", () => {
    expect(
      hiddenByCachedExclude({ ...cachedPassing(), error_count: 1 }),
    ).toBe(true);
  });

  it("keeps a run that hit the provider even when it passed", () => {
    expect(
      hiddenByCachedExclude({ ...cachedPassing(), cache_misses: 3 }),
    ).toBe(false);
  });

  it("keeps a legacy run we cannot classify", () => {
    expect(
      hiddenByCachedExclude({
        ...cachedPassing(),
        cache_hits: null,
        cache_misses: null,
      }),
    ).toBe(false);
  });
});

describe("isCachedFilter", () => {
  it("accepts the three tokens the server understands", () => {
    expect(isCachedFilter("exclude")).toBe(true);
    expect(isCachedFilter("only")).toBe(true);
    expect(isCachedFilter("all")).toBe(true);
  });

  it("rejects anything else", () => {
    expect(isCachedFilter("banana")).toBe(false);
    expect(isCachedFilter("")).toBe(false);
    expect(isCachedFilter(null)).toBe(false);
    expect(isCachedFilter(undefined)).toBe(false);
  });
});

describe("resolveCached", () => {
  // A shared link must show the same runs to whoever opens it, whatever their
  // own preference says. That is the whole reason the URL wins.
  it("takes an explicit URL value over the preference", () => {
    expect(resolveCached("all", "exclude")).toBe("all");
    expect(resolveCached("exclude", "all")).toBe("exclude");
    expect(resolveCached("only", "all")).toBe("only");
  });

  // Absence is what every link written before this feature carries, and what
  // "Clear filters" leaves behind. It must mean "whatever I normally want".
  it("falls back to the preference when the URL is silent", () => {
    expect(resolveCached(undefined, "exclude")).toBe("exclude");
    expect(resolveCached(null, "all")).toBe("all");
    expect(resolveCached("", "only")).toBe("only");
  });

  // A typo in a pasted URL should show the page, not a 400 from the server.
  it("falls back to the preference for a junk URL value", () => {
    expect(resolveCached("banana", "exclude")).toBe("exclude");
    expect(resolveCached("Exclude", "all")).toBe("all");
  });
});
