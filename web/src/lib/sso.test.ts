import { describe, expect, it } from "vitest";
import { buildSsoStartUrl, safeRedirectPath, ssoErrorMessage } from "./sso";

describe("safeRedirectPath", () => {
  it("accepts same-origin absolute paths", () => {
    expect(safeRedirectPath("/runs/abc")).toBe("/runs/abc");
    expect(safeRedirectPath("/runs/abc?tab=diff#x")).toBe("/runs/abc?tab=diff#x");
    expect(safeRedirectPath("/")).toBe("/");
  });

  it("rejects protocol-relative and absolute URLs", () => {
    expect(safeRedirectPath("//evil.example")).toBe("/");
    expect(safeRedirectPath("https://evil.example/x")).toBe("/");
    expect(safeRedirectPath("http://evil.example")).toBe("/");
  });

  it("rejects the backslash bypass (browsers normalize \\ to /)", () => {
    expect(safeRedirectPath("/\\evil.example")).toBe("/");
    expect(safeRedirectPath("\\\\evil.example")).toBe("/");
    expect(safeRedirectPath("\\/evil.example")).toBe("/");
  });

  it("rejects empty / missing input", () => {
    expect(safeRedirectPath("")).toBe("/");
    expect(safeRedirectPath(null)).toBe("/");
    expect(safeRedirectPath(undefined)).toBe("/");
    expect(safeRedirectPath("relative")).toBe("/");
  });
});

describe("buildSsoStartUrl", () => {
  it("uses ? for a bare login URL and encodes the path", () => {
    expect(
      buildSsoStartUrl("/api/v1/auth/oidc/google/start", "/runs/x?tab=y"),
    ).toBe("/api/v1/auth/oidc/google/start?return_to=%2Fruns%2Fx%3Ftab%3Dy");
  });

  it("uses & when the login URL already has a query string", () => {
    expect(buildSsoStartUrl("/start?foo=1", "/cache")).toBe(
      "/start?foo=1&return_to=%2Fcache",
    );
  });
});

describe("ssoErrorMessage", () => {
  it("maps known codes to specific copy", () => {
    expect(ssoErrorMessage("access_denied")).toMatch(/cancelled or denied/i);
    expect(ssoErrorMessage("email_not_allowed")).toMatch(/email domain/i);
    expect(ssoErrorMessage("replayed")).toMatch(/already used/i);
    expect(ssoErrorMessage("account_disabled")).toMatch(/disabled/i);
  });

  it("falls back to a generic message for unknown codes", () => {
    expect(ssoErrorMessage("who_knows")).toMatch(/single sign-on failed/i);
  });

  it("returns null for a missing code", () => {
    expect(ssoErrorMessage(null)).toBeNull();
    expect(ssoErrorMessage(undefined)).toBeNull();
    expect(ssoErrorMessage("")).toBeNull();
  });
});
