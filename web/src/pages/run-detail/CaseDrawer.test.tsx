import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { CaseDrawer } from "./CaseDrawer";
import { useCaseDetail, useCaseHistory, useSuites } from "@/api/queries";

// The drawer body, its baseline-diff section, and its history section are the
// consumers of the query layer here; mock all three so we can drive
// suite/baseline shapes and the enabled-gating wiring directly, without the
// fixtures or a QueryClient. The history section is collapsed by default, so its
// hook only needs a benign disabled-query result here.
vi.mock("@/api/queries", () => ({
  useCaseDetail: vi.fn(),
  useCaseHistory: vi.fn(),
  useSuites: vi.fn(),
}));

const mockUseCaseDetail = vi.mocked(useCaseDetail);
const mockUseCaseHistory = vi.mocked(useCaseHistory);
const mockUseSuites = vi.mocked(useSuites);

/** A disabled/collapsed history query result — the drawer's History section is
 *  collapsed by default, so it never reads `data`. */
function idleHistory() {
  return {
    isPending: true,
    isError: false,
    data: undefined,
  } as unknown as ReturnType<typeof useCaseHistory>;
}

const RUN = "run-current";
const BASELINE = "run-baseline";
const CASE = "case-0001";

type CaseDetail = ReturnType<typeof useCaseDetail>;

function currentDetail(output = "current output alpha"): CaseDetail {
  return {
    isPending: false,
    isError: false,
    error: null,
    refetch: () => {},
    data: {
      cell: { provider_id: "openai", test_id: CASE, repeat: 0 },
      case_key: CASE,
      name: "handles empty cart",
      tags: [],
      status: "pass",
      score: 1,
      output,
      asserts: [],
      usage: { input_tokens: 10, output_tokens: 5 },
      cost_usd: 0.001,
      latency_ms: 120,
      cached: false,
      attempts: 1,
    },
  } as unknown as CaseDetail;
}

/** A baseline-side `useCaseDetail` result in one of the states the section
 *  branches on. */
function baselineDetail(
  state: "loading" | "error" | { output: string },
): CaseDetail {
  if (state === "loading") {
    return { isPending: true, isError: false, data: undefined } as unknown as CaseDetail;
  }
  if (state === "error") {
    return { isPending: false, isError: true, data: undefined } as unknown as CaseDetail;
  }
  return {
    isPending: false,
    isError: false,
    data: { output: state.output, asserts: [] },
  } as unknown as CaseDetail;
}

/** Point `useSuites` at a suite whose baseline is `baselineRunId` (or none). */
function setSuites(baselineRunId: string | null) {
  mockUseSuites.mockReturnValue({
    data: {
      project: "checkout-agent",
      suites: [
        {
          suite: "regression",
          run_count: 12,
          last_run_at: null,
          baseline_run_id: baselineRunId,
          series: [],
        },
      ],
    },
  } as unknown as ReturnType<typeof useSuites>);
}

/** Route `useCaseDetail` by run id: baseline id -> `baseline`, else current. */
function setCaseDetail(baseline: CaseDetail, current = currentDetail()) {
  mockUseCaseDetail.mockImplementation((id: string) =>
    id === BASELINE ? baseline : current,
  );
}

function renderDrawer() {
  return render(
    <MemoryRouter initialEntries={[`/runs/${RUN}?case=${CASE}`]}>
      <CaseDrawer
        runId={RUN}
        project="checkout-agent"
        suite="regression"
        caseKey={CASE}
        onClose={() => {}}
      />
    </MemoryRouter>,
  );
}

/** All `useCaseDetail` calls made for the baseline run id. */
function baselineCalls() {
  return mockUseCaseDetail.mock.calls.filter((c) => c[0] === BASELINE);
}

describe("CaseDrawer baseline-diff section", () => {
  beforeEach(() => {
    mockUseCaseDetail.mockReset();
    mockUseSuites.mockReset();
    mockUseCaseHistory.mockReset();
    mockUseCaseHistory.mockReturnValue(idleHistory());
  });

  it("hides the section when the suite has no pinned baseline", () => {
    setSuites(null);
    setCaseDetail(baselineDetail("loading"));
    renderDrawer();

    expect(screen.getByText("Output")).toBeInTheDocument();
    expect(screen.queryByText(/Diff vs baseline/)).not.toBeInTheDocument();
    // With no baseline, the baseline case must never be requested at all.
    expect(baselineCalls()).toHaveLength(0);
  });

  it("hides the section when this run *is* the baseline", () => {
    setSuites(RUN);
    setCaseDetail(baselineDetail("loading"));
    renderDrawer();

    expect(screen.queryByText(/Diff vs baseline/)).not.toBeInTheDocument();
  });

  it("fetches the baseline immediately (expanded by default); collapsing disables it", async () => {
    const user = userEvent.setup();
    setSuites(BASELINE);
    setCaseDetail(baselineDetail("loading"));
    renderDrawer();

    // Expanded from the first render: the baseline query fires right away.
    expect(baselineCalls().length).toBeGreaterThan(0);
    expect(baselineCalls().at(-1)?.[2]?.enabled).toBe(true);
    expect(screen.getByText("Loading baseline…")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /Diff vs baseline/ }));

    // Tucked away: the query is disabled again.
    expect(baselineCalls().at(-1)?.[2]?.enabled).toBe(false);
    expect(screen.queryByText("Loading baseline…")).not.toBeInTheDocument();
  });

  it("shows a not-in-baseline message on a baseline 404", () => {
    setSuites(BASELINE);
    setCaseDetail(baselineDetail("error"));
    renderDrawer();

    expect(
      screen.getByText("This case does not exist in the baseline run."),
    ).toBeInTheDocument();
  });

  it("shows the identical note when the outputs match", () => {
    setSuites(BASELINE);
    setCaseDetail(
      baselineDetail({ output: "same output" }),
      currentDetail("same output"),
    );
    renderDrawer();

    expect(screen.getByText("Output identical to baseline.")).toBeInTheDocument();
    // The identical note replaces the diff — no mode control is rendered.
    expect(
      screen.queryByRole("radiogroup", { name: "Diff mode" }),
    ).not.toBeInTheDocument();
  });

  it("renders the diff and a full-compare link when the outputs differ", () => {
    setSuites(BASELINE);
    setCaseDetail(
      baselineDetail({ output: "baseline output" }),
      currentDetail("current output"),
    );
    renderDrawer();

    expect(
      screen.getByRole("radiogroup", { name: "Diff mode" }),
    ).toBeInTheDocument();
    const link = screen.getByRole("link", { name: /Open full compare/ });
    expect(link).toHaveAttribute(
      "href",
      `/runs/${BASELINE}/compare/${RUN}?case=${CASE}`,
    );
  });
});

// ---------------------------------------------------------------------------
// The case-detail endpoint returns the *stored* CaseResult blob verbatim, and
// the runner serializes with `skip_serializing_if`, so keys like `tags` (empty
// vec), `cost_usd`, and `error` are simply absent — not null, not []. The
// drawer must render such a blob without throwing (regression: every untagged
// case crashed the whole route with "Cannot read properties of undefined").
// ---------------------------------------------------------------------------

describe("CaseDrawer with a lean stored blob", () => {
  beforeEach(() => {
    mockUseCaseDetail.mockReset();
    mockUseSuites.mockReset();
    mockUseCaseHistory.mockReset();
    mockUseCaseHistory.mockReturnValue(idleHistory());
    setSuites(null);
  });

  it("renders a blob whose serde-skipped keys (tags, cost_usd, error) are absent", () => {
    mockUseCaseDetail.mockReturnValue({
      isPending: false,
      isError: false,
      error: null,
      refetch: () => {},
      // Mirrors a real `GET /runs/{id}/cases/{key}` body for an untagged case:
      // no `tags`, `cost_usd`, or `error` keys at all.
      data: {
        cell: { provider_id: "qwen", prompt_id: "qa", test_id: "capital/germany", repeat: 0 },
        case_key: CASE,
        name: "capital/germany",
        status: "pass",
        score: 1,
        output: "Berlin",
        stop_reason: "stop",
        asserts: [
          {
            kind: "icontains",
            status: "pass",
            score: 1,
            weight: 1,
            reason: 'output contains "Berlin" (case-insensitive)',
            cached: false,
          },
        ],
        usage: { input_tokens: 26, output_tokens: 158 },
        latency_ms: 28,
        cached: true,
        attempts: 0,
      },
    } as unknown as CaseDetail);

    renderDrawer();

    expect(screen.getByText("Output")).toBeInTheDocument();
    expect(screen.getByText("Berlin")).toBeInTheDocument();
    expect(screen.getByText(/output contains "Berlin"/)).toBeInTheDocument();
  });
});

// ---------------------------------------------------------------------------
// Schema-v2 drawer sections: rendered prompt, stop_reason chip, raw metadata.
// The baseline is pinned to null throughout so the diff section stays hidden and
// only the v2 affordances are under test.
// ---------------------------------------------------------------------------

/** A current-case detail carrying whatever v2 fields the test needs. */
function v2Detail(fields: {
  prompt?: unknown;
  stop_reason?: string;
  raw?: unknown;
}): CaseDetail {
  return {
    isPending: false,
    isError: false,
    error: null,
    refetch: () => {},
    data: {
      cell: { provider_id: "openai", test_id: CASE, repeat: 0 },
      case_key: CASE,
      name: "handles empty cart",
      tags: [],
      status: "pass",
      score: 1,
      output: "current output alpha",
      asserts: [],
      usage: { input_tokens: 10, output_tokens: 5 },
      cost_usd: 0.001,
      latency_ms: 120,
      cached: false,
      attempts: 1,
      ...fields,
    },
  } as unknown as CaseDetail;
}

describe("CaseDrawer schema-v2 sections", () => {
  beforeEach(() => {
    mockUseCaseDetail.mockReset();
    mockUseSuites.mockReset();
    mockUseCaseHistory.mockReset();
    mockUseCaseHistory.mockReturnValue(idleHistory());
    setSuites(null);
  });

  it("renders a messages-style prompt as role chips in order, expanded by default", () => {
    mockUseCaseDetail.mockReturnValue(
      v2Detail({
        prompt: {
          messages: [
            { role: "system", content: "Follow the policy strictly." },
            { role: "user", content: "Please resolve the empty cart." },
          ],
        },
      }),
    );
    renderDrawer();

    const toggle = screen.getByRole("button", { name: /Prompt/ });
    // Header reflects the message count; the cards are visible immediately.
    expect(toggle).toHaveAccessibleName(/2 messages/);

    const sys = screen.getByText("system");
    const usr = screen.getByText("user");
    expect(sys).toBeInTheDocument();
    expect(usr).toBeInTheDocument();
    // system tinted with the accent; rendered before the user turn.
    expect(sys).toHaveClass("text-accent");
    expect(
      sys.compareDocumentPosition(usr) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    // Message content is rendered through the OutputViewer.
    expect(screen.getByText(/resolve the empty cart/)).toBeInTheDocument();
  });

  it("renders a text-style prompt with no role chips", () => {
    mockUseCaseDetail.mockReturnValue(
      v2Detail({ prompt: { text: "a single flattened prompt body" } }),
    );
    renderDrawer();

    const toggle = screen.getByRole("button", { name: /Prompt/ });
    // No message count on a text prompt; the body is visible immediately.
    expect(toggle).toHaveAccessibleName(/^Prompt$/);
    expect(screen.getByText(/single flattened prompt body/)).toBeInTheDocument();
    expect(screen.queryByText("system")).not.toBeInTheDocument();
    expect(screen.queryByText("user")).not.toBeInTheDocument();
  });

  it("shows an amber stop_reason chip for a truncated reason", () => {
    mockUseCaseDetail.mockReturnValue(v2Detail({ stop_reason: "max_tokens" }));
    renderDrawer();

    const chip = screen.getByText("max_tokens");
    expect(chip).toHaveAttribute("title", "Provider stop reason");
    expect(chip).toHaveClass("text-amber");
  });

  it("shows a muted stop_reason chip for a clean end_turn", () => {
    mockUseCaseDetail.mockReturnValue(v2Detail({ stop_reason: "end_turn" }));
    renderDrawer();

    const chip = screen.getByText("end_turn");
    expect(chip).toHaveAttribute("title", "Provider stop reason");
    expect(chip).toHaveClass("text-muted");
    expect(chip).not.toHaveClass("text-amber");
  });

  it("renders the raw provider-metadata section only when raw is present, as a JSON tree", () => {
    mockUseCaseDetail.mockReturnValue(
      v2Detail({ raw: { model: "gpt-4o-mini", finish_reason: "end_turn" } }),
    );
    renderDrawer();

    // Expanded by default: the JsonTree (keys + expand/collapse controls) is
    // visible without clicking the section header.
    expect(
      screen.getByRole("button", { name: /Provider metadata/ }),
    ).toBeInTheDocument();
    expect(screen.getByText(/"model"/)).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Collapse all" }),
    ).toBeInTheDocument();
  });

  it("renders none of the v2 affordances for a v1 case", () => {
    mockUseCaseDetail.mockReturnValue(v2Detail({}));
    renderDrawer();

    // The pre-v2 drawer is intact...
    expect(screen.getByText("Output")).toBeInTheDocument();
    // ...with no prompt section, stop_reason chip, or provider-metadata section.
    expect(
      screen.queryByRole("button", { name: /Prompt/ }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /Provider metadata/ }),
    ).not.toBeInTheDocument();
    expect(screen.queryByTitle("Provider stop reason")).not.toBeInTheDocument();
  });
});
