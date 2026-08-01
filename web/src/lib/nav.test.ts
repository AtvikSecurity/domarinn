import { describe, expect, it } from "vitest";
import type { AuthView } from "./authz";
import { navItems } from "./nav";

/** Only the four fields `navItems` reads; the rest of AuthView is irrelevant here. */
function view(overrides: Partial<AuthView> = {}): AuthView {
  return {
    authenticated: true,
    scope: "read",
    setupRequired: false,
    canWrite: false,
    canAdmin: false,
    needsLogin: false,
    promptLogin: false,
    hasRealSession: true,
    ...overrides,
  };
}

const labels = (v: AuthView) => navItems(v).map((i) => i.label);

describe("navItems", () => {
  it("is empty for an anonymous visitor in closed mode", () => {
    // Every entry would be a dead link that bounces straight back to /login.
    expect(navItems(view({ needsLogin: true, authenticated: false }))).toEqual([]);
  });

  it("offers the four browse destinations plus settings by default", () => {
    expect(labels(view())).toEqual([
      "Overview",
      "Runs",
      "Sets",
      "Cache",
      "API keys",
      "Settings",
    ]);
  });

  it("hides API keys from someone who has not signed in", () => {
    // The page could only tell them to sign in.
    expect(labels(view({ promptLogin: true }))).not.toContain("API keys");
  });

  it("offers Admin only to admins", () => {
    expect(labels(view())).not.toContain("Admin");
    expect(labels(view({ canAdmin: true }))).toContain("Admin");
  });

  it("marks only Overview as an exact match", () => {
    // Every route starts with "/", so without `end` it would always be active.
    const items = navItems(view());
    expect(items.filter((i) => i.end).map((i) => i.to)).toEqual(["/"]);
  });
});
