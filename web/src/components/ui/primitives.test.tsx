import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { CollapsibleSection } from "./CollapsibleSection";
import { SegmentedControl } from "./SegmentedControl";

describe("CollapsibleSection", () => {
  it("is reachable as BOTH a heading and a button", () => {
    // This dual identity is the whole reason the button is nested inside the
    // heading: the drawer's sections are queried both ways across the suite.
    render(
      <CollapsibleSection title="Output">
        <p>body</p>
      </CollapsibleSection>,
    );
    expect(screen.getByRole("heading", { name: "Output" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Output" })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
  });

  it("keeps the title exactly matchable once meta is appended", () => {
    // `getByText("Output", { exact: true })` is used to prove the drawer body
    // loaded; meta must not merge into that text node.
    render(
      <CollapsibleSection title="Prompt" meta="· 2 messages">
        <p>body</p>
      </CollapsibleSection>,
    );
    expect(screen.getByText("Prompt", { exact: true })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Prompt/ })).toHaveAccessibleName(
      /2 messages/,
    );
  });

  it("toggles its body and reports state", async () => {
    const user = userEvent.setup();
    render(
      <CollapsibleSection title="Input">
        <p>body</p>
      </CollapsibleSection>,
    );
    expect(screen.getByText("body")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Input" }));
    expect(screen.queryByText("body")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Input" })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
  });

  it("supports controlled mode for query enabled-gating", async () => {
    const user = userEvent.setup();
    const onOpenChange = vi.fn();
    render(
      <CollapsibleSection title="History" open={false} onOpenChange={onOpenChange}>
        <p>body</p>
      </CollapsibleSection>,
    );
    expect(screen.queryByText("body")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "History" }));
    expect(onOpenChange).toHaveBeenCalledWith(true);
  });

  it("keeps actions out of the toggle's accessible name", async () => {
    const user = userEvent.setup();
    const onAction = vi.fn();
    render(
      <CollapsibleSection
        title="History"
        actions={<button onClick={onAction}>Widen</button>}
      >
        <p>body</p>
      </CollapsibleSection>,
    );
    expect(
      screen.getByRole("button", { name: "History" }),
    ).toHaveAccessibleName(/^History$/);
    // Clicking an action must not collapse the section.
    await user.click(screen.getByRole("button", { name: "Widen" }));
    expect(onAction).toHaveBeenCalled();
    expect(screen.getByText("body")).toBeInTheDocument();
  });
});

describe("SegmentedControl", () => {
  const OPTIONS = [
    { value: "list", label: "List" },
    { value: "matrix", label: "Matrix" },
    { value: "graph", label: "Graph" },
  ] as const;

  it("exposes a single tab stop that follows the selection", () => {
    render(
      <SegmentedControl
        ariaLabel="View"
        options={OPTIONS}
        value="matrix"
        onChange={() => {}}
      />,
    );
    expect(screen.getByRole("radio", { name: "List" })).toHaveAttribute(
      "tabindex",
      "-1",
    );
    expect(screen.getByRole("radio", { name: "Matrix" })).toHaveAttribute(
      "tabindex",
      "0",
    );
  });

  it("moves the selection with the arrow keys, wrapping at the ends", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <SegmentedControl
        ariaLabel="View"
        options={OPTIONS}
        value="list"
        onChange={onChange}
      />,
    );
    const first = screen.getByRole("radio", { name: "List" });
    first.focus();
    await user.keyboard("{ArrowRight}");
    expect(onChange).toHaveBeenCalledWith("matrix");

    onChange.mockClear();
    await user.keyboard("{ArrowLeft}");
    expect(onChange).toHaveBeenCalledWith("graph"); // wraps
  });

  it("jumps to the ends with Home and End", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <SegmentedControl
        ariaLabel="View"
        options={OPTIONS}
        value="matrix"
        onChange={onChange}
      />,
    );
    screen.getByRole("radio", { name: "Matrix" }).focus();
    await user.keyboard("{End}");
    expect(onChange).toHaveBeenCalledWith("graph");

    onChange.mockClear();
    await user.keyboard("{Home}");
    expect(onChange).toHaveBeenCalledWith("list");
  });

  it("skips disabled options when moving", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <SegmentedControl
        ariaLabel="Diff mode"
        options={[
          { value: "side", label: "Side", disabled: true },
          { value: "inline", label: "Inline" },
          { value: "lines", label: "Lines" },
        ]}
        value="inline"
        onChange={onChange}
      />,
    );
    screen.getByRole("radio", { name: "Inline" }).focus();
    await user.keyboard("{ArrowLeft}");
    // Wraps past the disabled "Side" to "Lines".
    expect(onChange).toHaveBeenCalledWith("lines");
  });
});
