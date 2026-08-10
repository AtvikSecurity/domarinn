import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { CodeBlock, MAX_HIGHLIGHT_LINES } from "./CodeBlock";

const PY = 'def f():\n    """Doc\n    spans lines\n    """\n    return 1';

/** Token spans are what the `.hljs-*` theme in index.css paints. */
function tokens(container: HTMLElement): NodeListOf<Element> {
  return container.querySelectorAll('[class^="hljs-"]');
}

function gutter(): HTMLElement[] {
  return screen.queryAllByTestId("code-block-line-num");
}

function codeCells(container: HTMLElement): HTMLElement[] {
  return [...container.querySelectorAll("code")];
}

describe("CodeBlock line-number gutter", () => {
  it("renders one gutter cell per logical line", () => {
    render(<CodeBlock code={"a\nb\nc"} />);
    expect(gutter().map((el) => el.textContent)).toEqual(["1", "2", "3"]);
  });

  it("suppresses the gutter for single-line code", () => {
    render(<CodeBlock code="just one line" />);
    expect(gutter()).toHaveLength(0);
  });

  it("honours an explicit showLineNumbers in both directions", () => {
    const { unmount } = render(<CodeBlock code="one line" showLineNumbers />);
    expect(gutter()).toHaveLength(1);
    unmount();

    render(<CodeBlock code={"a\nb"} showLineNumbers={false} />);
    expect(gutter()).toHaveLength(0);
  });

  it("keeps one gutter cell per SOURCE line when highlighting splits a token", () => {
    // The docstring is a single hljs-string span covering lines 2-4. If the
    // splitter or the grid ever regress, the gutter desyncs from the code.
    const { container } = render(<CodeBlock code={PY} language="python" />);
    expect(gutter()).toHaveLength(5);
    expect(codeCells(container)).toHaveLength(5);
  });

  it("keeps a row for an empty middle line", () => {
    render(<CodeBlock code={"a\n\nc"} />);
    expect(gutter()).toHaveLength(3);
  });
});

describe("CodeBlock header", () => {
  it("labels the block with the language hint it was given", () => {
    render(<CodeBlock code={PY} language="python" />);
    expect(screen.getByText("python")).toBeInTheDocument();
  });

  it('falls back to "code" when there is nothing to detect', () => {
    render(<CodeBlock code="x" highlight={false} />);
    expect(screen.getByText("code")).toBeInTheDocument();
  });

  it("reports the auto-detected language when no hint was passed", () => {
    // detect.ts only yields a hint when markdown carried a fence tag, so most
    // blocks arrive unlabelled and lean on hljs auto-detection.
    render(<CodeBlock code={PY} />);
    expect(screen.getByTestId("code-block")).toHaveTextContent("python");
  });

  it("copies the code, not the rendered markup", async () => {
    const writeText = vi.fn<(t: string) => Promise<void>>().mockResolvedValue();
    const user = userEvent.setup();
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });

    render(<CodeBlock code={PY} language="python" />);
    await user.click(screen.getByRole("button", { name: "Copy code" }));

    expect(writeText).toHaveBeenCalledWith(PY);
  });
});

describe("CodeBlock soft wrap", () => {
  it("wraps by default and leaves horizontal scroll off", () => {
    const { container } = render(<CodeBlock code={"a\nb"} />);
    expect(screen.getByTestId("code-block-wrap-toggle")).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    for (const cell of codeCells(container)) {
      expect(cell).toHaveClass("whitespace-pre-wrap");
    }
    expect(screen.getByTestId("code-block-body")).not.toHaveClass("overflow-x-auto");
  });

  it("hands horizontal overflow to the body when toggled off", async () => {
    const user = userEvent.setup();
    const { container } = render(<CodeBlock code={"a\nb"} />);

    await user.click(screen.getByTestId("code-block-wrap-toggle"));

    expect(screen.getByTestId("code-block-wrap-toggle")).toHaveAttribute(
      "aria-pressed",
      "false",
    );
    for (const cell of codeCells(container)) {
      expect(cell).toHaveClass("whitespace-pre");
      expect(cell).not.toHaveClass("whitespace-pre-wrap");
    }
    expect(screen.getByTestId("code-block-body")).toHaveClass("overflow-x-auto");
  });

  it("honours defaultWrap={false}", () => {
    render(<CodeBlock code={"a\nb"} defaultWrap={false} />);
    expect(screen.getByTestId("code-block-wrap-toggle")).toHaveAttribute(
      "aria-pressed",
      "false",
    );
  });

  it("defers to the caller when controlled, instead of holding its own state", async () => {
    // OutputViewer and MarkdownView drive this from the shared prefs store, so
    // the block must not fork a private copy — two viewers on screen would
    // disagree, which is the bug prefs.ts exists to prevent.
    const onWrapChange = vi.fn();
    const user = userEvent.setup();
    render(<CodeBlock code={"a\nb"} wrap={false} onWrapChange={onWrapChange} />);

    await user.click(screen.getByTestId("code-block-wrap-toggle"));

    expect(onWrapChange).toHaveBeenCalledWith(true);
    // Still false: the store owns it, and no store update came back.
    expect(screen.getByTestId("code-block-wrap-toggle")).toHaveAttribute(
      "aria-pressed",
      "false",
    );
  });
});

describe("CodeBlock highlighting", () => {
  it("emits themed token spans for a known language", () => {
    const { container } = render(
      <CodeBlock code={'{"a": 1}'} language="json" showLineNumbers={false} />,
    );
    expect(tokens(container).length).toBeGreaterThan(0);
  });

  it("does not bleed a multiline token past its closing delimiter", () => {
    // Every line must be independently balanced; otherwise the docstring's
    // colour runs to the bottom of the block.
    const { container } = render(<CodeBlock code={PY} language="python" />);
    const cells = codeCells(container);
    // `return 1` is after the docstring closes — it must carry keyword/number
    // tokens of its own rather than sitting inside a still-open string span.
    expect(cells[4]!.querySelector(".hljs-string")).toBeNull();
    expect(cells[4]!.querySelector(".hljs-keyword")).not.toBeNull();
  });

  it("renders plain when highlighting is disabled", () => {
    const { container } = render(
      <CodeBlock code={PY} language="python" highlight={false} />,
    );
    expect(tokens(container)).toHaveLength(0);
    expect(screen.getByTestId("code-block-body")).toHaveTextContent("return 1");
  });

  it("falls back to plain above the line cap", () => {
    const huge = Array.from({ length: MAX_HIGHLIGHT_LINES + 1 }, () => "x = 1").join(
      "\n",
    );
    const { container } = render(<CodeBlock code={huge} language="python" />);
    expect(tokens(container)).toHaveLength(0);
  });

  it("escapes source that looks like markup", () => {
    // The highlighted path injects HTML, so anything the highlighter passes
    // through must already be escaped or a provider output could inject markup.
    const { container } = render(
      <CodeBlock code={'<img src=x onerror="boom">'} language="xml" />,
    );
    expect(container.querySelector("img")).toBeNull();
    expect(screen.getByTestId("code-block-body")).toHaveTextContent("onerror");
  });
});

describe("CodeBlock scroll ownership", () => {
  it("caps and scrolls the body only when asked", () => {
    const { unmount } = render(<CodeBlock code={"a\nb"} maxHeight="10rem" />);
    const body = screen.getByTestId("code-block-body");
    expect(body).toHaveClass("overflow-y-auto");
    expect(body.style.maxHeight).toBe("10rem");
    unmount();

    render(<CodeBlock code={"a\nb"} />);
    expect(screen.getByTestId("code-block-body")).not.toHaveClass("overflow-y-auto");
  });
});
