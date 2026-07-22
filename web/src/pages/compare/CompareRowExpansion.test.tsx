import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { CompareRowExpansion } from "./CompareRowExpansion";
import { useCaseDetail } from "@/api/queries";

// Only `useCaseDetail` is used by the component; mock it so we can feed an
// output whose size crosses the 50k perf-guard threshold without touching the
// deterministic fixtures.
vi.mock("@/api/queries", () => ({ useCaseDetail: vi.fn() }));

const mockUseCaseDetail = vi.mocked(useCaseDetail);

function detail(output: string) {
  return {
    isPending: false,
    data: { output, asserts: [] },
  } as unknown as ReturnType<typeof useCaseDetail>;
}

function renderExpansion() {
  return render(
    <CompareRowExpansion
      baseRunId="base"
      headRunId="head"
      caseKey="case-0000"
      assertFlips={[]}
      mode="side"
      onModeChange={() => {}}
    />,
  );
}

describe("CompareRowExpansion perf guard", () => {
  beforeEach(() => {
    mockUseCaseDetail.mockReset();
  });

  it("forces the unified line diff and disables word-diff options for >50k output", async () => {
    mockUseCaseDetail.mockReturnValue(detail("x".repeat(50_001)));
    const { container } = renderExpansion();

    // The muted large-output notice appears next to the control.
    expect(
      screen.getByText("large output — unified diff"),
    ).toBeInTheDocument();

    // Side/Inline are locked out; Unified is the only usable option.
    expect(screen.getByRole("radio", { name: "Side" })).toBeDisabled();
    expect(screen.getByRole("radio", { name: "Inline" })).toBeDisabled();
    expect(screen.getByRole("radio", { name: "Unified" })).toBeEnabled();

    // Even though mode="side" was requested, rendering is forced to unified.
    await waitFor(() =>
      expect(container.querySelector('[data-diff-mode="lines"]')).not.toBeNull(),
    );
    expect(container.querySelector('[data-diff-mode="side"]')).toBeNull();
  });

  it("keeps the word-diff modes available for normal-sized output", async () => {
    mockUseCaseDetail.mockReturnValue(detail("hello world"));
    const { container } = renderExpansion();

    expect(
      screen.queryByText("large output — unified diff"),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "Side" })).toBeEnabled();
    expect(screen.getByRole("radio", { name: "Inline" })).toBeEnabled();

    // mode="side" is honoured (the two-column pane renders).
    await waitFor(() =>
      expect(container.querySelector('[data-diff-mode="side"]')).not.toBeNull(),
    );
  });
});
