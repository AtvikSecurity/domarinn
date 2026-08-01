import { describe, expect, it } from "vitest";
import type { ProjectSetView } from "@/api";
import { rankProjects } from "./setSearch";

function project(name: string, runCount = 1): ProjectSetView {
  return {
    project: name,
    suite_count: 1,
    run_count: runCount,
    last_run_at: 0,
    pass_count: 0,
    fail_count: 0,
    error_count: 0,
    case_count: 0,
    recent_pass_rates: [],
    restricted: false,
    my_level: null,
  };
}

const names = (rows: ProjectSetView[]) => rows.map((r) => r.project);

describe("rankProjects", () => {
  it("returns nothing for an empty query", () => {
    // Otherwise a blank search box, or `/search?q=`, lists every project.
    expect(rankProjects([project("a"), project("b")], "", 5)).toEqual([]);
    expect(rankProjects([project("a")], "   ", 5)).toEqual([]);
  });

  it("matches a typed phrase against a kebab-case name", () => {
    const all = [project("checkout-agent")];
    expect(names(rankProjects(all, "checkout agent", 5))).toEqual([
      "checkout-agent",
    ]);
    expect(names(rankProjects(all, "CHECKOUT-AGENT", 5))).toEqual([
      "checkout-agent",
    ]);
  });

  it("ranks exact over prefix over word-start over substring", () => {
    const all = [
      project("reagent-scores"), // substring only
      project("agent"), // exact
      project("checkout-agent"), // starts a word
      project("agentic-flows"), // prefix
    ];
    expect(names(rankProjects(all, "agent", 10))).toEqual([
      "agent",
      "agentic-flows",
      "checkout-agent",
      "reagent-scores",
    ]);
  });

  it("breaks ties on run count, then name", () => {
    const all = [
      project("checkout-zulu", 2),
      project("checkout-alpha", 2),
      project("checkout-busy", 90),
    ];
    expect(names(rankProjects(all, "checkout", 10))).toEqual([
      "checkout-busy",
      "checkout-alpha",
      "checkout-zulu",
    ]);
  });

  it("drops non-matches and honours the limit", () => {
    const all = [project("alpha"), project("beta"), project("alpaca")];
    expect(names(rankProjects(all, "al", 10))).toEqual(["alpaca", "alpha"]);
    expect(rankProjects(all, "al", 1)).toHaveLength(1);
    expect(rankProjects(all, "zzz", 10)).toEqual([]);
  });
});
