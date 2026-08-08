import { beforeAll, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { DrawerResizer } from "./DrawerResizer";
import { maxWidth } from "@/lib/drawerWidth";

// Pointer capture is how the handle keeps tracking after the cursor leaves its
// narrow hit target; jsdom does not implement it.
beforeAll(() => {
  Element.prototype.hasPointerCapture = () => false;
  Element.prototype.setPointerCapture = () => {};
  Element.prototype.releasePointerCapture = () => {};
});

function renderResizer() {
  const onResize = vi.fn();
  const onToggle = vi.fn();
  render(<DrawerResizer width={704} onResize={onResize} onToggle={onToggle} />);
  return {
    separator: screen.getByRole("separator", { name: "Resize panel" }),
    onResize,
    onToggle,
  };
}

describe("DrawerResizer", () => {
  it("renders a visible grip tab inside the separator", () => {
    const { separator } = renderResizer();
    const grip = screen.getByTestId("drawer-resize-grip");
    expect(separator).toContainElement(grip);
    expect(grip).toHaveClass("h-24", "w-7");
  });

  it("exposes its dragging state from pointer down through pointer up", () => {
    const { separator } = renderResizer();
    fireEvent.pointerDown(separator, { pointerId: 1, clientX: 300 });
    expect(separator).toHaveAttribute("data-dragging", "true");
    fireEvent.pointerUp(separator, { pointerId: 1, clientX: 300 });
    expect(separator).not.toHaveAttribute("data-dragging");
  });

  // `clampWidth` stops at 95% of the viewport, so announcing the full width
  // promises a size the handle will not reach.
  it("advertises the width it can actually reach", () => {
    const { separator } = renderResizer();
    expect(separator).toHaveAttribute(
      "aria-valuemax",
      String(maxWidth(window.innerWidth)),
    );
  });

  // Without this the browser can claim the drag as a pan and fire
  // pointercancel mid-resize.
  it("keeps the drag from being stolen by touch panning", () => {
    const { separator } = renderResizer();
    expect(separator).toHaveClass("touch-none");
  });

  it("resizes left and right from the keyboard", async () => {
    const user = userEvent.setup();
    const { separator, onResize } = renderResizer();
    separator.focus();
    await user.keyboard("{ArrowLeft}{ArrowRight}");
    expect(onResize).toHaveBeenNthCalledWith(1, 728);
    expect(onResize).toHaveBeenNthCalledWith(2, 680);
  });

  it("toggles expansion from double click and Enter", async () => {
    const user = userEvent.setup();
    const { separator, onToggle } = renderResizer();
    await user.dblClick(separator);
    separator.focus();
    await user.keyboard("{Enter}");
    expect(onToggle).toHaveBeenCalledTimes(2);
  });
});
