import { expect, test, type Page } from "@playwright/test";

/**
 * The run-set browser, driven through the real app.
 *
 * The specs browse as the implicit static admin unless they sign in — see
 * src/mocks/authState.ts. `member` is the seeded non-admin who holds `manage`
 * over `support-bot` and only `view` over `search-rerank/ndcg-eval`, which is
 * what makes the two halves of the access gate observable from the UI.
 */
async function signIn(page: Page, username: string, password: string) {
  await page.goto("/login");
  await page.getByLabel("Username").fill(username);
  await page.getByLabel("Password").fill(password);
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByRole("heading", { name: "Overview" })).toBeVisible();
}

test.describe("Run sets", () => {
  test("drills from the listing down to a run, and back up the breadcrumb", async ({
    page,
  }) => {
    await page.goto("/");
    await page.getByRole("navigation", { name: "Primary" }).getByRole("link", { name: "Sets" }).click();
    await expect(page).toHaveURL(/\/sets$/);
    await expect(page.getByRole("heading", { name: "Run sets" })).toBeVisible();

    await page.getByRole("link", { name: "checkout-agent" }).click();
    await expect(page).toHaveURL(/\/sets\/checkout-agent$/);
    await expect(
      page.getByRole("heading", { name: "checkout-agent" }),
    ).toBeVisible();

    await page.getByRole("link", { name: "regression" }).click();
    await expect(page).toHaveURL(/\/sets\/checkout-agent\/regression$/);
    await expect(page.getByRole("heading", { name: "regression" })).toBeVisible();

    // The suite's runs are listed here, and lead into the run itself.
    await page.getByRole("link", { name: "checkout-agent-regression-12" }).click();
    await expect(page).toHaveURL(/\/runs\/checkout-agent-regression-12$/);

    // Back up the trail: the breadcrumb is the only way home from a suite.
    await page.goBack();
    const crumbs = page.getByRole("navigation", { name: "Breadcrumb" });
    await crumbs.getByRole("link", { name: "checkout-agent" }).click();
    await expect(page).toHaveURL(/\/sets\/checkout-agent$/);
    await crumbs.getByRole("link", { name: "Sets" }).click();
    await expect(page).toHaveURL(/\/sets$/);
  });

  test("says which projects are restricted", async ({ page }) => {
    await page.goto("/sets");
    await expect(page.getByTestId("set-row-support-bot")).toContainText(
      "restricted",
    );
    // A suite-level lock is not a project-level one, and the row must not
    // claim otherwise.
    await expect(page.getByTestId("set-row-search-rerank")).not.toContainText(
      "restricted",
    );
  });

  test("flags the suite that carries its own lock", async ({ page }) => {
    await page.goto("/sets/search-rerank");
    await expect(page.getByTestId("suite-row-ndcg-eval")).toContainText(
      "restricted",
    );
  });

  test("states the inherited restriction on a suite inside a locked project", async ({
    page,
  }) => {
    // The suite owns no restriction row, so the panel's own payload says
    // "open". What the reader needs to know is that the project's lock already
    // hides it — and that pressing the toggle would add a second lock.
    await page.goto("/sets/support-bot/faq-accuracy");
    await page.getByRole("button", { name: "Access" }).click();
    const panel = page.getByRole("dialog");

    await expect(panel).toContainText("restricted");
    await expect(panel.getByText(/inherited from support-bot/)).toBeVisible();
    await expect(
      panel.getByText(/Anyone who can read this server/),
    ).toHaveCount(0);
    await expect(
      panel.getByRole("button", { name: "Restrict suite" }),
    ).toBeVisible();
  });

  test("adds, re-levels and removes a grant", async ({ page }) => {
    await page.goto("/sets/checkout-agent");
    await page.getByRole("button", { name: "Access" }).click();

    const panel = page.getByRole("dialog");
    await expect(panel).toBeVisible();
    await expect(panel.getByText(/Nobody has been granted this set yet/)).toBeVisible();

    await panel.getByLabel("Add person").selectOption({ label: "member" });
    await panel.getByLabel("Level for the new grant").selectOption("upload");
    await panel.getByRole("button", { name: "Add" }).click();

    const level = panel.getByLabel("Level for member");
    await expect(level).toHaveValue("upload");

    await level.selectOption("manage");
    await expect(panel.getByLabel("Level for member")).toHaveValue("manage");

    await panel.getByRole("button", { name: "Remove member" }).click();
    await expect(panel.getByText(/Nobody has been granted this set yet/)).toBeVisible();
  });

  test("locking a set from the panel updates the page behind it", async ({
    page,
  }) => {
    await page.goto("/sets/checkout-agent");
    await page.getByRole("button", { name: "Access" }).click();
    const panel = page.getByRole("dialog");

    await panel.getByRole("button", { name: "Restrict project" }).click();
    // The confirm is a step inside the same modal, not a second dialog.
    await expect(page.getByRole("dialog")).toHaveCount(1);
    await expect(panel.getByText(/Restricting hides this project/)).toBeVisible();
    await panel.getByRole("button", { name: "Restrict project" }).click();

    // The toggle now offers the opposite action — the set is locked.
    await expect(panel.getByRole("button", { name: "Unlock project" })).toBeVisible();
    await page.keyboard.press("Escape");

    // The listing reflects it too, because the whole ["sets"] subtree is
    // invalidated rather than just the panel's own query.
    await page
      .getByRole("navigation", { name: "Breadcrumb" })
      .getByRole("link", { name: "Sets" })
      .click();
    await expect(page.getByTestId("set-row-checkout-agent")).toContainText(
      "restricted",
    );
  });
});

test.describe("Run sets as a non-admin", () => {
  test("offers the access panel only where the member holds manage", async ({
    page,
  }) => {
    await signIn(page, "member", "member");

    // Granted `manage` here: the panel is reachable, and the level pickers work
    // even though this account can never toggle the restriction.
    await page.goto("/sets/support-bot");
    await page.getByRole("button", { name: "Access" }).click();
    const panel = page.getByRole("dialog");
    await expect(panel.getByLabel("Level for member")).toBeVisible();
    await expect(
      panel.getByRole("button", { name: /Unlock|Restrict/ }),
    ).toHaveCount(0);
    await expect(panel.getByText(/Ask an admin to add new people/)).toBeVisible();
    await page.keyboard.press("Escape");

    // No grant at all on this open project — no panel to offer.
    await page.goto("/sets/checkout-agent");
    await expect(page.getByRole("link", { name: "regression" })).toBeVisible();
    await expect(page.getByRole("button", { name: "Access" })).toHaveCount(0);
  });
});
