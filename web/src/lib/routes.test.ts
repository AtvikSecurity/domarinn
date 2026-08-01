import { describe, expect, it } from "vitest";
import {
  comparePath,
  runPath,
  runsFilterHref,
  setsPath,
  suitePath,
} from "./routes";

/**
 * These are encoding tests, not string-formatting tests. A project or suite is
 * named by whoever wrote the suite YAML, so every one of these inputs is
 * reachable, and an unescaped `/` is the difference between a working link and
 * a route that silently resolves somewhere else.
 */
describe("route builders", () => {
  it("escapes a slash so a name cannot invent a path segment", () => {
    expect(setsPath("a/b")).toBe("/sets/a%2Fb");
    expect(suitePath("a/b", "c/d")).toBe("/sets/a%2Fb/c%2Fd");
  });

  it("escapes spaces, hashes and percent signs", () => {
    expect(setsPath("check out")).toBe("/sets/check%20out");
    // A bare `#` would truncate the URL at the fragment.
    expect(setsPath("a#b")).toBe("/sets/a%23b");
    // A bare `%` is an invalid escape, not a literal.
    expect(setsPath("100%")).toBe("/sets/100%25");
  });

  it("escapes non-ASCII names", () => {
    expect(setsPath("café")).toBe("/sets/caf%C3%A9");
  });

  it("leaves ordinary kebab-case names alone", () => {
    expect(suitePath("checkout-agent", "regression")).toBe(
      "/sets/checkout-agent/regression",
    );
  });

  it("builds run and compare paths base-first", () => {
    expect(runPath("abc123")).toBe("/runs/abc123");
    // Older run first: the server reads segment one as the base.
    expect(comparePath("run-11", "run-12")).toBe(
      "/runs/run-11/compare/run-12",
    );
  });

  it("encodes a space as %20, not +, in the runs filter query", () => {
    // `URLSearchParams` would produce `+` here. These URLs get shared by hand.
    expect(runsFilterHref("my project", "smoke test")).toBe(
      "/runs?project=my%20project&suite=smoke%20test&cached=all",
    );
  });

  it("asks the runs stream for cached runs too", () => {
    // Without this the stream hides fully-cached passing runs and its count
    // disagrees with the status surface the visitor just came from.
    expect(runsFilterHref("p", "s")).toContain("cached=all");
  });
});
