import { expect, test } from "@playwright/test";
import { MONEY_RUN } from "./helpers";

// The fixture assigns each case a stable output flavor by index (see
// `outputFlavor` in src/mocks/fixtures.ts): case-0000 is JSON, case-0003 is
// plain text, case-0004 is markdown. Deep-link straight to each via `?case=`.
test.describe("OutputViewer", () => {
  test("a JSON output renders a collapsible tree, then toggles to raw", async ({
    page,
  }) => {
    await page.goto(`/runs/${MONEY_RUN}?case=case-0000`);
    const drawer = page.getByRole("dialog");
    await expect(drawer).toBeVisible();
    await expect(drawer.getByRole("heading", { name: "Output" })).toBeVisible();

    // Rendered by default: the json type chip + tree controls + a known key.
    await expect(drawer.getByText("json", { exact: true })).toBeVisible();
    const expandAll = drawer.getByRole("button", { name: "Expand all" });
    await expect(expandAll).toBeVisible();
    await expect(drawer.getByText(/"intent"/)).toBeVisible();

    // Toggle to Raw: the tree controls vanish and the source shows through the
    // code block, syntax-highlighted and numbered rather than a bare <pre>.
    await drawer.getByRole("radio", { name: "Raw" }).click();
    await expect(expandAll).toHaveCount(0);
    const block = drawer.getByTestId("code-block");
    await expect(block).toBeVisible();
    await expect(block.getByTestId("code-block-line-num").first()).toBeVisible();

    // Highlighting is deferred until the block nears the viewport, and the
    // Output section sits well below the fold in a full case drawer — so this
    // scroll is not incidental, it is the thing being tested. Asserting the
    // tokens without it passed only by accident of where the drawer happened to
    // be scrolled.
    await block.scrollIntoViewIfNeeded();
    await expect(block.locator(".hljs-attr").first()).toBeVisible();
    await expect(drawer.getByText(/"explanation"/)).toBeVisible();

    // Toggling back restores the tree.
    await drawer.getByRole("radio", { name: "Rendered" }).click();
    await expect(drawer.getByRole("button", { name: "Expand all" })).toBeVisible();
  });

  test("a markdown output renders a heading", async ({ page }) => {
    await page.goto(`/runs/${MONEY_RUN}?case=case-0004`);
    const drawer = page.getByRole("dialog");
    await expect(drawer).toBeVisible();

    await expect(drawer.getByText("markdown", { exact: true })).toBeVisible();
    // The fixture's markdown output leads with an H1 that react-markdown renders.
    await expect(
      drawer.getByRole("heading", { name: "Resolution summary" }),
    ).toBeVisible();
  });

  test("a fenced code block renders a numbered gutter and its own controls", async ({
    page,
  }) => {
    // Only reachable in a real browser: jsdom has no layout, so the grid gutter
    // and the block's own lazy chunk are not provable in the unit suite.
    await page.goto(`/runs/${MONEY_RUN}?case=case-0004`);
    const drawer = page.getByRole("dialog");
    await expect(drawer).toBeVisible();

    const block = drawer.getByTestId("code-block").first();
    await expect(block).toBeVisible();
    // The fixture's fence is pretty-printed JSON, so the gutter numbers each of
    // its five lines.
    await expect(block.getByTestId("code-block-line-num")).toHaveCount(5);
    await expect(block.getByText("json", { exact: true })).toBeVisible();

    // The header controls are revealed on hover; they drive the shared wrap
    // preference, so the pressed state has to survive the round trip.
    await block.hover();
    const wrap = block.getByTestId("code-block-wrap-toggle");
    await expect(wrap).toHaveAttribute("aria-pressed", "true");
    await wrap.click();
    await expect(wrap).toHaveAttribute("aria-pressed", "false");

    await block.getByRole("button", { name: "Copy" }).click();
  });

  test("the wrap toggle flips its pressed state on a text output", async ({
    page,
  }) => {
    await page.goto(`/runs/${MONEY_RUN}?case=case-0003`);
    const drawer = page.getByRole("dialog");
    await expect(drawer).toBeVisible();

    // Plain text has no rendered view. It goes through the block so it gains a
    // header and copy, but stays unhighlighted — auto-detecting a grammar for a
    // prose sentence would colour ordinary words as keywords.
    const block = drawer.getByTestId("code-block");
    await expect(block.getByText("text", { exact: true })).toBeVisible();
    await expect(block.locator(".hljs-keyword")).toHaveCount(0);

    await block.hover();
    const wrap = block.getByTestId("code-block-wrap-toggle");
    await expect(wrap).toHaveAttribute("aria-pressed", "true");
    await wrap.click();
    await expect(wrap).toHaveAttribute("aria-pressed", "false");
  });

  test("the copy button shows feedback", async ({ page }) => {
    await page.goto(`/runs/${MONEY_RUN}?case=case-0003`);
    const drawer = page.getByRole("dialog");
    await expect(drawer).toBeVisible();

    // Exact match: the drawer header now also has a "Copy link" permalink
    // button, so target the OutputViewer's own "Copy" button precisely.
    await drawer.getByRole("button", { name: "Copy", exact: true }).click();
    await expect(drawer.getByText("Copied")).toBeVisible();
  });
});
