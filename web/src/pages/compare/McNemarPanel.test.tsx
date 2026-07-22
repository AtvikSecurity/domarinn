import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import type { CompareStats, WilsonView } from "@/api";
import { TooltipProvider } from "@/components/ui/Tooltip";
import { McNemarPanel } from "./McNemarPanel";

function wilson(passed: number, total: number, rate: number, lower: number, upper: number): WilsonView {
  return { passed, total, rate, lower, upper };
}

function stats(overrides: Partial<CompareStats["mcnemar"]>): CompareStats {
  return {
    mcnemar: {
      regressions: 5,
      fixes: 20,
      statistic: 8.643,
      significant: true,
      ...overrides,
    },
    base_pass_rate: wilson(50, 60, 0.833, 0.691, 0.922),
    head_pass_rate: wilson(55, 60, 0.9167, 0.813, 0.965),
  };
}

function renderPanel(s: CompareStats) {
  return render(
    <TooltipProvider>
      <McNemarPanel stats={s} />
    </TooltipProvider>,
  );
}

describe("McNemarPanel", () => {
  it("shows the regression/fix counts and the 2-dec statistic", () => {
    renderPanel(stats({ regressions: 5, fixes: 20, statistic: 8.643 }));
    expect(screen.getByText("5")).toBeInTheDocument();
    expect(screen.getByText("20")).toBeInTheDocument();
    // χ² rounded to two decimals.
    expect(screen.getByText("8.64")).toBeInTheDocument();
  });

  it("marks a significant fixes-dominant result with the pass tone", () => {
    renderPanel(stats({ regressions: 5, fixes: 20, significant: true }));
    const badge = screen.getByText("Statistically significant");
    expect(badge).toBeInTheDocument();
    expect(badge).toHaveClass("text-pass");
  });

  it("marks a significant regression-dominant result with the fail tone", () => {
    renderPanel(stats({ regressions: 30, fixes: 4, significant: true }));
    const badge = screen.getByText("Statistically significant");
    expect(badge).toHaveClass("text-fail");
  });

  it("renders a muted 'Not significant' badge when not significant", () => {
    renderPanel(stats({ significant: false }));
    const badge = screen.getByText("Not significant");
    expect(badge).toBeInTheDocument();
    expect(badge).toHaveClass("text-muted");
  });

  it("labels both Wilson CI bars as `rate% (lower–upper)`", () => {
    renderPanel(stats({}));
    // Base: 83.3% (69.1–92.2), Head: 91.7% (81.3–96.5).
    expect(screen.getByText("83.3% (69.1–92.2)")).toBeInTheDocument();
    expect(screen.getByText("91.7% (81.3–96.5)")).toBeInTheDocument();
    expect(screen.getByText("Base pass rate")).toBeInTheDocument();
    expect(screen.getByText("Head pass rate")).toBeInTheDocument();
  });
});
