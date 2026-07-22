import { expect, test } from "@playwright/test";
import {
  MONEY_RUN,
  V2_MESSAGES_CASE,
  V2_RUN,
  V2_TRUNCATED_CASE,
} from "./helpers";

test.describe("Case drawer schema-v2 sections", () => {
  test("shows the rendered prompt, stop reason, and provider metadata for a v2 case", async ({
    page,
  }) => {
    // A run of the one v2-flavored suite; case-0000 carries a role-tagged
    // system+user prompt, a clean end_turn stop, and raw provider metadata.
    await page.goto(`/runs/${V2_RUN}?case=${V2_MESSAGES_CASE}`);

    const drawer = page.getByRole("dialog");
    await expect(drawer).toBeVisible();

    // stop_reason chip on the meta line — visible without expanding anything.
    await expect(drawer.getByTitle("Provider stop reason")).toBeVisible();
    await expect(drawer.getByText("end_turn")).toBeVisible();

    // Prompt section starts collapsed; expanding it reveals role-tagged cards.
    const promptToggle = drawer.getByRole("button", { name: /prompt/i });
    await expect(promptToggle).toBeVisible();
    await expect(promptToggle).toHaveAttribute("aria-expanded", "false");
    await promptToggle.click();
    await expect(promptToggle).toHaveAttribute("aria-expanded", "true");

    await expect(drawer.getByText("system", { exact: true })).toBeVisible();
    await expect(drawer.getByText("user", { exact: true })).toBeVisible();
    // The message content renders through the OutputViewer.
    await expect(drawer.getByText(/customer needs help/i)).toBeVisible();

    // Provider metadata section starts collapsed; expanding shows a JSON tree.
    const rawToggle = drawer.getByRole("button", { name: /provider metadata/i });
    await expect(rawToggle).toBeVisible();
    await expect(rawToggle).toHaveAttribute("aria-expanded", "false");
    await rawToggle.click();
    await expect(rawToggle).toHaveAttribute("aria-expanded", "true");
    await expect(drawer.getByText(/"finish_reason"/)).toBeVisible();
  });

  test("marks a truncated stop reason (max_tokens) on the meta line", async ({
    page,
  }) => {
    await page.goto(`/runs/${V2_RUN}?case=${V2_TRUNCATED_CASE}`);

    const drawer = page.getByRole("dialog");
    await expect(drawer).toBeVisible();
    await expect(drawer.getByTitle("Provider stop reason")).toBeVisible();
    await expect(drawer.getByText("max_tokens")).toBeVisible();
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

    await expect(drawer.getByRole("button", { name: /prompt/i })).toHaveCount(0);
    await expect(
      drawer.getByRole("button", { name: /provider metadata/i }),
    ).toHaveCount(0);
    await expect(drawer.getByTitle("Provider stop reason")).toHaveCount(0);
  });
});
