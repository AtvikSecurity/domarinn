import { expect, test } from "@playwright/test";

test.describe("API keys", () => {
  test("creating a key reveals the secret once, then it can be revoked", async ({
    page,
  }) => {
    await page.goto("/keys");
    await expect(page.getByRole("heading", { name: "API keys" })).toBeVisible();

    // Create a write-scoped key.
    await page.getByLabel("Name").fill("CI key");
    await page.getByLabel("Key scope").selectOption("write");
    await page.getByRole("button", { name: "Create key" }).click();

    // The secret is revealed exactly once, with a copy affordance + warning.
    await expect(
      page.getByRole("heading", { name: "API key created" }),
    ).toBeVisible();
    const secret = page.getByTestId("api-key-secret");
    await expect(secret).toHaveText(/^mllm_/);
    await expect(page.getByText(/only time the full key is displayed/)).toBeVisible();
    await expect(page.getByRole("button", { name: "Copy key" })).toBeVisible();

    // Dismiss the modal — the secret is gone for good.
    await page.getByRole("button", { name: "Done" }).click();
    await expect(page.getByTestId("api-key-secret")).toHaveCount(0);

    // The key is listed as active.
    const row = page.getByRole("row").filter({ hasText: "CI key" });
    await expect(row).toBeVisible();
    await expect(row.getByText("active")).toBeVisible();

    // Revoke it (with confirmation).
    await row.getByRole("button", { name: "Revoke" }).click();
    await expect(
      page.getByRole("heading", { name: "Revoke API key" }),
    ).toBeVisible();
    await page.getByRole("button", { name: "Revoke key" }).click();

    await expect(row.getByText("revoked")).toBeVisible();
  });
});
