import { expect, test } from "@playwright/test";
import { MONEY_RUN } from "./helpers";

test("desktop navigation stays reachable in compact mode and remembers it", async ({ page }) => {
  await page.goto(`/runs/${MONEY_RUN}`);
  const nav = page.getByRole("navigation", { name: "Primary" });
  await expect(nav.getByRole("link", { name: "Runs" })).toHaveAttribute("aria-current", "page");
  const expandedWidth = (await nav.boundingBox())!.width;
  await page.getByRole("button", { name: "Collapse sidebar" }).click();
  expect((await nav.boundingBox())!.width).toBeLessThan(expandedWidth);
  await nav.getByRole("link", { name: "Cache" }).click();
  await expect(page).toHaveURL(/\/cache$/);
  await page.reload();
  await expect(page.getByRole("button", { name: "Expand sidebar" })).toBeVisible();
  const active = nav.getByRole("link", { name: "Cache" });
  await expect(active).toHaveAttribute("aria-current", "page");
  expect(await active.evaluate((el) => getComputedStyle(el).backgroundColor)).not.toBe(
    await nav.getByRole("link", { name: "Runs" }).evaluate((el) => getComputedStyle(el).backgroundColor),
  );
  await expect(page.getByRole("banner")).not.toContainText(/protect-writes|\bclosed\b|\bv\d+\.\d+/);
});

test("activity opens exact runs and shares filters with the table", async ({ page }) => {
  await page.goto("/runs?project=checkout-agent&suite=regression");
  const activity = page.getByRole("list", { name: "Run activity" });
  await expect(activity.getByRole("link", { name: MONEY_RUN, exact: true })).toHaveAttribute("href", `/runs/${MONEY_RUN}`);
  await expect(activity.getByRole("link", { name: "checkout-agent / regression" }).first()).toHaveAttribute("href", "/sets/checkout-agent/regression");
  await page.getByRole("radio", { name: "Table" }).click();
  await expect(page.getByRole("columnheader", { name: /Pass rate/ }).first()).toBeVisible();
  expect(new URL(page.url()).searchParams.get("project")).toBe("checkout-agent");
  expect(new URL(page.url()).searchParams.get("suite")).toBe("regression");
  expect(new URL(page.url()).searchParams.get("view")).toBe("table");
  await page.goBack();
  await expect(activity).toBeVisible();
});

test("a suite history point reveals the score on keyboard focus and opens its run", async ({ page }) => {
  await page.goto("/sets/checkout-agent/regression");
  const history = page.getByRole("region", { name: "Pass-rate history" });
  const point = history.getByRole("link", { name: new RegExp(MONEY_RUN) });
  await history.getByRole("link").first().hover();
  await point.focus();
  await expect(history.getByRole("status")).toContainText("72.0%");
  await point.press("Enter");
  await expect(page).toHaveURL(new RegExp(`/runs/${MONEY_RUN}$`));
  await expect(page.getByRole("link", { name: "View suite" })).toHaveAttribute("href", "/sets/checkout-agent/regression");
});
