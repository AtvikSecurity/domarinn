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

    // Toggle to Raw: the tree controls vanish, the raw JSON <pre> remains.
    await drawer.getByRole("radio", { name: "Raw" }).click();
    await expect(expandAll).toHaveCount(0);
    await expect(drawer.locator("pre")).toBeVisible();
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

  test("the wrap toggle flips its pressed state on a text output", async ({
    page,
  }) => {
    await page.goto(`/runs/${MONEY_RUN}?case=case-0003`);
    const drawer = page.getByRole("dialog");
    await expect(drawer).toBeVisible();

    // Plain text has no rendered view — just the raw <pre> and a Wrap toggle.
    await expect(drawer.getByText("text", { exact: true })).toBeVisible();
    const wrap = drawer.getByRole("button", { name: "Wrap" });
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
