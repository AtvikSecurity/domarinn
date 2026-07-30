import { test as setup } from "@playwright/test";

/**
 * Log in once against the real server (POST the real login endpoint, not a
 * UI form fill) and save the resulting session cookie as storageState, so the
 * `capture` project can reuse it for every screenshot spec without repeating
 * the login flow.
 */

const AUTH_FILE = "screenshots/.auth/session.json";

const ADMIN_USER = process.env.DOMARINN_ADMIN_USER ?? "admin";
const ADMIN_PASSWORD = process.env.DOMARINN_ADMIN_PASSWORD ?? "screenshots";

setup("authenticate as the seeded admin", async ({ request }) => {
  const response = await request.post("/api/v1/auth/login", {
    data: { username: ADMIN_USER, password: ADMIN_PASSWORD },
  });
  if (!response.ok()) {
    throw new Error(
      `login as "${ADMIN_USER}" failed (${response.status()}): ${await response.text()}`,
    );
  }
  await request.storageState({ path: AUTH_FILE });
});
