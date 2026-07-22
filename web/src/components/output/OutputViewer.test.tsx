import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { OutputViewer } from "./OutputViewer";

beforeEach(() => {
  localStorage.clear();
});

describe("OutputViewer", () => {
  it("shows a type chip for the detected content", () => {
    const { unmount } = render(<OutputViewer value={'{"a":1}'} />);
    expect(screen.getByText("json")).toBeInTheDocument();
    unmount();

    render(<OutputViewer value={"# Heading\n\nbody"} />);
    expect(screen.getByText("markdown")).toBeInTheDocument();
  });

  it("hides the Rendered|Raw toggle for plain text (nothing to render)", () => {
    render(<OutputViewer value={"just some plain prose output"} />);
    expect(screen.queryByRole("radio", { name: "Raw" })).toBeNull();
    expect(screen.getByText("text")).toBeInTheDocument();
  });

  it("toggles to Raw and persists the preference to localStorage", async () => {
    const user = userEvent.setup();
    const json = '{"intent":"resolve"}';
    const { unmount } = render(<OutputViewer value={json} />);

    // Default is Rendered → a JSON tree (with expand controls), not the raw pre.
    expect(screen.getByRole("button", { name: "Expand all" })).toBeInTheDocument();

    await user.click(screen.getByRole("radio", { name: "Raw" }));
    // Raw view: the expand controls are gone, the raw JSON string is shown.
    expect(screen.queryByRole("button", { name: "Expand all" })).toBeNull();
    expect(localStorage.getItem("domarinn.output.raw")).toBe("1");
    unmount();

    // A fresh viewer reads the persisted preference and starts in Raw.
    render(<OutputViewer value={json} />);
    expect(screen.getByRole("radio", { name: "Raw" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(screen.queryByRole("button", { name: "Expand all" })).toBeNull();
  });

  it("shows the RawText fallback while the lazy markdown view loads", async () => {
    const { container } = render(<OutputViewer value={"# Hello\n\nworld"} />);
    // Synchronously, the Suspense fallback (RawText <pre> with the raw source)
    // is on screen so nothing flashes empty.
    const pre = container.querySelector("pre");
    expect(pre).not.toBeNull();
    expect(pre?.textContent).toContain("# Hello");

    // Once the lazy chunk resolves, the rendered heading replaces it.
    expect(await screen.findByRole("heading", { name: "Hello" })).toBeInTheDocument();
  });

  it("copies the raw text and shows feedback", async () => {
    const writeText = vi.fn<(t: string) => Promise<void>>().mockResolvedValue();
    const user = userEvent.setup();
    // Override after setup() so the component's writeText is our spy, not the
    // clipboard stub userEvent installs.
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });

    render(<OutputViewer value={"plain output text"} />);
    await user.click(screen.getByRole("button", { name: "Copy" }));

    expect(writeText).toHaveBeenCalledWith("plain output text");
    expect(await screen.findByText("Copied")).toBeInTheDocument();
  });
});
