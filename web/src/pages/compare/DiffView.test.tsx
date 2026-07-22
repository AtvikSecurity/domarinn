import { describe, expect, it } from "vitest";
import { render, waitFor } from "@testing-library/react";
import { DiffView } from "./DiffView";

async function paneFor(
  container: HTMLElement,
  mode: string,
): Promise<HTMLElement> {
  return waitFor(() => {
    const el = container.querySelector(`[data-diff-mode="${mode}"]`);
    expect(el).not.toBeNull();
    return el as HTMLElement;
  });
}

describe("DiffView", () => {
  it("defaults to the two-column side-by-side pane", async () => {
    const { container } = render(<DiffView base="alpha" head="beta" />);
    const pane = await paneFor(container, "side");
    expect(pane.querySelectorAll("pre")).toHaveLength(2);
  });

  it("inline mode marks additions (green) and removals (strikethrough)", async () => {
    const { container } = render(
      <DiffView base="hello world" head="hello there" mode="inline" />,
    );
    const pane = await paneFor(container, "inline");
    expect(pane.querySelector(".line-through")?.textContent).toContain("world");
    expect(pane.querySelector(".text-pass")?.textContent).toContain("there");
  });

  it("lines mode renders +/- gutters for changed lines", async () => {
    const { container } = render(
      <DiffView base={"a\nb\nc"} head={"a\nX\nc"} mode="lines" />,
    );
    const pane = await paneFor(container, "lines");
    const gutters = Array.from(pane.querySelectorAll(".select-none")).map(
      (g) => g.textContent,
    );
    expect(gutters).toContain("+");
    expect(gutters).toContain("-");
  });
});
