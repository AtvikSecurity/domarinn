import { expect, test } from "@playwright/test";

// These specs boot the mock server in "closed" auth mode (every page behind a
// login), the new default. The mock reads `domarinn.mock.authmode` before the
// app renders.
test.describe("Closed-mode auth gating", () => {
  test.beforeEach(async ({ page }) => {
    await page.addInitScript(() => {
      try {
        localStorage.setItem("domarinn.mock.authmode", "closed");
      } catch {
        /* ignore */
      }
    });
  });

  test("the root route redirects an anonymous visitor to /login", async ({
    page,
  }) => {
    await page.goto("/");
    await expect(page).toHaveURL(/\/login$/);
    await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();
  });

  test("a deep link is preserved across login", async ({ page }) => {
    await page.goto("/cache");
    await expect(page).toHaveURL(/\/login$/);

    await page.getByLabel("Username").fill("admin");
    await page.getByLabel("Password").fill("admin");
    await page.getByRole("button", { name: "Sign in" }).click();

    // Lands back on the originally-requested page, not the default home.
    await expect(page).toHaveURL(/\/cache$/);
  });

  test("an unknown path redirects to login without leaking that it 404s", async ({
    page,
  }) => {
    await page.goto("/does-not-exist");
    await expect(page).toHaveURL(/\/login$/);
    await expect(page.getByText(/not found/i)).toHaveCount(0);
  });

  test("SSO provider buttons render from meta", async ({ page }) => {
    await page.goto("/login");
    await expect(
      page.getByRole("button", { name: /Continue with Google/ }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: /Continue with Corp SSO/ }),
    ).toBeVisible();
  });

  test("an ?sso_error banner is shown and stripped from the URL", async ({
    page,
  }) => {
    await page.goto("/login?sso_error=access_denied&provider=google");
    await expect(page.getByRole("alert")).toHaveText(/cancelled or denied/i);
    // The error params are removed so a refresh won't resurrect the banner.
    await expect(page).toHaveURL(/\/login$/);
  });

  test("logout returns to the login screen", async ({ page }) => {
    await page.goto("/login");
    await page.getByLabel("Username").fill("admin");
    await page.getByLabel("Password").fill("admin");
    await page.getByRole("button", { name: "Sign in" }).click();
    await expect(page.getByRole("heading", { name: "Overview" })).toBeVisible();

    await page.getByRole("button", { name: "Log out" }).click();
    await expect(page).toHaveURL(/\/login$/);
    await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();
  });

  test("the SSO redirect round-trips the return_to parameter", async ({
    page,
  }) => {
    // Stub the server-side SSO start endpoint: it writes the mock session and
    // bounces back to the `return_to` path, exactly as the real callback does.
    await page.route("**/api/v1/auth/oidc/google/start*", async (route) => {
      const url = new URL(route.request().url());
      const returnTo = url.searchParams.get("return_to") ?? "/";
      await route.fulfill({
        status: 200,
        contentType: "text/html",
        body: `<!doctype html><script>
          localStorage.setItem("domarinn.mock.session", "u_admin");
          location.assign(${JSON.stringify(returnTo)});
        </script>`,
      });
    });

    await page.goto("/settings");
    await expect(page).toHaveURL(/\/login$/);
    await page.getByRole("button", { name: /Continue with Google/ }).click();

    // Back on the originally-requested deep link, now authenticated.
    await expect(page).toHaveURL(/\/settings$/);
    await expect(
      page.getByRole("heading", { name: "Settings" }),
    ).toBeVisible();
  });
});
