import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createRef } from "react";
import { Button } from "./Button";

describe("Button variants", () => {
  it.each([
    ["primary", "btn-primary"],
    ["secondary", "btn-outline"],
    ["ghost", "btn-ghost"],
    ["danger", "btn-danger"],
  ] as const)("renders %s through the shared recipe", (variant, expected) => {
    render(<Button variant={variant}>Go</Button>);
    expect(screen.getByRole("button", { name: "Go" })).toHaveClass(expected);
  });

  it("defaults to the quiet outline", () => {
    render(<Button>Go</Button>);
    expect(screen.getByRole("button", { name: "Go" })).toHaveClass("btn-outline");
  });

  it("weights the two filled variants heavier than the quiet ones", () => {
    // The design system sets primary/danger at 600 and outline/ghost at 500;
    // without it a filled button reads no louder than a ghost beside it.
    const { unmount } = render(<Button variant="primary">Go</Button>);
    expect(screen.getByRole("button", { name: "Go" })).toHaveClass("font-semibold");
    unmount();

    render(<Button variant="ghost">Go</Button>);
    expect(screen.getByRole("button", { name: "Go" })).toHaveClass("font-medium");
  });

  it("carries a border width so each variant's border-color can paint", () => {
    // The recipe sets only `border-color`. Drop the width from the base and
    // every button silently loses its hairline.
    render(<Button variant="primary">Go</Button>);
    expect(screen.getByRole("button", { name: "Go" })).toHaveClass("border");
  });

  it("transitions shadow and transform, not just colours", () => {
    // `transition-colors` covers neither the inset highlight nor the half-pixel
    // press, so both would snap rather than ease.
    render(<Button>Go</Button>);
    const button = screen.getByRole("button", { name: "Go" });
    expect(button).toHaveClass("transition");
    expect(button).not.toHaveClass("transition-colors");
  });
});

describe("Button sizing", () => {
  // Sizes were explicitly held back from the design-system migration, so they
  // are pinned rather than assumed: the recipe brings its own heights upstream
  // and it would be easy to pick them up by accident later.
  it.each([
    ["sm", "h-7"],
    ["md", "h-9"],
  ] as const)("keeps the existing %s height", (size, height) => {
    render(<Button size={size}>Go</Button>);
    expect(screen.getByRole("button", { name: "Go" })).toHaveClass(height);
  });

  it("defaults to md", () => {
    render(<Button>Go</Button>);
    expect(screen.getByRole("button", { name: "Go" })).toHaveClass("h-9");
  });
});

describe("Button behaviour", () => {
  it("forwards native attributes, a ref, and extra classes", () => {
    const ref = createRef<HTMLButtonElement>();
    render(
      <Button ref={ref} type="submit" aria-label="Save draft" className="mt-3">
        Save
      </Button>,
    );
    const button = screen.getByRole("button", { name: "Save draft" });
    expect(ref.current).toBe(button);
    expect(button).toHaveAttribute("type", "submit");
    expect(button).toHaveClass("mt-3", "btn-outline");
  });

  it("does not fire while disabled", async () => {
    const onClick = vi.fn();
    const user = userEvent.setup();
    render(
      <Button disabled onClick={onClick}>
        Go
      </Button>,
    );

    const button = screen.getByRole("button", { name: "Go" });
    expect(button).toBeDisabled();
    expect(button).toHaveClass("disabled:opacity-50");
    await user.click(button);
    expect(onClick).not.toHaveBeenCalled();
  });
});
