import { describe, expect, it, vi } from "vitest";
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

describe("JsonTree surface", () => {
  it("labels the payload and copies it pretty-printed", async () => {
    const writeText = vi.fn<(t: string) => Promise<void>>().mockResolvedValue();
    const user = userEvent.setup();
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });

    render(<JsonTree data={{ intent: "refund", score: 0.84 }} />);
    expect(screen.getByText("json")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Copy" }));
    // Copying the tree has to yield source you can paste back, not the
    // collapsed one-line shape the tree happens to be showing.
    expect(writeText).toHaveBeenCalledWith('{\n  "intent": "refund",\n  "score": 0.84\n}');
  });

  it("drives soft wrap from the body so every node follows", async () => {
    const user = userEvent.setup();
    render(<JsonTree data={{ note: "a b c" }} />);
    const body = screen.getByTestId("json-tree-body");

    expect(body).toHaveClass("whitespace-pre-wrap", "break-words");
    expect(body).not.toHaveClass("overflow-x-auto");

    await user.click(screen.getByTestId("json-tree-wrap-toggle"));
    expect(body).toHaveClass("whitespace-pre");
    expect(body).not.toHaveClass("whitespace-pre-wrap");
    expect(body).toHaveClass("overflow-x-auto");

    // The nodes must carry no wrap class of their own — the whole point is that
    // `white-space` inherits, so re-pinning it on a node would strand that
    // subtree in the old mode. (Whether the inherited value actually paints is
    // a browser question; jsdom runs with css disabled.)
    const nodes = [...screen.getByTestId("json-tree-body").querySelectorAll("div")];
    expect(nodes.length).toBeGreaterThan(0); // guard: the check below must not be vacuous
    expect(
      nodes.flatMap((el) => [...el.classList]).filter((c) => c.startsWith("whitespace-")),
    ).toEqual([]);
  });

  it("defers wrap to the caller when controlled", async () => {
    const onWrapChange = vi.fn();
    const user = userEvent.setup();
    render(<JsonTree data={{ a: 1 }} wrap={false} onWrapChange={onWrapChange} />);

    await user.click(screen.getByTestId("json-tree-wrap-toggle"));

    expect(onWrapChange).toHaveBeenCalledWith(true);
    expect(screen.getByTestId("json-tree-wrap-toggle")).toHaveAttribute(
      "aria-pressed",
      "false",
    );
  });

  it("keeps the expand controls out of the hover-revealed group", () => {
    // Expand/collapse is how you read a deep payload at all; hiding it until
    // hover would make the tree unusable by anyone who does not know to try.
    render(<JsonTree data={{ a: { b: 1 } }} />);
    const expand = screen.getByRole("button", { name: "Expand all" });
    expect(expand.closest(".opacity-0")).toBeNull();
    expect(
      screen.getByRole("button", { name: "Copy" }).closest(".opacity-0"),
    ).not.toBeNull();
  });
});
