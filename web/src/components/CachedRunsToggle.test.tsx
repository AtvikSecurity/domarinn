import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { CachedRunsToggle } from "./CachedRunsToggle";

describe("CachedRunsToggle", () => {
  it("says how many runs are hidden and reveals them", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <CachedRunsToggle resolved="exclude" hiddenCount={6} onChange={onChange} />,
    );

    expect(screen.getByText(/6 fully cached runs hidden/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Show" }));
    expect(onChange).toHaveBeenCalledWith("all");
  });

  it("counts one hidden run in the singular", () => {
    render(<CachedRunsToggle resolved="exclude" hiddenCount={1} onChange={vi.fn()} />);
    expect(screen.getByText(/1 fully cached run hidden/)).toBeInTheDocument();
  });

  // Suppression that suppressed nothing is not worth a line of the page, and
  // "0 hidden" reads as a bug.
  it("renders nothing when hiding but nothing was hidden", () => {
    const { container } = render(
      <CachedRunsToggle resolved="exclude" hiddenCount={0} onChange={vi.fn()} />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("renders nothing when there is nothing to report", () => {
    const { container } = render(
      <CachedRunsToggle resolved="exclude" onChange={vi.fn()} />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  // Search ranks by bm25 behind a LIMIT, so no honest count exists — but the
  // suppression still has to be visible, or a short result list reads as "we
  // found nothing" rather than "we hid some".
  it("announces suppression without a number when it cannot count", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <CachedRunsToggle
        resolved="exclude"
        hiddenCount="unknown"
        onChange={onChange}
      />,
    );

    expect(
      screen.getByText(/Hits from fully cached runs are hidden/),
    ).toBeInTheDocument();
    // No invented figure anywhere in the line.
    expect(screen.queryByText(/\d/)).toBeNull();
    await user.click(screen.getByRole("button", { name: "Show" }));
    expect(onChange).toHaveBeenCalledWith("all");
  });

  // The revealed state must stay visible and reversible: a user who clicked
  // Show needs a way back that is not "find the filter bar".
  it("offers a way back when cached runs are showing", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CachedRunsToggle resolved="all" onChange={onChange} />);

    expect(screen.getByText(/Showing cached runs/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Hide" }));
    expect(onChange).toHaveBeenCalledWith("exclude");
  });

  it("explains the only-cached view and offers the way out", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CachedRunsToggle resolved="only" onChange={onChange} />);

    expect(
      screen.getByText(/Showing only fully cached runs/),
    ).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Show all" }));
    expect(onChange).toHaveBeenCalledWith("all");
  });

  // `all` is the reveal, so a leftover count from the hidden view must not
  // follow the user into it.
  it("ignores a stale hidden count once revealed", () => {
    render(<CachedRunsToggle resolved="all" hiddenCount={6} onChange={vi.fn()} />);
    expect(screen.queryByText(/hidden/)).toBeNull();
  });
});
