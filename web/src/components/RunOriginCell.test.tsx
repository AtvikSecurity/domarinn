import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import type { RunListItem } from "@/api";
import { TooltipProvider } from "./ui/Tooltip";
import { isCiRun, RunOriginCell } from "./RunOriginCell";

function run(overrides: Partial<RunListItem> = {}): RunListItem {
  return {
    id: "r-1",
    project: "p",
    suite: "s",
    created_at: "2026-01-01T00:00:00+00:00",
    git_branch: "main",
    git_commit: "abc1234",
    git_dirty: false,
    case_count: 1,
    pass_count: 1,
    fail_count: 0,
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
    ci_provider: null,
    ci_run_url: null,
    note: null,
    domarinn_version: null,
    tags: [],
    ...overrides,
  };
}

describe("isCiRun", () => {
  // `ci_provider` is the exact signal — CI detection returns a provider even
  // for a bare `CI` env var — which is why there is no second boolean.
  it("is true for any recorded provider and false otherwise", () => {
    expect(isCiRun({ ci_provider: "github" })).toBe(true);
    expect(isCiRun({ ci_provider: "ci" })).toBe(true);
    expect(isCiRun({ ci_provider: null })).toBe(false);
  });
});

/** The shell supplies the provider in the real app; mirror it here. */
function renderCell(r: RunListItem) {
  return render(
    <TooltipProvider>
      <RunOriginCell run={r} />
    </TooltipProvider>,
  );
}

describe("RunOriginCell", () => {
  it("labels a CI run and shows its actor", () => {
    renderCell(run({ ci_provider: "github", actor: "alice" }));
    expect(screen.getByText("CI")).toBeInTheDocument();
    expect(screen.getByText("alice")).toBeInTheDocument();
  });

  it("labels a developer run as local", () => {
    renderCell(run({ actor: "bob" }));
    expect(screen.getByText("local")).toBeInTheDocument();
    expect(screen.getByText("bob")).toBeInTheDocument();
  });

  // A shared CI token names nobody, so the run still has to attribute itself
  // to whoever the client recorded.
  it("falls back to the uploader when no actor was recorded", () => {
    renderCell(run({ uploaded_by: "ci-token" }));
    expect(screen.getByText("ci-token")).toBeInTheDocument();
  });

  // Runs from clients that predate provenance know nothing about themselves;
  // an em dash is honest where a blank cell looks like a rendering bug.
  it("renders a placeholder when nothing is known", () => {
    renderCell(run());
    expect(screen.getByText("—")).toBeInTheDocument();
  });
});
