import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { OutputViewer } from "./OutputViewer";
import { __resetOutputPrefs } from "./prefs";

beforeEach(() => {
  localStorage.clear();
  // The Rendered/Raw + wrap preferences are a module-level store shared by
  // every mounted viewer (that is the point — see prefs.ts), so it outlives an
  // unmount and has to be reset between cases.
  __resetOutputPrefs();
});

describe("OutputViewer", () => {
  it("shows a type chip for the detected content", () => {
    const { unmount } = render(<OutputViewer value={'{"a":1}'} />);
    expect(screen.getByText("json")).toBeInTheDocument();
    unmount();

    render(<OutputViewer value={"# Heading\n\nbody"} />);
    expect(screen.getByText("markdown")).toBeInTheDocument();
  });

  it("keeps two simultaneously-mounted viewers in agreement", async () => {
    // The case drawer renders one viewer per prompt message plus one for the
    // output. Reading the shared preference once at mount left them disagreeing
    // on screen until something happened to remount.
    const user = userEvent.setup();
    render(
      <>
        <div data-testid="a">
          <OutputViewer value={'{"a":1}'} />
        </div>
        <div data-testid="b">
          <OutputViewer value={'{"b":2}'} />
        </div>
      </>,
    );

    const rawToggles = screen.getAllByRole("radio", { name: "Raw" });
    expect(rawToggles).toHaveLength(2);
    await user.click(rawToggles[0]!);

    for (const toggle of screen.getAllByRole("radio", { name: "Raw" })) {
      expect(toggle).toHaveAttribute("aria-checked", "true");
    }
  });

  it("hides the Rendered|Raw toggle for plain text (nothing to render)", () => {
    render(<OutputViewer value={"just some plain prose output"} />);
    expect(screen.queryByRole("radio", { name: "Raw" })).toBeNull();
    expect(screen.getByText("text")).toBeInTheDocument();
  });

  it("uses an outline pill for soft wrap and reports its pressed state", async () => {
    const user = userEvent.setup();
    __resetOutputPrefs({ raw: false, wrap: false });
    render(<OutputViewer value={"just some plain prose output"} />);

    const wrap = screen.getByRole("button", { name: "Wrap" });
    expect(wrap).toHaveAttribute("aria-pressed", "false");
    expect(wrap).toHaveClass("rounded-[3px]", "border-border-strong", "bg-transparent");

    await user.click(wrap);
    expect(wrap).toHaveAttribute("aria-pressed", "true");
    expect(localStorage.getItem("domarinn.output.wrap")).toBe("1");
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

  it("renders markdown through the lazy view once it resolves", async () => {
    render(<OutputViewer value={"# Hello\n\nworld"} />);
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
