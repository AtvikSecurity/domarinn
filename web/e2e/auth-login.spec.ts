import { expect, test } from "@playwright/test";

test.describe("Login", () => {
  test("wrong password shows an inline error and does not open the token modal", async ({
    page,
  }) => {
    await page.goto("/login");
    await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();

    await page.getByLabel("Username").fill("admin");
    await page.getByLabel("Password").fill("wrong-password");
    await page.getByRole("button", { name: "Sign in" }).click();

    // Inline error, still on /login, and the 401→token-modal path is suppressed.
    await expect(page.getByRole("alert")).toHaveText(/Invalid username or password/);
    await expect(page).toHaveURL(/\/login$/);
    await expect(
      page.getByRole("heading", { name: "Access token required" }),
    ).toHaveCount(0);
  });

  test("correct credentials establish a session and redirect home", async ({
    page,
  }) => {
    await page.goto("/login");

    await page.getByLabel("Username").fill("admin");
    await page.getByLabel("Password").fill("admin");
    await page.getByRole("button", { name: "Sign in" }).click();

    // Redirected to the runs list.
    await expect(page.getByRole("heading", { name: "Eval runs" })).toBeVisible();
    await expect(page).toHaveURL(/\/$/);

    // The session now rides the mock "cookie" (localStorage stand-in), not a
    // stored bearer token, and the nav offers "Log out".
    const session = await page.evaluate(() =>
      localStorage.getItem("domarinn.mock.session"),
    );
    expect(session).toBeTruthy();
    await expect(page.getByRole("button", { name: "Log out" })).toBeVisible();
  });
});

test.describe("First-run setup", () => {
  test("routes to setup, creates the first admin, and logs in", async ({
    page,
  }) => {
    // Flip the mock into "needs setup" mode before the app boots.
    await page.addInitScript(() => {
      try {
        localStorage.setItem("domarinn.mock.setup", "1");
      } catch {
        /* ignore */
      }
    });

    await page.goto("/");
    // The boot gate forces the setup flow.
    await expect(page).toHaveURL(/\/setup$/);
    await expect(
      page.getByRole("heading", { name: "Create the first admin" }),
    ).toBeVisible();

    await page.getByLabel("Username").fill("founder");
    await page.getByLabel("Password", { exact: true }).fill("s3cret!");
    await page.getByLabel("Confirm password").fill("s3cret!");
    await page.getByRole("button", { name: /Create admin/ }).click();

    // Setup completes, logs in, and lands on the runs list.
    await expect(page.getByRole("heading", { name: "Eval runs" })).toBeVisible();
    await expect(page.getByText("founder")).toBeVisible();
    await expect(page.getByRole("button", { name: "Log out" })).toBeVisible();
  });
});
