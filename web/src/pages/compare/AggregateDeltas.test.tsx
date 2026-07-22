import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { AggregateDeltas, type AggregateStatsInput } from "./AggregateDeltas";

const MINUS = "−";

const base: AggregateStatsInput = {
  prompt_tokens: 1000,
  completion_tokens: 1000,
  cost_usd: 0.1,
  duration_ms: 5000,
  case_count: 100,
};

describe("AggregateDeltas", () => {
  it("shows signed, pass-tinted deltas when tokens/cost/duration shrink", () => {
    const head: AggregateStatsInput = {
      prompt_tokens: 400,
      completion_tokens: 600, // 1000 total, 1000 fewer than base's 2000
      cost_usd: 0.05, // -$0.05
      duration_ms: 4000, // -1.0s
      case_count: 100,
    };
    render(<AggregateDeltas base={base} head={head} />);

    expect(screen.getByText(`${MINUS}1.0k`)).toHaveClass("text-pass");
    expect(screen.getByText(`${MINUS}$0.05`)).toHaveClass("text-pass");
    expect(screen.getByText(`${MINUS}1.0s`)).toHaveClass("text-pass");
  });

  it("marks a worse (larger) token delta with a + sign and the fail tone", () => {
    const head: AggregateStatsInput = {
      ...base,
      prompt_tokens: 1500, // +500 tokens vs base
    };
    render(<AggregateDeltas base={base} head={head} />);

    expect(screen.getByText("+500")).toHaveClass("text-fail");
  });

  it("renders an unchanged metric as a muted zero (no sign)", () => {
    render(<AggregateDeltas base={base} head={base} />);

    // Both the tokens and case-count deltas format to a bare "0" when equal.
    const zeros = screen.getAllByText("0");
    expect(zeros.length).toBeGreaterThan(0);
    for (const z of zeros) expect(z).toHaveClass("text-muted");
  });
});
