import { describe, expect, it } from "vitest";
import {
  canAdmin,
  canWrite,
  deriveAuthView,
  isAdminRole,
  scopeAtLeast,
  scopesAtMost,
} from "./authz";
import type { AuthMode, MetaResponse, MeResponse } from "@/api";

function meta(auth_mode: AuthMode, setup_required = false): MetaResponse {
  return {
    name: "domarinn",
    version: "test",
    auth_mode,
    setup_required,
    sso_providers: [],
    supported_schema_versions: [1],
    result_schema_version: 1,
    cache: { max_entry_bytes: 1_048_576, max_bytes: 1_073_741_824, max_age_days: 30 },
    cache_tiers: [],
  mcp_enabled: false,
  };
}

const staticAdmin: MeResponse = {
  authenticated: true,
  user: {
    id: "u_admin",
    username: "admin",
    role: "admin",
    identities: [],
    role_managed_by: null,
  },
  source: "static",
  scope: "admin",
};

const sessionMember: MeResponse = {
  authenticated: true,
  user: {
    id: "u_member",
    username: "member",
    role: "member",
    identities: [],
    role_managed_by: null,
  },
  source: "session",
  scope: "write",
};

const anonymous: MeResponse = {
  authenticated: false,
  user: null,
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
    expect(view.promptLogin).toBe(false);
    expect(view.hasRealSession).toBe(false); // "static" source is not a real login
    expect(view.role).toBe("admin");
  });

  it("open-mode anonymous never prompts or needs login", () => {
    const view = deriveAuthView(meta("open"), anonymous);
    expect(view.needsLogin).toBe(false);
    expect(view.promptLogin).toBe(false);
  });

  it("protect-writes anonymous prompts login but can still read (no hard redirect)", () => {
    const view = deriveAuthView(meta("protect-writes"), anonymous);
    expect(view.authenticated).toBe(false);
    expect(view.canWrite).toBe(false);
    expect(view.canAdmin).toBe(false);
    // Reads are open in protect-writes, so no hard redirect...
    expect(view.needsLogin).toBe(false);
    // ...but signing in would grant more, so the soft prompt is on.
    expect(view.promptLogin).toBe(true);
    expect(view.hasRealSession).toBe(false);
  });

  it("closed-mode anonymous needs login (hard redirect)", () => {
    const view = deriveAuthView(meta("closed"), anonymous);
    expect(view.needsLogin).toBe(true);
    expect(view.promptLogin).toBe(true);
  });

  it("closed-mode session member neither needs nor is prompted to log in", () => {
    const view = deriveAuthView(meta("closed"), sessionMember);
    expect(view.needsLogin).toBe(false);
    expect(view.promptLogin).toBe(false);
    expect(view.hasRealSession).toBe(true);
  });

  it("both flags stay false while meta is still loading", () => {
    const view = deriveAuthView(undefined, undefined);
    expect(view.needsLogin).toBe(false);
    expect(view.promptLogin).toBe(false);
  });

  it("a session login is a real session", () => {
    const view = deriveAuthView(meta("protect-writes"), sessionMember);
    expect(view.hasRealSession).toBe(true);
    expect(view.canWrite).toBe(true);
    expect(view.canAdmin).toBe(false);
    expect(view.needsLogin).toBe(false);
    expect(view.promptLogin).toBe(false);
  });

  it("surfaces setup_required and defaults scope to read when me is absent", () => {
    const view = deriveAuthView(meta("closed", true), undefined);
    expect(view.setupRequired).toBe(true);
    expect(view.scope).toBe("read");
    expect(view.authenticated).toBe(false);
  });

  // Regression guard: the real `/auth/me` response always carries `user` and
  // `scope` as explicit `null` (never omitted, never `undefined`) for an
  // anonymous caller, and `source` can be the literal string "anonymous"
  // (see generated MeResponse.ts / IdentitySource.ts). The hand-written mock
  // types never modeled this; make sure the view degrades gracefully instead
  // of throwing on `me.user.role` or similar.
  it("handles the wire-accurate anonymous shape (null user/scope, source: anonymous)", () => {
    const wireAnonymous: MeResponse = {
      authenticated: false,
      user: null,
      source: "anonymous",
      scope: null,
    };
    const view = deriveAuthView(meta("protect-writes"), wireAnonymous);
    expect(view.authenticated).toBe(false);
    expect(view.user).toBeUndefined();
    expect(view.role).toBeUndefined();
    expect(view.scope).toBe("read");
    expect(view.source).toBe("anonymous");
    expect(view.canWrite).toBe(false);
    expect(view.canAdmin).toBe(false);
    // protect-writes: reads open, so a hard redirect is not required...
    expect(view.needsLogin).toBe(false);
    // ...but the soft login prompt is on.
    expect(view.promptLogin).toBe(true);
    expect(view.hasRealSession).toBe(false);
  });
});
