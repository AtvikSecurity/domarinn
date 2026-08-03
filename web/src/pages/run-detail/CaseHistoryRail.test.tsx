import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { CaseHistoryRail } from "./CaseHistoryRail";
import { useCaseHistory } from "@/api/queries";
import { TooltipProvider } from "@/components/ui/Tooltip";
import type { CaseHistoryPoint, CaseHistoryResponse } from "@/api";

// The section's only query-layer consumer is `useCaseHistory`; mock it so we can
// drive the payload (and inspect the `enabled` gate) without the fixtures.
vi.mock("@/api/queries", () => ({ useCaseHistory: vi.fn() }));

const mockUseCaseHistory = vi.mocked(useCaseHistory);

const PROJECT = "checkout-agent";
const SUITE = "regression";
const CURRENT = "run-03";
const BASELINE = "run-02";
const CASE = "case-0024";

type HistoryResult = ReturnType<typeof useCaseHistory>;

/** One newest-first history point; only the fields the section reads matter. */
function point(over: Partial<CaseHistoryPoint> & { run_id: string }): CaseHistoryPoint {
  return {
    created_at: "2026-07-19T15:00:00.000Z",
    status: "pass",
    score: 0.9,
    output_hash: "h",
    output_changed: null,
    cached: false,
    prompt_tokens: 10,
    completion_tokens: 5,
    cost_usd: 0.001,
    latency_ms: 120,
    git_commit: "abc1234",
    config_digest: "blake3:deadbeef",
    ...over,
  };
}

/** A loaded history payload, newest-first (as the server/mock return it). */
function loaded(points: CaseHistoryPoint[]): HistoryResult {
  const data: CaseHistoryResponse = {
    project: PROJECT,
    suite: SUITE,
    case_key: CASE,
    baseline_run_id: BASELINE,
    points,
  };
  return { isPending: false, isError: false, data } as unknown as HistoryResult;
}

function renderSection() {
  return render(
    <TooltipProvider>
      <MemoryRouter initialEntries={[`/runs/${CURRENT}?case=${CASE}`]}>
        <CaseHistoryRail
          project={PROJECT}
          suite={SUITE}
          runId={CURRENT}
          caseKey={CASE}
        />
      </MemoryRouter>
    </TooltipProvider>,
  );
}

/** The rendered squares, in DOM (left-to-right / oldest→newest) order. */
function squareRunIds(): string[] {
  return Array.from(document.querySelectorAll("[data-history-square]")).map(
    (el) => el.getAttribute("data-run-id") ?? "",
  );
}

describe("CaseHistoryRail", () => {
  beforeEach(() => {
    mockUseCaseHistory.mockReset();
  });

  it("fetches whenever the drawer is open, and keeps fetching when collapsed", async () => {
    const user = userEvent.setup();
    mockUseCaseHistory.mockReturnValue({
      isPending: true,
      isError: false,
      data: undefined,
    } as unknown as HistoryResult);
    renderSection();

    // Expanded from the first render: the window query fires right away.
    const toggle = screen.getByRole("button", { name: "History" });
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    const firstCall = mockUseCaseHistory.mock.calls.at(-1)!;
    expect(firstCall[3]?.enabled).toBe(true);
    expect(firstCall[3]?.limit).toBe(20);

    await user.click(toggle);

    // Collapsing hides the rail, but the query stays enabled: the rail now
    // lives in the drawer's fixed strip rather than in the scrolling body, so
    // gating the fetch on expansion would defeat having it above the fold.
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(squareRunIds()).toHaveLength(0);
    const lastCall = mockUseCaseHistory.mock.calls.at(-1)!;
    expect(lastCall[3]?.enabled).toBe(true);
  });

  it("renders squares oldest→newest, reversing the newest-first payload", () => {
    // Payload newest-first: run-03 (current) → run-02 → run-01.
    mockUseCaseHistory.mockReturnValue(
      loaded([
        point({ run_id: "run-03" }),
        point({ run_id: "run-02" }),
        point({ run_id: "run-01" }),
      ]),
    );
    renderSection();

    // Displayed left-to-right oldest→newest — the reverse of the payload.
    expect(squareRunIds()).toEqual(["run-01", "run-02", "run-03"]);
  });

  it("ring-highlights the current run's square and marks the baseline", () => {
    mockUseCaseHistory.mockReturnValue(
      loaded([
        point({ run_id: CURRENT }),
        point({ run_id: BASELINE }),
        point({ run_id: "run-01" }),
      ]),
    );
    renderSection();

    // Exactly one square is flagged current, and it is the drawer's run.
    const current = document.querySelectorAll('[data-history-square][data-current="true"]');
    expect(current).toHaveLength(1);
    expect(current[0]!.getAttribute("data-run-id")).toBe(CURRENT);

    // Exactly one baseline underline, under the baseline run's column.
    const baselineMarks = document.querySelectorAll('[data-baseline="true"]');
    expect(baselineMarks).toHaveLength(1);
    const column = baselineMarks[0]!.closest("div.flex.flex-col") as HTMLElement;
    expect(
      within(column).getByRole("link").getAttribute("data-run-id"),
    ).toBe(BASELINE);
  });

  it("each square deep-links to the same case in that run", () => {
    mockUseCaseHistory.mockReturnValue(
      loaded([point({ run_id: CURRENT }), point({ run_id: BASELINE })]),
    );
    renderSection();

    const baselineLink = document.querySelector(
      `[data-history-square][data-run-id="${BASELINE}"]`,
    )!;
    expect(baselineLink.getAttribute("href")).toBe(
      `/runs/${BASELINE}?case=${CASE}`,
    );
  });

  it("shows a changed marker only under runs whose output_changed is true", () => {
    // newest-first: run-03 changed, run-02 unchanged, run-01 oldest (null).
    mockUseCaseHistory.mockReturnValue(
      loaded([
        point({ run_id: "run-03", output_changed: true }),
        point({ run_id: "run-02", output_changed: false }),
        point({ run_id: "run-01", output_changed: null }),
      ]),
    );
    renderSection();

    const markers = document.querySelectorAll("[data-output-changed]");
    expect(markers).toHaveLength(1);
    // The marker sits in run-03's column, not run-02's or the oldest's.
    const column = markers[0]!.closest("div.flex.flex-col") as HTMLElement;
    expect(
      within(column).getByRole("link").getAttribute("data-run-id"),
    ).toBe("run-03");
  });

  it("renders the score sparkline only when ≥2 points carry a score", () => {

    // Only one scored point (the other is null) — no sparkline.
    mockUseCaseHistory.mockReturnValue(
      loaded([
        point({ run_id: "run-02", score: 0.8 }),
        point({ run_id: "run-01", score: null }),
      ]),
    );
    const { unmount } = renderSection();
    expect(screen.queryByRole("img", { name: "Score trend" })).not.toBeInTheDocument();
    unmount();

    // Two scored points — the sparkline renders.
    mockUseCaseHistory.mockReturnValue(
      loaded([
        point({ run_id: "run-02", score: 0.9 }),
        point({ run_id: "run-01", score: 0.5 }),
      ]),
    );
    renderSection();
    expect(screen.getByRole("img", { name: "Score trend" })).toBeInTheDocument();
  });

  it("summarizes the window as N runs · M output changes", () => {
    mockUseCaseHistory.mockReturnValue(
      loaded([
        point({ run_id: "run-03", output_changed: true }),
        point({ run_id: "run-02", output_changed: true }),
        point({ run_id: "run-01", output_changed: null }),
      ]),
    );
    renderSection();

    // The summary rides on the section header, so it is readable while the
    // rail is collapsed too.
    expect(
      screen.getByRole("button", { name: /History/ }),
    ).toHaveAccessibleName(/3 runs · 2 changes/);
  });

  it("shows a muted message on error", () => {
    mockUseCaseHistory.mockReturnValue({
      isPending: false,
      isError: true,
      data: undefined,
    } as unknown as HistoryResult);
    renderSection();

    expect(screen.getByText("Case history is unavailable.")).toBeInTheDocument();
    expect(squareRunIds()).toHaveLength(0);
  });
});

describe("CaseHistoryRail replayed streaks", () => {
  beforeEach(() => {
    mockUseCaseHistory.mockReset();
  });

  /** Newest-first, as the server returns it. */
  const replayed = (id: string) => point({ run_id: id, cached: true });
  const fresh = (id: string) => point({ run_id: id, cached: false });

  it("folds a run of replayed runs into one counted marker", () => {
    // Newest-first: the current run, then four replays, then a fresh run.
    mockUseCaseHistory.mockReturnValue(
      loaded([
        fresh(CURRENT),
        replayed("run-r4"),
        replayed("run-r3"),
        replayed("run-r2"),
        replayed("run-r1"),
        fresh("run-old"),
      ]),
    );
    renderSection();

    const streak = document.querySelector("[data-replayed-streak]");
    expect(streak).not.toBeNull();
    expect(streak?.getAttribute("data-count")).toBe("4");

    // The four folded runs are gone from the square row; the two measured ones
    // and the baseline remain.
    expect(squareRunIds()).not.toContain("run-r2");
    expect(squareRunIds()).toContain(CURRENT);
    expect(squareRunIds()).toContain("run-old");
  });

  it("restores every folded run when the marker is selected", async () => {
    const user = userEvent.setup();
    mockUseCaseHistory.mockReturnValue(
      loaded([
        fresh(CURRENT),
        replayed("run-r3"),
        replayed("run-r2"),
        replayed("run-r1"),
      ]),
    );
    renderSection();

    await user.click(screen.getByRole("button", { name: /3 replayed runs/ }));

    expect(document.querySelector("[data-replayed-streak]")).toBeNull();
    for (const id of ["run-r1", "run-r2", "run-r3"]) {
      expect(squareRunIds()).toContain(id);
    }
  });

  // Folding away the run the drawer is open on would hide the current-run ring,
  // which is how a reader locates themselves on the timeline.
  it("never folds a streak containing the run being viewed", () => {
    mockUseCaseHistory.mockReturnValue(
      loaded([
        replayed(CURRENT),
        replayed("run-r2"),
        replayed("run-r1"),
        fresh("run-old"),
      ]),
    );
    renderSection();

    expect(document.querySelector("[data-replayed-streak]")).toBeNull();
    expect(squareRunIds()).toContain(CURRENT);
  });

  it("leaves a window of freshly measured runs untouched", () => {
    mockUseCaseHistory.mockReturnValue(
      loaded([fresh(CURRENT), fresh("run-b"), fresh("run-a")]),
    );
    renderSection();

    expect(document.querySelector("[data-replayed-streak]")).toBeNull();
    expect(squareRunIds()).toHaveLength(3);
  });
});
