import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { CodeSurface } from "./CodeSurface";

describe("CodeSurface", () => {
  it("names the payload and copies exactly what it was given", async () => {
    const writeText = vi.fn<(t: string) => Promise<void>>().mockResolvedValue();
    const user = userEvent.setup();
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });

    render(
      <CodeSurface label="yaml" copyValue={"a: 1\nb: 2"}>
        <span>body</span>
      </CodeSurface>,
    );

    expect(screen.getByText("yaml")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith("a: 1\nb: 2");
    // Not icon-only: this stands in for the viewer's own copy button, which
    // confirmed the copy in words.
    expect(await screen.findByText("Copied")).toBeInTheDocument();
  });

  it("omits the wrap toggle when no caller wired one", () => {
    // The only branch neither CodeBlock nor JsonTree exercises: both always
    // wire wrap, so a surface used for something unwrappable would otherwise
    // ship a toggle that silently does nothing.
    render(
      <CodeSurface label="json" copyValue="{}">
        <span>body</span>
      </CodeSurface>,
    );
    expect(screen.queryByRole("button", { name: "Wrap" })).toBeNull();
    expect(screen.getByRole("button", { name: "Copy" })).toBeInTheDocument();
  });

  it("reports wrap state and hands changes back to the caller", async () => {
    const onWrapChange = vi.fn();
    const user = userEvent.setup();
    render(
      <CodeSurface label="json" copyValue="{}" wrap onWrapChange={onWrapChange}>
        <span>body</span>
      </CodeSurface>,
    );

    const wrap = screen.getByRole("button", { name: "Wrap" });
    expect(wrap).toHaveAttribute("aria-pressed", "true");
    await user.click(wrap);
    expect(onWrapChange).toHaveBeenCalledWith(false);
  });

  it("scrolls horizontally only when wrapping is off", () => {
    const { rerender } = render(
      <CodeSurface label="json" copyValue="{}" wrap onWrapChange={() => {}}>
        <span>body</span>
      </CodeSurface>,
    );
    expect(screen.getByTestId("code-surface-body")).not.toHaveClass("overflow-x-auto");

    rerender(
      <CodeSurface label="json" copyValue="{}" wrap={false} onWrapChange={() => {}}>
        <span>body</span>
      </CodeSurface>,
    );
    expect(screen.getByTestId("code-surface-body")).toHaveClass("overflow-x-auto");
  });

  it("keeps the type scale on the frame so caller overrides still inherit", () => {
    // Several call sites tighten a nested payload with `text-[11px]/relaxed`.
    // Font size inherits, so it has to land on the element that sets it — if
    // the body ever takes over the scale, those overrides stop working.
    render(
      <CodeSurface label="json" copyValue="{}" className="text-[11px]/relaxed">
        <span>body</span>
      </CodeSurface>,
    );
    const frame = screen.getByTestId("code-surface");
    expect(frame).toHaveClass("text-[11px]/relaxed");
    expect(frame).not.toHaveClass("text-xs");
    expect(screen.getByTestId("code-surface-body")).not.toHaveClass("text-xs");
  });
});
