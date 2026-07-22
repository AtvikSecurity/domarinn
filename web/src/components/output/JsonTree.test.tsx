import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { JsonTree } from "./JsonTree";

describe("JsonTree", () => {
  it("renders nested values expanded by default and collapses on click", async () => {
    const user = userEvent.setup();
    render(<JsonTree data={{ user: { name: "alice" } }} />);

    // Nested object (<=20 children) starts open, so the value is visible.
    expect(screen.getByText(/alice/)).toBeInTheDocument();

    // Clicking the `user` node collapses it, hiding its children.
    await user.click(screen.getByRole("button", { name: /"user"/ }));
    expect(screen.queryByText(/alice/)).toBeNull();

    // Clicking again re-expands.
    await user.click(screen.getByRole("button", { name: /"user"/ }));
    expect(screen.getByText(/alice/)).toBeInTheDocument();
  });

  it("default-collapses a node with more than 20 children", async () => {
    const user = userEvent.setup();
    const big = Object.fromEntries(
      Array.from({ length: 25 }, (_, i) => [`k${i}`, `val-${i}`]),
    );
    render(<JsonTree data={{ big }} />);

    // The 25-key node is collapsed: a summary shows, the children do not.
    expect(screen.getByText(/25 keys/)).toBeInTheDocument();
    expect(screen.queryByText(/val-0/)).toBeNull();

    // Expanding reveals the children.
    await user.click(screen.getByRole("button", { name: /"big"/ }));
    expect(screen.getByText(/val-0/)).toBeInTheDocument();
  });

  it("expand-all / collapse-all reaches deeply nested nodes", async () => {
    const user = userEvent.setup();
    render(<JsonTree data={{ a: { b: { c: "deep-value" } } }} />);

    expect(screen.getByText(/deep-value/)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Collapse all" }));
    expect(screen.queryByText(/deep-value/)).toBeNull();

    await user.click(screen.getByRole("button", { name: "Expand all" }));
    expect(screen.getByText(/deep-value/)).toBeInTheDocument();
  });

  it("truncates long strings behind a more/less expander", async () => {
    const user = userEvent.setup();
    const longStr = "x".repeat(200);
    render(<JsonTree data={{ note: longStr }} />);

    const more = screen.getByRole("button", { name: "more" });
    expect(more).toBeInTheDocument();
    // Truncated: the full string is not shown yet.
    expect(screen.queryByText(new RegExp(longStr))).toBeNull();

    await user.click(more);
    expect(screen.getByRole("button", { name: "less" })).toBeInTheDocument();
    expect(screen.getByText(new RegExp(longStr))).toBeInTheDocument();
  });

  it("renders a bare primitive without expand controls", () => {
    render(<JsonTree data={"just a string"} />);
    expect(screen.getByText(/just a string/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Expand all" })).toBeNull();
  });
});
