import { createRef } from "react";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Card } from "./Card";
import { Chip } from "./Chip";
import { StatBlock } from "./StatBlock";

describe("Card", () => {
  it("renders the chrome frame with default caller padding", () => {
    render(<Card>Body</Card>);
    expect(screen.getByText("Body")).toHaveClass(
      "rounded-lg",
      "border-chrome-border",
      "bg-transparent",
      "p-4",
    );
  });

  it("keeps flush cards responsible for clipping instead of padding", () => {
    render(<Card padding="flush">Grid</Card>);
    const card = screen.getByText("Grid");
    expect(card).toHaveClass("overflow-hidden");
    expect(card).not.toHaveClass("p-4");
  });
});

describe("Chip", () => {
  it("renders a semantic outline label", () => {
    render(<Chip tone="fail">failed</Chip>);
    expect(screen.getByText("failed")).toHaveClass(
      "rounded-[8px]",
      "border-fail",
      "text-fail",
      "bg-transparent",
      "font-mono",
      "uppercase",
    );
  });

  it("allows case-sensitive labels to opt out of visual uppercasing", () => {
    render(<Chip className="normal-case">shaAbC</Chip>);
    expect(screen.getByText("shaAbC")).toHaveClass("normal-case");
    expect(screen.getByText("shaAbC")).not.toHaveClass("uppercase");
  });
});

describe("StatBlock", () => {
  it("uses chrome only for the boxed variant", () => {
    const { rerender } = render(<StatBlock label="Cost">$1.00</StatBlock>);
    expect(screen.getByText("$1.00").parentElement).toHaveClass(
      "rounded-lg",
      "border-chrome-border",
      "bg-transparent",
    );

    rerender(
      <StatBlock label="Cost" variant="bare">
        $1.00
      </StatBlock>,
    );
    expect(screen.getByText("$1.00").parentElement).not.toHaveClass("rounded-lg");
  });

  it("renders its existing label as an operator eyebrow", () => {
    render(<StatBlock label="Tokens">123</StatBlock>);
    expect(screen.getByText("Tokens")).toHaveClass(
      "font-mono",
      "text-[10px]",
      "uppercase",
      "tracking-[0.12em]",
    );
  });
});

describe("PillButton", () => {
  it("is a ref-forwarding native button with pressed state", async () => {
    const { PillButton } = await import("./PillButton");
    const ref = createRef<HTMLButtonElement>();
    render(
      <PillButton ref={ref} tone="amber" pressed disabled>
        Changed
      </PillButton>,
    );

    const button = screen.getByRole("button", { name: "Changed" });
    expect(button).toHaveAttribute("type", "button");
    expect(button).toHaveAttribute("aria-pressed", "true");
    expect(button).toHaveAttribute("data-pressed", "true");
    expect(button).toHaveClass("rounded-[8px]", "border-amber", "text-amber");
    expect(button).toBeDisabled();
    expect(ref.current).toBe(button);
  });
});
