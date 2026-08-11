import { beforeAll, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { DetailDrawer } from "./DetailDrawer";

// Radix uses pointer capture and measurement that jsdom does not implement.
beforeAll(() => {
  Element.prototype.hasPointerCapture = () => false;
  Element.prototype.setPointerCapture = () => {};
  Element.prototype.releasePointerCapture = () => {};
  Element.prototype.scrollIntoView = () => {};
  if (!("ResizeObserver" in globalThis)) {
    globalThis.ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
});

interface Row {
  key: string;
  name: string;
}

const ROW: Row = { key: "case-0001", name: "handles empty cart" };

function renderDrawer(props: Partial<React.ComponentProps<typeof DetailDrawer<Row>>> = {}) {
  const onClose = vi.fn();
  const utils = render(
    <DetailDrawer<Row>
      open
      item={ROW}
      onClose={onClose}
      navItemLabel="case"
      renderEyebrow={(r) => r?.key ?? null}
      renderTitle={(r) => r.name}
      renderBody={(r) => <div>body for {r.name}</div>}
      {...props}
    />,
  );
  return { ...utils, onClose };
}

/** The same drawer again with one prop changed, for the latch/step cases. */
function rerenderDrawer(
  rerender: (ui: React.ReactElement) => void,
  props: Partial<React.ComponentProps<typeof DetailDrawer<Row>>>,
) {
  rerender(
    <DetailDrawer<Row>
      open
      item={ROW}
      onClose={vi.fn()}
      navItemLabel="case"
      renderEyebrow={(r) => r?.key ?? null}
      renderTitle={(r) => r.name}
      renderBody={(r) => <div>body for {r.name}</div>}
      {...props}
    />,
  );
}

describe("DetailDrawer", () => {
  it("renders nothing when closed", () => {
    renderDrawer({ open: false });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("renders the eyebrow, title and body of the item", () => {
    renderDrawer();
    expect(screen.getByRole("dialog", { name: "handles empty cart" })).toBeInTheDocument();
    expect(screen.getByText("case-0001")).toBeInTheDocument();
    expect(screen.getByText(/body for handles empty cart/)).toBeInTheDocument();
  });

  it("renders the subheader outside the scrolling body", () => {
    renderDrawer({ renderSubheader: (r) => <div>verdict for {r.name}</div> });
    expect(screen.getByText(/verdict for handles empty cart/)).toBeInTheDocument();
  });

  // The click has to feel answered immediately, so the shell opens on the
  // selection and fills in when the data lands.
  describe("while pending", () => {
    it("shows a skeleton announced once", () => {
      renderDrawer({ item: null });
      expect(screen.getByRole("status")).toHaveTextContent("Loading case…");
      expect(screen.queryByText(/body for/)).toBeNull();
    });

    // Radix takes the dialog's accessible name from the title, so it must never
    // be absent: "undefined" is what a screen reader would otherwise announce.
    it("still gives the dialog an accessible name", () => {
      renderDrawer({ item: null });
      expect(screen.getByRole("dialog", { name: "case" })).toBeInTheDocument();
      expect(screen.getByRole("dialog")).toHaveAttribute("aria-busy", "true");
    });

    // The selection is already in the URL, so a cold open knows which case it
    // is fetching. Replacing that with an anonymous bar loses the one fact the
    // user could have checked while waiting.
    it("shows the identity the caller can name without the item", () => {
      renderDrawer({ item: null, renderEyebrow: (r) => r?.key ?? "case-0002" });
      expect(screen.getByText("case-0002")).toBeInTheDocument();
    });
  });

  describe("when the load fails", () => {
    it("shows the error with a retry", async () => {
      const onRetry = vi.fn();
      renderDrawer({ item: null, error: new Error("boom"), onRetry });
      expect(screen.getByText("boom")).toBeInTheDocument();
      await userEvent.click(screen.getByRole("button", { name: "Retry" }));
      expect(onRetry).toHaveBeenCalledOnce();
    });

    it("shows no skeleton", () => {
      renderDrawer({ item: null, error: new Error("boom") });
      expect(screen.queryByRole("status")).toBeNull();
    });

    // Leaving the previous case on screen under a failed load would state
    // something false about what is being shown.
    it("wins over a latched item", () => {
      const { rerender } = renderDrawer();
      rerenderDrawer(rerender, { item: null, error: new Error("boom") });
      expect(screen.getByText("boom")).toBeInTheDocument();
      expect(screen.queryByText(/body for/)).toBeNull();
    });

    // The header is part of "what is being shown": a failure that keeps the
    // previous case's key, name and verdict strip above it reads as that case
    // having failed, which is a different and wrong claim.
    it("drops the latched item's header, title and subheader too", () => {
      const subheader = (r: Row) => <div>verdict for {r.name}</div>;
      const { rerender } = renderDrawer({ renderSubheader: subheader });
      rerenderDrawer(rerender, {
        item: null,
        error: new Error("boom"),
        renderSubheader: subheader,
      });

      expect(screen.getByText("boom")).toBeInTheDocument();
      expect(screen.queryByText("case-0001")).toBeNull();
      expect(screen.queryByText("handles empty cart")).toBeNull();
      expect(screen.queryByText(/verdict for/)).toBeNull();
    });
  });

  // Stepping through a list re-fetches on every step; without the latch each
  // step would blink through a skeleton.
  it("keeps the previous item rendered while the next one loads", () => {
    const { rerender } = renderDrawer();
    rerenderDrawer(rerender, { item: null });
    expect(screen.getByText(/body for handles empty cart/)).toBeInTheDocument();
    expect(screen.queryByRole("status")).toBeNull();
  });

  describe("navigation", () => {
    it("is absent when the page supplies no neighbours", () => {
      renderDrawer();
      expect(screen.queryByRole("button", { name: /Previous case/ })).toBeNull();
    });

    it("disables the chevron at an end of the list rather than hiding it", () => {
      renderDrawer({ onNext: vi.fn(), position: { index: 1, total: 4 } });
      expect(screen.getByRole("button", { name: /Previous case/ })).toBeDisabled();
      expect(screen.getByRole("button", { name: /Next case/ })).toBeEnabled();
    });

    it("shows the position over loaded rows", () => {
      renderDrawer({ onPrev: vi.fn(), onNext: vi.fn(), position: { index: 3, total: 47 } });
      expect(screen.getByText("3 of 47")).toBeInTheDocument();
    });

    it("steps on click", async () => {
      const onNext = vi.fn();
      renderDrawer({ onNext });
      await userEvent.click(screen.getByRole("button", { name: /Next case/ }));
      expect(onNext).toHaveBeenCalledOnce();
    });

    it("steps on the arrow keys", async () => {
      const onPrev = vi.fn();
      const onNext = vi.fn();
      renderDrawer({ onPrev, onNext });
      await userEvent.keyboard("{ArrowDown}");
      expect(onNext).toHaveBeenCalledOnce();
      await userEvent.keyboard("{ArrowUp}");
      expect(onPrev).toHaveBeenCalledOnce();
    });

    // The drawer contains real inputs, and an arrow key inside one belongs to
    // the caret.
    it("leaves the arrow keys alone while typing", async () => {
      const onNext = vi.fn();
      renderDrawer({
        onNext,
        renderBody: () => <input aria-label="Filter" />,
      });
      await userEvent.click(screen.getByRole("textbox", { name: "Filter" }));
      await userEvent.keyboard("{ArrowDown}");
      expect(onNext).not.toHaveBeenCalled();
    });

    // The body carries segmented controls whose own Arrow handling calls
    // preventDefault. Stepping the list as well would move two things at once
    // from one keypress.
    it("leaves the arrow keys to a control that already handled them", async () => {
      const onNext = vi.fn();
      renderDrawer({
        onNext,
        renderBody: () => (
          <button type="button" onKeyDown={(e) => e.preventDefault()}>
            Raw
          </button>
        ),
      });
      await userEvent.click(screen.getByRole("button", { name: "Raw" }));
      await userEvent.keyboard("{ArrowDown}");
      expect(onNext).not.toHaveBeenCalled();
    });

    it("ignores the arrow keys once closed", async () => {
      const onNext = vi.fn();
      renderDrawer({ open: false, onNext });
      await userEvent.keyboard("{ArrowDown}");
      expect(onNext).not.toHaveBeenCalled();
    });
  });

  describe("chrome", () => {
    it("closes from the ✕", async () => {
      const { onClose } = renderDrawer();
      await userEvent.click(screen.getByRole("button", { name: "Close case drawer" }));
      expect(onClose).toHaveBeenCalledOnce();
    });

    it("closes on Escape", async () => {
      const { onClose } = renderDrawer();
      await userEvent.keyboard("{Escape}");
      expect(onClose).toHaveBeenCalledOnce();
    });

    it("renders header actions before the built-in controls", () => {
      renderDrawer({
        renderHeaderActions: () => <button type="button">Copy link</button>,
      });
      expect(screen.getByRole("button", { name: "Copy link" })).toBeInTheDocument();
    });

    // A "copy this" button that acts on the row still loading behind the one
    // on screen hands back the wrong id without ever looking wrong.
    it("gives header actions the item on screen, not the pending selection", () => {
      const { rerender } = renderDrawer({
        renderHeaderActions: (r) => <button type="button">copy {r?.key ?? "none"}</button>,
      });
      rerenderDrawer(rerender, {
        item: null,
        renderHeaderActions: (r) => <button type="button">copy {r?.key ?? "none"}</button>,
      });
      expect(screen.getByRole("button", { name: "copy case-0001" })).toBeInTheDocument();
    });

    it("carries the resize handle and the width toggle", () => {
      renderDrawer();
      expect(screen.getByRole("separator", { name: "Resize panel" })).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Toggle panel width" })).toBeInTheDocument();
    });

    it("carries matching open and closed animation classes", () => {
      renderDrawer();
      const dialog = screen.getByRole("dialog");
      const overlay = document.querySelector<HTMLElement>(".detail-drawer-overlay");

      expect(overlay).not.toBeNull();
      expect(overlay).toHaveClass(
        "data-[state=open]:animate-[overlay-in_120ms_ease-out]",
        "data-[state=closed]:animate-[overlay-out_220ms_ease]",
      );
      expect(dialog).toHaveClass(
        "detail-drawer-panel",
        "data-[state=open]:animate-[drawer-in_160ms_ease-out]",
        "data-[state=closed]:animate-[drawer-out_220ms_cubic-bezier(0.2,0.9,0.2,1)]",
      );
    });
  });
});
