import { describe, expect, it } from "vitest";
import {
  canAdmin,
  canWrite,
  deriveAuthView,
  isAdminRole,
  scopeAtLeast,
  scopesAtMost,
} from "./authz";
import type { AuthMode, Meta, MeResponse } from "@/api/types";

function meta(auth_mode: AuthMode, setup_required = false): Meta {
  return {
    name: "measurellm",
    version: "test",
    auth_mode,
    setup_required,
    supported_schema_versions: [1],
  };
}

const staticAdmin: MeResponse = {
  authenticated: true,
  user: { id: "u_admin", username: "admin", role: "admin" },
  source: "static",
  scope: "admin",
};

const sessionMember: MeResponse = {
  authenticated: true,
  user: { id: "u_member", username: "member", role: "member" },
  source: "session",
  scope: "write",
};

const anonymous: MeResponse = {
  authenticated: false,
  source: "session",
  scope: "read",
};

describe("scopeAtLeast", () => {
  it("ranks read < write < admin", () => {
    expect(scopeAtLeast("admin", "write")).toBe(true);
    expect(scopeAtLeast("write", "write")).toBe(true);
    expect(scopeAtLeast("read", "write")).toBe(false);
    expect(scopeAtLeast("write", "admin")).toBe(false);
    expect(scopeAtLeast(undefined, "read")).toBe(false);
  });
});

describe("scopesAtMost", () => {
  it("returns scopes at or below the given one", () => {
    expect(scopesAtMost("admin")).toEqual(["read", "write", "admin"]);
    expect(scopesAtMost("write")).toEqual(["read", "write"]);
    expect(scopesAtMost("read")).toEqual(["read"]);
    expect(scopesAtMost(undefined)).toEqual(["read"]);
  });
});

describe("isAdminRole", () => {
  it("only admin is admin", () => {
    expect(isAdminRole("admin")).toBe(true);
    expect(isAdminRole("member")).toBe(false);
    expect(isAdminRole(undefined)).toBe(false);
  });
});

describe("canWrite", () => {
  it("is always true in open mode, scope-gated otherwise", () => {
    expect(canWrite(anonymous, "open")).toBe(true);
    expect(canWrite(anonymous, "protect-writes")).toBe(false);
    expect(canWrite(sessionMember, "protect-writes")).toBe(true);
    expect(canWrite(sessionMember, "closed")).toBe(true);
    expect(canWrite(undefined, "closed")).toBe(false);
  });
});

describe("canAdmin", () => {
  it("requires an authenticated admin-scoped principal", () => {
    expect(canAdmin(staticAdmin)).toBe(true);
    expect(canAdmin(sessionMember)).toBe(false);
    expect(canAdmin(anonymous)).toBe(false);
    expect(canAdmin(undefined)).toBe(false);
  });
});

describe("deriveAuthView", () => {
  it("open-mode static admin can write + admin without a real session", () => {
    const view = deriveAuthView(meta("open"), staticAdmin);
    expect(view.authenticated).toBe(true);
    expect(view.canWrite).toBe(true);
    expect(view.canAdmin).toBe(true);
    expect(view.needsLogin).toBe(false);
    expect(view.hasRealSession).toBe(false); // "static" source is not a real login
    expect(view.role).toBe("admin");
  });

  it("protect-writes anonymous needs login and cannot write", () => {
    const view = deriveAuthView(meta("protect-writes"), anonymous);
    expect(view.authenticated).toBe(false);
    expect(view.canWrite).toBe(false);
    expect(view.canAdmin).toBe(false);
    expect(view.needsLogin).toBe(true);
    expect(view.hasRealSession).toBe(false);
  });

  it("a session login is a real session", () => {
    const view = deriveAuthView(meta("protect-writes"), sessionMember);
    expect(view.hasRealSession).toBe(true);
    expect(view.canWrite).toBe(true);
    expect(view.canAdmin).toBe(false);
    expect(view.needsLogin).toBe(false);
  });

  it("surfaces setup_required and defaults scope to read when me is absent", () => {
    const view = deriveAuthView(meta("closed", true), undefined);
    expect(view.setupRequired).toBe(true);
    expect(view.scope).toBe("read");
    expect(view.authenticated).toBe(false);
  });
});
