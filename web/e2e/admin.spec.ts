import { expect, test } from "@playwright/test";

// The mock resolves an unauthenticated request to a static admin, so the admin
// panel is reachable without an explicit login (the "pre-seeded admin").

test.describe("Admin — user management", () => {
  test("creates a user and changes a role inline", async ({ page }) => {
    await page.goto("/admin");
    await expect(page.getByRole("heading", { name: "Users" })).toBeVisible();

    // Seeded accounts are listed.
    await expect(page.getByTestId("user-row-admin")).toBeVisible();
    await expect(page.getByTestId("user-row-member")).toBeVisible();

    // Create a new member.
    await page.getByLabel("Username").fill("tester");
    await page.getByLabel("Password").fill("pw123456");
    await page.getByLabel("New user role").selectOption("member");
    await page.getByRole("button", { name: "Create user" }).click();

    const testerRow = page.getByTestId("user-row-tester");
    await expect(testerRow).toBeVisible();

    // Promote the seeded member to admin via the inline role selector.
    const memberRole = page.getByRole("combobox", { name: "Role for member" });
    await memberRole.selectOption("admin");
    await expect(memberRole).toHaveValue("admin");
  });

  test("deleting the last admin is blocked with a graceful message", async ({
    page,
  }) => {
    await page.goto("/admin");
    await expect(page.getByRole("heading", { name: "Users" })).toBeVisible();

    // The seeded "admin" is the only admin, so deletion must be refused.
    await page
      .getByTestId("user-row-admin")
      .getByRole("button", { name: "Delete" })
      .click();
    await page.getByRole("button", { name: "Delete user" }).click();

    await expect(page.getByRole("alert")).toHaveText(
      /Cannot remove the last active admin/,
    );
    // The row survives the failed delete.
    await expect(page.getByTestId("user-row-admin")).toBeVisible();
  });
});
