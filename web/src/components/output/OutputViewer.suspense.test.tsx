import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { OutputViewer } from "./OutputViewer";

/**
 * This lives in its own file on purpose, and must stay the only test here that
 * renders markdown.
 *
 * `React.lazy` caches the resolved module on the lazy component itself, so a
 * Suspense fallback is only ever observable on the *first* render of that
 * component in a process. Any earlier markdown render in the same file resolves
 * the chunk, and this assertion then races the import instead of testing it —
 * which is exactly how the sibling suite passed under a loaded machine and
 * failed when run on its own. Vitest isolates module state per file (the
 * default), so a file of its own is what makes the first render genuinely first.
 */
describe("OutputViewer lazy-chunk fallback", () => {
  it("shows the raw source while the markdown view is still loading", () => {
    const { container } = render(<OutputViewer value={"# Hello\n\nworld"} />);

    // Synchronously — before the chunk resolves — the RawText <pre> carrying
    // the unrendered source is on screen, so the drawer never flashes empty.
    const pre = container.querySelector("pre");
    expect(pre).not.toBeNull();
    expect(pre?.textContent).toContain("# Hello");
    expect(screen.queryByRole("heading", { name: "Hello" })).toBeNull();
  });
});
