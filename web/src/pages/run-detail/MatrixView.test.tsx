import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MatrixView } from "./MatrixView";
import { useCaseDetail, useMatrixAll } from "@/api/queries";
import type { MatrixCell, MatrixResponse } from "@/api";

// The matrix view and its popover/compare children are the only query-layer
// consumers here; mock both hooks so we can drive matrix + case shapes directly,
// without fixtures or a QueryClient (mirrors CaseDrawer.test.tsx).
vi.mock("@/api/queries", () => ({
  useMatrixAll: vi.fn(),
  useCaseDetail: vi.fn(),
}));

const mockUseMatrixAll = vi.mocked(useMatrixAll);
const mockUseCaseDetail = vi.mocked(useCaseDetail);

const RUN = "run-x";

// Radix Popover uses pointer capture + floating-ui measurement, neither of which
// jsdom implements. Stub the members it touches.
beforeAll(() => {
  Element.prototype.hasPointerCapture = () => false;
  Element.prototype.setPointerCapture = () => {};
  Element.prototype.releasePointerCapture = () => {};
  Element.prototype.scrollIntoView = () => {};
  if (!("ResizeObserver" in globalThis)) {
    globalThis.ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
});

function cell(partial: Partial<MatrixCell>): MatrixCell {
  return {
    total: 2,
    passed: 0,
    failed: 0,
    errored: 0,
    skipped: 0,
    xfailed: 0,
    xpassed: 0,
    score_mean: null,
    pass_fraction: 0,
    distinct_outputs: 1,
    latency_ms_mean: null,
    cost_usd: null,
    fallback_answered: 0,
    case_keys: [],
    ...partial,
  };
}

/** A single-prompt, two-provider matrix with a 1/2 cell, a fully-passing flake
 *  cell, a null cell, and a single-run cell. */
function singlePromptMatrix(): MatrixResponse {
  return {
    run_id: RUN,
    columns: [
      { provider_id: "prov-a", prompt_id: "p1" },
      { provider_id: "prov-b", prompt_id: "p1" },
    ],
    rows: [
      {
        test_id: "test-a",
        name: "Alpha case",
        cells: [
          cell({
            total: 2,
            passed: 1,
            failed: 1,
            pass_fraction: 0.5,
            score_mean: 0.6,
            latency_ms_mean: 110,
            case_keys: ["ca0", "ca1"],
          }),
          cell({
            total: 2,
            passed: 2,
            pass_fraction: 1,
            score_mean: 0.9,
            distinct_outputs: 2, // flake on a fully-passing cell
            case_keys: ["cb0", "cb1"],
          }),
        ],
      },
      {
        test_id: "test-b",
        name: "Beta case",
        cells: [
          null, // never ran -> em-dash
          cell({ total: 1, passed: 1, pass_fraction: 1, case_keys: ["cx0"] }),
        ],
      },
    ],
    // The common case: one entry per column provider, all of them columns.
    provider_costs: [
      { provider_id: "prov-a", cases: 3, cost_usd: 0.42 },
      { provider_id: "prov-b", cases: 3, cost_usd: 0.11 },
    ],
    next_cursor: null,
  };
}

/**
 * The same matrix with a `fallback_only` answerer: `reserve` spent tokens on
 * two of prov-a's repeats, so it appears in `provider_costs` while forming no
 * column of its own.
 */
function fallbackMatrix(): MatrixResponse {
  const m = singlePromptMatrix();
  m.rows[0]!.cells[0] = cell({
    total: 2,
    passed: 1,
    failed: 1,
    pass_fraction: 0.5,
    score_mean: 0.6,
    fallback_answered: 1,
    case_keys: ["ca0", "ca1"],
  });
  m.provider_costs = [
    { provider_id: "prov-a", cases: 2, cost_usd: 0.4 },
    { provider_id: "prov-b", cases: 3, cost_usd: 0.11 },
    { provider_id: "reserve", cases: 1, cost_usd: 0.02 },
  ];
  return m;
}

/** A single-provider run: one column, one cost entry naming it. */
function singleProviderMatrix(): MatrixResponse {
  return {
    run_id: RUN,
    columns: [{ provider_id: "prov-a", prompt_id: null }],
    rows: [
      {
        test_id: "test-a",
        name: "Alpha case",
        cells: [cell({ total: 1, passed: 1, pass_fraction: 1, case_keys: ["cx0"] })],
      },
    ],
    provider_costs: [{ provider_id: "prov-a", cases: 1, cost_usd: 0.42 }],
    next_cursor: null,
  };
}

/** A two-prompt, two-provider matrix (exercises prompt-section headers). */
function twoPromptMatrix(): MatrixResponse {
  return {
    run_id: RUN,
    columns: [
      { provider_id: "prov-a", prompt_id: "baseline" },
      { provider_id: "prov-a", prompt_id: "cot" },
      { provider_id: "prov-b", prompt_id: "baseline" },
      { provider_id: "prov-b", prompt_id: "cot" },
    ],
    rows: [
      {
        test_id: "test-a",
        name: "Alpha case",
        cells: [
          cell({ passed: 2, pass_fraction: 1, case_keys: ["k0", "k1"] }),
          cell({ passed: 1, failed: 1, pass_fraction: 0.5, case_keys: ["k2", "k3"] }),
          cell({ passed: 2, pass_fraction: 1, case_keys: ["k4", "k5"] }),
          cell({ passed: 0, failed: 2, pass_fraction: 0, case_keys: ["k6", "k7"] }),
        ],
      },
    ],
    provider_costs: [
      { provider_id: "prov-a", cases: 4, cost_usd: 0.4 },
      { provider_id: "prov-b", cases: 4, cost_usd: 0.1 },
    ],
    next_cursor: null,
  };
}

function setMatrix(m: MatrixResponse) {
  mockUseMatrixAll.mockReturnValue({
    data: { pages: [m], pageParams: [undefined] },
    hasNextPage: false,
    isFetchingNextPage: false,
    fetchNextPage: vi.fn(),
    isPending: false,
    isError: false,
    error: null,
    refetch: vi.fn(),
  } as unknown as ReturnType<typeof useMatrixAll>);
}

const OUTPUTS: Record<
  string,
  { status: string; score: number; output: string; latency: number; answeredBy?: string }
> = {
  // Repeat #0 of prov-a's cell is the one a fallback answered.
  ca0: {
    status: "fail",
    score: 0.4,
    output: "alpha output from provider A",
    latency: 100,
    answeredBy: "reserve",
  },
  ca1: { status: "pass", score: 0.9, output: "second repeat A", latency: 120 },
  cb0: { status: "pass", score: 0.95, output: "alpha output from provider B", latency: 90 },
  cb1: { status: "pass", score: 0.9, output: "second repeat B", latency: 95 },
  cx0: { status: "pass", score: 0.95, output: "beta output", latency: 80 },
};

function setCaseDetail() {
  mockUseCaseDetail.mockImplementation((_id: string, caseKey: string | undefined) => {
    const o = caseKey ? OUTPUTS[caseKey] : undefined;
    if (!o) {
      return { isPending: true, isError: false, data: undefined } as unknown as ReturnType<
        typeof useCaseDetail
      >;
    }
    return {
      isPending: false,
      isError: false,
      error: null,
      refetch: vi.fn(),
      data: {
        cell: { provider_id: "p", test_id: "t", repeat: 0 },
        case_key: caseKey,
        name: caseKey,
        tags: [],
        status: o.status,
        score: o.score,
        output: o.output,
        asserts: [],
        cost_usd: 0.001,
        latency_ms: o.latency,
        cached: false,
        attempts: 1,
        ...(o.answeredBy ? { answered_by_provider_id: o.answeredBy } : {}),
      },
    } as unknown as ReturnType<typeof useCaseDetail>;
  });
}

function cellButton(testId: string, colIndex: number): HTMLElement {
  const el = document.querySelector(`[data-cell="${testId}:${colIndex}"]`);
  if (!el) throw new Error(`no cell ${testId}:${colIndex}`);
  return el as HTMLElement;
}

describe("MatrixView", () => {
  beforeEach(() => {
    mockUseMatrixAll.mockReset();
    mockUseCaseDetail.mockReset();
    setCaseDetail();
  });

  it("renders a table with provider column headers and pass@k cells", () => {
    setMatrix(singlePromptMatrix());
    render(<MatrixView runId={RUN} onSelectCase={() => {}} />);

    // Real table with column + row headers.
    expect(screen.getByRole("table")).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: "prov-a" })).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: "prov-b" })).toBeInTheDocument();
    expect(screen.getByRole("rowheader", { name: /Alpha case/ })).toBeInTheDocument();

    // A 1/2 fraction cell, a fully-passing flake cell (tilde), and an em-dash.
    expect(screen.getByText("1/2")).toBeInTheDocument();
    expect(screen.getByText("2/2")).toBeInTheDocument();
    expect(screen.getByText("~")).toBeInTheDocument();
    expect(screen.getByLabelText("no data")).toBeInTheDocument();

    // The 1/2 tile carries the half-fraction bucket background.
    expect(cellButton("test-a", 0).className).toContain("bg-amber/20");
  });

  it("groups columns under prompt-section headers when the run has >1 prompt", () => {
    setMatrix(twoPromptMatrix());
    render(<MatrixView runId={RUN} onSelectCase={() => {}} />);

    // Prompt spans on top...
    expect(screen.getByRole("columnheader", { name: "baseline" })).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: "cot" })).toBeInTheDocument();
    // ...providers beneath, one per (provider, prompt) column.
    expect(screen.getAllByRole("columnheader", { name: "prov-a" })).toHaveLength(2);
    expect(screen.getAllByRole("columnheader", { name: "prov-b" })).toHaveLength(2);
  });

  it("opens a cell popover whose repeat rows deep-link into the drawer", async () => {
    const user = userEvent.setup();
    const onSelectCase = vi.fn();
    setMatrix(singlePromptMatrix());
    render(<MatrixView runId={RUN} onSelectCase={onSelectCase} />);

    await user.click(cellButton("test-a", 0));

    // Popover shows one row per repeat (its status fetched via useCaseDetail).
    const row0 = await screen.findByRole("button", { name: /#0/ });
    expect(screen.getByRole("button", { name: /#1/ })).toBeInTheDocument();

    await user.click(row0);
    expect(onSelectCase).toHaveBeenCalledWith("ca0");
  });

  it("opens the provider-compare modal and toggles to a two-provider diff", async () => {
    const user = userEvent.setup();
    setMatrix(singlePromptMatrix());
    render(<MatrixView runId={RUN} onSelectCase={() => {}} />);

    await user.click(cellButton("test-a", 0));
    await user.click(
      await screen.findByRole("button", { name: /Compare across providers/ }),
    );

    const dialog = await screen.findByRole("dialog");
    // One panel per provider that ran the test.
    expect(within(dialog).getByText("prov-a")).toBeInTheDocument();
    expect(within(dialog).getByText("prov-b")).toBeInTheDocument();

    // Both providers produced output -> a two-way diff is offered.
    const toggle = await within(dialog).findByRole("radiogroup", {
      name: "Compare view",
    });
    await user.click(within(toggle).getByRole("radio", { name: "Diff" }));

    // The diff renders (async jsdiff import) with its side-by-side data hook.
    await waitFor(() =>
      expect(dialog.querySelector("[data-diff-mode]")).toBeInTheDocument(),
    );
  });
});

// ---------------------------------------------------------------------------
// Fallback attribution. `provider_costs` is keyed by the provider that
// ANSWERED, not by the column's configured one, so an entry can name a provider
// the grid never shows — and a cell can be filled by someone else's answers
// while still reading as its column's score.
// ---------------------------------------------------------------------------

describe("MatrixView fallback attribution", () => {
  beforeEach(() => {
    mockUseMatrixAll.mockReset();
    mockUseCaseDetail.mockReset();
    setCaseDetail();
  });

  it("lists per-provider spend and marks an answerer that is not a column", () => {
    setMatrix(fallbackMatrix());
    render(<MatrixView runId={RUN} onSelectCase={() => {}} />);

    expect(screen.getByText("Spend")).toBeInTheDocument();
    expect(screen.getByText("prov-a $0.40")).toBeInTheDocument();
    expect(screen.getByText("prov-b $0.11")).toBeInTheDocument();
    // The fallback-only provider is named AND marked: without the suffix the
    // line reads as a column total matching no column above it.
    expect(screen.getByText("reserve $0.02 (fallback)")).toBeInTheDocument();
    expect(
      screen.getByTitle("cost attributed to the provider that answered"),
    ).toBeInTheDocument();
  });

  it("shows the legend for a multi-provider run even with no fallback", () => {
    setMatrix(singlePromptMatrix());
    render(<MatrixView runId={RUN} onSelectCase={() => {}} />);

    expect(screen.getByText("prov-a $0.42")).toBeInTheDocument();
    expect(screen.queryByText(/\(fallback\)/)).not.toBeInTheDocument();
  });

  it("renders no legend for a single configured provider with a cost", () => {
    // The common case. One provider, one column, one bill — the legend would
    // only restate the column header above it.
    setMatrix(singleProviderMatrix());
    render(<MatrixView runId={RUN} onSelectCase={() => {}} />);

    expect(screen.queryByText("Spend")).not.toBeInTheDocument();
    expect(screen.queryByText(/\$0\.42/)).not.toBeInTheDocument();
  });

  it("counts a cell's fallback repeats in its popover and names the answerer", async () => {
    const user = userEvent.setup();
    setMatrix(fallbackMatrix());
    render(<MatrixView runId={RUN} onSelectCase={() => {}} />);

    await user.click(cellButton("test-a", 0));

    expect(
      await screen.findByText("1 of 2 answered by a fallback"),
    ).toBeInTheDocument();
    // ...and the repeat row that it refers to says who answered it.
    const row0 = screen.getByRole("button", { name: /#0/ });
    expect(within(row0).getByText("reserve")).toBeInTheDocument();
    expect(
      within(screen.getByRole("button", { name: /#1/ })).queryByText("reserve"),
    ).not.toBeInTheDocument();
  });

  it("says nothing about fallback on a cell no fallback answered", async () => {
    const user = userEvent.setup();
    setMatrix(singlePromptMatrix());
    render(<MatrixView runId={RUN} onSelectCase={() => {}} />);

    await user.click(cellButton("test-a", 1));

    expect(await screen.findByRole("button", { name: /#0/ })).toBeInTheDocument();
    expect(
      screen.queryByText(/answered by a fallback/),
    ).not.toBeInTheDocument();
  });
});
