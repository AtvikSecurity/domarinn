import { describe, expect, it } from "vitest";
import {
  activeCacheFilterCount,
  activeRunsFilterCount,
  cacheRequestFilters,
  mergeParams,
  mergeParamsResetting,
  parseCaseFilters,
  parseRunsFilters,
  pickParams,
  runsRequestFilters,
  RUNS_FILTER_KEYS,
  toggleValue,
} from "./filters";

const sp = (init: string) => new URLSearchParams(init);

describe("pickParams / parseRunsFilters", () => {
  it("picks known keys and ignores blanks + unknown keys", () => {
    const params = sp("project=alpha&suite=&tag=nightly&nope=1&status=fail");
    expect(parseRunsFilters(params)).toEqual({
      project: "alpha",
      tag: "nightly",
      status: "fail",
    });
  });

  it("treats whitespace-only values as absent", () => {
    expect(pickParams(sp("q=%20%20"), ["q"])).toEqual({});
  });

  it("parses case filters including the client-only case + sort keys", () => {
    expect(
      parseCaseFilters(sp("status=pass&case=case-0007&q=foo&sort=-latency")),
    ).toEqual({
      status: "pass",
      case: "case-0007",
      q: "foo",
      sort: "-latency",
    });
  });

  it("parses the provider + prompt server filters (Task 11)", () => {
    expect(
      parseCaseFilters(sp("provider=gpt-5-mini&prompt=cot-v2&status=fail")),
    ).toEqual({
      provider: "gpt-5-mini",
      prompt: "cot-v2",
      status: "fail",
    });
  });

  it("parses the cached key on both runs and case filters", () => {
    expect(parseRunsFilters(sp("cached=all"))).toEqual({ cached: "all" });
    expect(parseCaseFilters(sp("cached=false"))).toEqual({ cached: "false" });
  });

  it("counts an explicit cached value as an active runs filter", () => {
    expect(activeRunsFilterCount(sp("cached=all"))).toBe(1);
    expect(activeRunsFilterCount(sp(""))).toBe(0);
  });
});

describe("runsRequestFilters", () => {
  it("excludes cached runs when neither the URL nor a preference says otherwise", () => {
    expect(runsRequestFilters({})).toEqual({ cached: "exclude" });
    expect(runsRequestFilters({ project: "alpha" })).toEqual({
      project: "alpha",
      cached: "exclude",
    });
  });

  it("sends no cached param when the URL explicitly shows all", () => {
    expect(runsRequestFilters({ cached: "all" })).toEqual({});
  });

  it("passes only through", () => {
    expect(runsRequestFilters({ cached: "only" })).toEqual({ cached: "only" });
  });

  it("passes an explicit exclude through", () => {
    expect(runsRequestFilters({ cached: "exclude" })).toEqual({
      cached: "exclude",
    });
  });

  it("sanitizes junk cached values back to the fallback", () => {
    expect(runsRequestFilters({ cached: "banana" })).toEqual({
      cached: "exclude",
    });
  });

  // The second argument is the user's stored preference. A URL that says
  // nothing adopts it, which is how one setting reaches every surface.
  it("falls back to the caller's preference when the URL is silent", () => {
    expect(runsRequestFilters({}, "all")).toEqual({});
    expect(runsRequestFilters({}, "only")).toEqual({ cached: "only" });
    expect(runsRequestFilters({ project: "alpha" }, "all")).toEqual({
      project: "alpha",
    });
  });

  // Shared links must not change meaning based on who opens them.
  it("lets an explicit URL value beat the preference", () => {
    expect(runsRequestFilters({ cached: "exclude" }, "all")).toEqual({
      cached: "exclude",
    });
    expect(runsRequestFilters({ cached: "all" }, "only")).toEqual({});
  });

  it("sanitizes junk against the preference, not the shipped default", () => {
    expect(runsRequestFilters({ cached: "banana" }, "all")).toEqual({});
  });
});

describe("mergeParams", () => {
  it("sets, deletes on blank, and preserves untouched keys", () => {
    const next = mergeParams(sp("project=alpha&cursor=20"), {
      project: "beta",
      suite: "smoke",
      cursor: undefined,
    });
    expect(next.get("project")).toBe("beta");
    expect(next.get("suite")).toBe("smoke");
    expect(next.has("cursor")).toBe(false);
  });

  it("does not mutate the input", () => {
    const input = sp("project=alpha");
    const next = mergeParams(input, { project: "beta" });
    expect(input.get("project")).toBe("alpha");
    expect(next).not.toBe(input);
  });

  it("mergeParamsResetting clears reset keys before applying the patch", () => {
    const next = mergeParamsResetting(
      sp("project=alpha&cursor=40&suite=old"),
      { project: "beta" },
      ["cursor", "suite"],
    );
    expect(next.get("project")).toBe("beta");
    expect(next.has("cursor")).toBe(false);
    expect(next.has("suite")).toBe(false);
  });
});

describe("toggleValue", () => {
  it("sets a value when different and clears it when equal", () => {
    const set = toggleValue(sp(""), "status", "fail");
    expect(set.get("status")).toBe("fail");
    const cleared = toggleValue(set, "status", "fail");
    expect(cleared.has("status")).toBe(false);
  });
});

describe("activeRunsFilterCount", () => {
  it("counts only active filter keys", () => {
    expect(activeRunsFilterCount(sp("project=a&tag=b&cursor=20&case=x"))).toBe(2);
    expect(activeRunsFilterCount(sp(""))).toBe(0);
  });

  it("counts the origin and actor facets", () => {
    expect(activeRunsFilterCount(sp("origin=ci"))).toBe(1);
    expect(activeRunsFilterCount(sp("origin=local&actor=alice"))).toBe(2);
  });
});

describe("origin + actor facets", () => {
  it("parses both out of the URL", () => {
    expect(parseRunsFilters(sp("origin=ci&actor=alice"))).toEqual({
      origin: "ci",
      actor: "alice",
    });
  });

  // Both are SERVER filters: the runs list is cursor-paginated, so a
  // client-side origin filter would silently apply only to loaded pages and
  // read as "there are no CI runs" on a page that happens to hold none.
  it("passes both through to the request", () => {
    expect(runsRequestFilters({ origin: "ci", actor: "alice" })).toEqual({
      origin: "ci",
      actor: "alice",
      cached: "exclude",
    });
  });

  // The filter bar's clear-all is derived from RUNS_FILTER_KEYS; if a key is
  // counted but not listed there, the button offers to clear a filter it
  // cannot reach.
  it("every counted key is clearable", () => {
    const active = sp(
      RUNS_FILTER_KEYS.map((k) => `${k}=x`).join("&"),
    );
    expect(activeRunsFilterCount(active)).toBe(RUNS_FILTER_KEYS.length);
    const cleared = mergeParams(
      active,
      Object.fromEntries(RUNS_FILTER_KEYS.map((k) => [k, undefined])),
    );
    expect(activeRunsFilterCount(cleared)).toBe(0);
  });
});

describe("cacheRequestFilters", () => {
  it("defaults to newest first when the url says nothing", () => {
    expect(cacheRequestFilters({})).toEqual({ sort: "created", order: "desc" });
  });

  it("keeps sort in the request — it is a server key on this page", () => {
    const request = cacheRequestFilters({ sort: "-size", model: "gpt-4o" });
    expect(request.sort).toBe("size");
    expect(request.order).toBe("desc");
    expect(request.model).toBe("gpt-4o");
  });

  it("splits an ascending sort into column and order", () => {
    expect(cacheRequestFilters({ sort: "cost" })).toEqual({
      sort: "cost",
      order: "asc",
    });
  });

  it("falls back to the default for a column the server would reject", () => {
    // A junk value in a shared URL should render the page, not a 400.
    expect(cacheRequestFilters({ sort: "-bogus" })).toEqual({
      sort: "created",
      order: "desc",
    });
  });

  it("drops the drawer selection, which means nothing to the server", () => {
    const request = cacheRequestFilters({ entry: "sha256:abc", kind: "judge" });
    expect(request).not.toHaveProperty("entry");
    expect(request.kind).toBe("judge");
  });
});

describe("activeCacheFilterCount", () => {
  it("ignores tier, sort and the open drawer", () => {
    const sp = new URLSearchParams(
      "tier=local&sort=-size&entry=sha256:abc",
    );
    expect(activeCacheFilterCount(sp)).toBe(0);
  });

  it("counts real narrowings", () => {
    const sp = new URLSearchParams("kind=judge&model=gpt-4o&q=refund");
    expect(activeCacheFilterCount(sp)).toBe(3);
  });
});
