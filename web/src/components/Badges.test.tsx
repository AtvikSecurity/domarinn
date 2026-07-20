import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { StatusBadge } from "./StatusBadge";
import { PassRateBadge } from "./PassRateBadge";
import { Sparkline } from "./Sparkline";

describe("StatusBadge", () => {
  it("renders the human label for a status", () => {
    render(<StatusBadge status="fail" />);
    expect(screen.getByText("Fail")).toBeInTheDocument();
  });
});

describe("PassRateBadge", () => {
  it("shows the computed pass percentage", () => {
    render(<PassRateBadge pass={95} fail={4} error={1} />);
    expect(screen.getByText("95.0%")).toBeInTheDocument();
  });
});

describe("Sparkline", () => {
  it("draws a polyline path for the series", () => {
    const { container } = render(<Sparkline values={[0.5, 0.7, 0.6, 0.9]} />);
    const paths = container.querySelectorAll("path");
    // area fill + line stroke
    expect(paths.length).toBeGreaterThanOrEqual(2);
    expect(container.querySelector("svg")).toBeInTheDocument();
  });

  it("renders nothing drawable for an empty series", () => {
    const { container } = render(<Sparkline values={[]} />);
    expect(container.querySelector("path")).toBeNull();
  });
});
