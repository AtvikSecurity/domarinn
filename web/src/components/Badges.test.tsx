import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { StatusBadge } from "./StatusBadge";
import { PassRateBadge, RateBadge } from "./PassRateBadge";
import { ProviderBadge } from "./ProviderBadge";
import { Sparkline } from "./Sparkline";
import { TooltipProvider } from "./ui/Tooltip";

describe("StatusBadge", () => {
  it("renders the human label and dot in the matching outline tone", () => {
    render(<StatusBadge status="fail" />);
    const badge = screen.getByText("Fail");
    expect(badge).toHaveClass(
      "rounded-[8px]",
      "border-fail",
      "text-fail",
      "bg-transparent",
    );
    expect(badge).not.toHaveClass("rounded-full");
    expect(badge.querySelector("[aria-hidden]")).toBeInTheDocument();
  });
});

describe("PassRateBadge", () => {
  it("shows the computed pass percentage in the matching outline tone", () => {
    render(<PassRateBadge pass={95} fail={4} error={1} />);
    expect(screen.getByText("95.0%").parentElement).toHaveClass(
      "border-pass",
      "text-pass",
    );
  });

  it.each([
    [1, "100%"],
    [0.732, "73.2%"],
    [0, "0%"],
  ] as const)("fills the meter to the rate (%s)", (rate, width) => {
    // The width IS the value — the badge reads as a bar as well as a number,
    // so a meter stuck at a constant width would be worse than none at all.
    const { container } = render(<RateBadge rate={rate} />);
    const meter = container.querySelector<HTMLElement>(".absolute");
    expect(meter).not.toBeNull();
    expect(meter!.style.width).toBe(width);
    expect(meter).toHaveAttribute("aria-hidden");
  });

  it("draws no fill for an unknown rate", () => {
    // `null` is "no runs yet", not "zero percent" — the tone already goes
    // neutral, and a zero-width bar keeps it from reading as a hard failure.
    const { container } = render(<RateBadge rate={null} />);
    expect(container.querySelector<HTMLElement>(".absolute")!.style.width).toBe("0%");
  });

  it("keeps the meter behind the label and clipped to the pill", () => {
    const { container } = render(<RateBadge rate={0.5} />);
    const badge = screen.getByText("50.0%").parentElement!;
    expect(badge).toHaveClass("relative", "overflow-hidden");
    // Positioned boxes paint over static in-flow content whatever the DOM
    // order, so the label has to be positioned too or the meter covers it.
    expect(screen.getByText("50.0%")).toHaveClass("relative");
    expect(container.querySelector(".absolute")).toBeInTheDocument();
  });

  it.each([
    [0.95, "border-pass", "text-pass"],
    [0.8, "border-amber", "text-amber"],
    [0.79, "border-fail", "text-fail"],
    [null, "border-border-strong", "text-muted"],
  ] as const)("maps %s to the expected outline tone", (rate, border, text) => {
    render(<RateBadge rate={rate} />);
    expect(
      screen.getByText(rate === null ? "-" : `${(rate * 100).toFixed(1)}%`).parentElement,
    ).toHaveClass(border, text);
  });
});

describe("ProviderBadge", () => {
  it("stays a focusable tooltip trigger with the outline recipe", () => {
    render(
      <TooltipProvider>
        <ProviderBadge
          identity={{
            provider: "oidc:google",
            kind: "oidc",
            subject: "user-123",
            email: "dev@example.com",
            last_login_at: null,
          }}
        />
      </TooltipProvider>,
    );

    expect(screen.getByText("Google")).toHaveAttribute("tabindex", "0");
    expect(screen.getByText("Google")).toHaveClass(
      "rounded-[8px]",
      "border-border-strong",
      "bg-transparent",
      "focus-visible:ring-2",
    );
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

  it("renders a single point as a dot, not an invisible one-command path", () => {
    // A lone `M x y` path draws nothing, so a brand-new suite used to show an
    // empty box.
    const { container } = render(<Sparkline values={[0.42]} />);
    expect(container.querySelector("circle")).toBeInTheDocument();
    expect(container.querySelector("path")).toBeNull();
  });

  it("does not claim a trend from a single point", () => {
    const { container } = render(<Sparkline values={[0.42]} />);
    // Neutral, not pass-green: one point is not "up".
    expect(container.querySelector("circle")).toHaveAttribute(
      "fill",
      "var(--color-skip)",
    );
  });

  it("centres an all-equal series instead of pinning it to the floor", () => {
    // A 100%-passing suite is the common case here; on the floor it reads as 0%.
    const { container } = render(
      <Sparkline values={[1, 1, 1, 1]} height={26} />,
    );
    const line = container.querySelectorAll("path")[1] ?? container.querySelector("path");
    // height 26, pad 2 -> inner height 22 -> centre y = 2 + 11 = 13
    expect(line?.getAttribute("d")).toContain("13.0");
  });

  it("inverts the trend colour when lower is better", () => {
    const rising = [100, 200, 400];
    const good = render(<Sparkline values={rising} />);
    expect(good.container.querySelector("circle")).toHaveAttribute(
      "fill",
      "var(--color-pass)",
    );
    good.unmount();

    // Same rising series, but it is latency: rising is a regression.
    const bad = render(<Sparkline values={rising} higherIsBetter={false} />);
    expect(bad.container.querySelector("circle")).toHaveAttribute(
      "fill",
      "var(--color-fail)",
    );
  });

  it("breaks the line at a null instead of dropping the slot", () => {
    const { container } = render(<Sparkline values={[0.9, null, 0.5]} />);
    const d = container.querySelector("path")?.getAttribute("d") ?? "";
    // Two subpaths -> two move commands, and no area fill under a gapped line.
    expect(d.match(/M/g)).toHaveLength(2);
    expect(container.querySelectorAll("path")).toHaveLength(1);
  });

  it("keeps x-positions tied to the index when a value is missing", () => {
    // The last point must sit at the far right even though the middle is null,
    // so the chart still lines up with anything rendered above it.
    const { container } = render(
      <Sparkline values={[0.9, null, 0.5]} width={96} />,
    );
    // width 96, pad 2 -> inner width 92, step 46 -> last x = 2 + 92 = 94
    expect(container.querySelector("circle")).toHaveAttribute("cx", "94");
  });
});
