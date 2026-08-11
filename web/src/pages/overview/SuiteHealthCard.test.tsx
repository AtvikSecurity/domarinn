import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import type { RunListItem } from "@/api";
import { SuiteHealthCard } from "./SuiteHealthCard";

const NOW = Date.parse("2026-07-27T12:00:00Z");
const DAY = 24 * 60 * 60 * 1000;

function run(o: Partial<RunListItem> & { id: string }): RunListItem {
  return {
    project: "p",
    suite: "s",
    created_at: new Date(NOW).toISOString(),
    git_branch: "main",
    git_commit: null,
    git_dirty: null,
    case_count: 10,
    pass_count: 10,
    fail_count: 0,
    xfail_count: 0,
    xpass_count: 0,
    error_count: 0,
    pass_rate: 1,
    prompt_tokens: 0,
    completion_tokens: 0,
    cost_usd: null,
    duration_ms: 0,
    cache_hits: null,
    cache_misses: null,
    actor: null,
    host: null,
    uploaded_by: null,
    ci_provider: "github",
    ci_run_url: null,
    note: null,
    domarinn_version: null,
    tags: [],
    ...o,
  };
}

function edgeOf(runs: RunListItem[]): DOMTokenList {
  render(
    <MemoryRouter>
      <SuiteHealthCard project="p" suite="s" to="/sets/p/s" runs={runs} now={NOW} />
    </MemoryRouter>,
  );
  return screen.getByTestId("suite-health-card").classList;
}

/**
 * The edge is the card's whole signal at a glance, so what it may say — and
 * what it must stay quiet about — is behaviour, not styling.
 */
describe("SuiteHealthCard severity edge", () => {
  it("keeps the default frame for a healthy suite", () => {
    const edge = edgeOf([run({ id: "r1" })]);
    expect(edge).toContain("border-chrome-border");
    expect([...edge].some((c) => c.startsWith("border-fail"))).toBe(false);
    expect([...edge].some((c) => c.startsWith("border-amber"))).toBe(false);
  });

  // A suite whose only runs are local has no CI result to report yet. Painting
  // that in a status colour claims a verdict nothing produced — and on a fresh
  // server it is every card on the page.
  it("keeps the default frame when no CI run has happened yet", () => {
    const edge = edgeOf([run({ id: "r1", ci_provider: null })]);
    expect(edge).toContain("border-chrome-border");
    expect([...edge].some((c) => c.startsWith("border-skip"))).toBe(false);
    expect([...edge].some((c) => c.startsWith("border-amber"))).toBe(false);
    expect([...edge].some((c) => c.startsWith("border-fail"))).toBe(false);
  });

  it("takes the fail tone when the canonical run has failures", () => {
    const edge = edgeOf([run({ id: "r1", fail_count: 2, pass_count: 8 })]);
    expect(edge).toContain("border-fail/50");
  });

  it("takes the amber tone when CI has gone quiet", () => {
    const edge = edgeOf([
      run({ id: "r1", created_at: new Date(NOW - 30 * DAY).toISOString() }),
      run({ id: "r2", created_at: new Date(NOW - 31 * DAY).toISOString() }),
      run({ id: "r3", created_at: new Date(NOW - 32 * DAY).toISOString() }),
    ]);
    expect(edge).toContain("border-amber/50");
  });
});
