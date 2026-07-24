import { expect, test } from "@playwright/test";
import {
  MONEY_RUN,
  V2_MESSAGES_CASE,
  V2_RUN,
  V2_TRUNCATED_CASE,
} from "./helpers";

test.describe("Case drawer schema-v2 sections", () => {
  test("shows the input, stop reason, and provider metadata for a v2 case", async ({
    page,
  }) => {
    // A run of the one v2-flavored suite; case-0000 carries a role-tagged
    // system+user prompt, a clean end_turn stop, and raw provider metadata.
    await page.goto(`/runs/${V2_RUN}?case=${V2_MESSAGES_CASE}`);

    const drawer = page.getByRole("dialog");
    await expect(drawer).toBeVisible();

    // stop_reason chip on the meta line. Asserted via the chip's title —
    // "end_turn" also appears inside the (now default-expanded) raw JSON tree.
    await expect(drawer.getByTitle("Provider stop reason")).toBeVisible();
    await expect(drawer.getByTitle("Provider stop reason")).toHaveText("end_turn");

    // The Input section is expanded by default: role-tagged cards are visible
    // immediately, and the toggle can still tuck them away. "Prompt" and "Input"
    // used to be separate sections answering one question between them.
    const inputToggle = drawer.getByRole("button", { name: /^Input/ });
    await expect(inputToggle).toBeVisible();
    await expect(inputToggle).toHaveAttribute("aria-expanded", "true");

    await expect(drawer.getByText("system", { exact: true })).toBeVisible();
    await expect(drawer.getByText("user", { exact: true })).toBeVisible();
    // The message content renders through the OutputViewer.
    await expect(drawer.getByText(/customer needs help/i)).toBeVisible();

    // The model that answered is stated here rather than only inside the raw
    // metadata dump: a provider id is a config alias, not a model.
    await expect(drawer.getByText("gpt-4o-mini").first()).toBeVisible();

    // Switching to Raw shows the captured provider payload verbatim — the
    // sampling params included, which appear nowhere else in the drawer.
    const inputView = drawer.getByRole("radiogroup", { name: "Input view" });
    await inputView.getByRole("radio", { name: "Raw" }).click();
    await expect(drawer.getByText(/"max_tokens"/)).toBeVisible();
    await expect(drawer.getByText(/"temperature"/)).toBeVisible();
    await expect(drawer.getByText(/chat\/completions/)).toBeVisible();

    // ...and back, without losing the rendered turns.
    await inputView.getByRole("radio", { name: "Rendered" }).click();
    await expect(drawer.getByText("system", { exact: true })).toBeVisible();

    // Provider metadata section is expanded by default with its JSON tree.
    const rawToggle = drawer.getByRole("button", { name: /provider metadata/i });
    await expect(rawToggle).toBeVisible();
    await expect(rawToggle).toHaveAttribute("aria-expanded", "true");
    await expect(drawer.getByText(/"finish_reason"/)).toBeVisible();

    // Collapsing still works and hides the tree.
    await rawToggle.click();
    await expect(rawToggle).toHaveAttribute("aria-expanded", "false");
    await expect(drawer.getByText(/"finish_reason"/)).toHaveCount(0);
  });

  test("marks a truncated stop reason (max_tokens) on the meta line", async ({
    page,
  }) => {
    await page.goto(`/runs/${V2_RUN}?case=${V2_TRUNCATED_CASE}`);

    const drawer = page.getByRole("dialog");
    await expect(drawer).toBeVisible();
    await expect(drawer.getByTitle("Provider stop reason")).toBeVisible();
    await expect(drawer.getByTitle("Provider stop reason")).toHaveText("max_tokens");
  });

  test("a v1 case renders none of the schema-v2 sections or chips", async ({
    page,
  }) => {
    // The money run predates schema v2, so its case details carry no
    // prompt/stop_reason/raw — the drawer degrades to its pre-v2 shape.
    await page.goto(`/runs/${MONEY_RUN}?case=case-0000`);

    const drawer = page.getByRole("dialog");
    await expect(drawer).toBeVisible();
    // Confirm the drawer body loaded before asserting absence.
    await expect(drawer.getByRole("heading", { name: "Output" })).toBeVisible();

    // No prompt section, and — because a v1 case has neither a captured request
    // nor a rendered prompt — no Rendered/Raw toggle that would lead nowhere.
    await expect(drawer.getByRole("button", { name: /prompt/i })).toHaveCount(0);
    await expect(
      drawer.getByRole("radiogroup", { name: "Input view" }),
    ).toHaveCount(0);
    await expect(
      drawer.getByRole("button", { name: /provider metadata/i }),
    ).toHaveCount(0);
    await expect(drawer.getByTitle("Provider stop reason")).toHaveCount(0);
  });
});
