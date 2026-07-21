import { beforeEach, describe, expect, it } from "vitest";
import {
  createApiKey,
  createUser,
  deleteUser,
  listApiKeys,
  listUsers,
  login,
  logout,
  resetMockAuth,
  resolveAuth,
  revokeApiKey,
  setup,
  updateUser,
} from "./authState";

beforeEach(() => resetMockAuth());

describe("resolveAuth", () => {
  it("treats a missing token as the static admin (open-mode default)", () => {
    const { me } = resolveAuth(null);
    expect(me.authenticated).toBe(true);
    expect(me.source).toBe("static");
    expect(me.scope).toBe("admin");
    expect(me.user?.username).toBe("admin");
  });

  it("reports an unknown token as unauthenticated (never 401)", () => {
    const { me } = resolveAuth("not-a-real-token");
    expect(me.authenticated).toBe(false);
  });
});

describe("login / logout", () => {
  it("issues a session for valid credentials and resolves it", () => {
    const res = login("admin", "admin");
    expect(res).not.toBeNull();
    const { me } = resolveAuth(res!.token);
    expect(me.authenticated).toBe(true);
    expect(me.source).toBe("session");
    expect(me.scope).toBe("admin");

    logout(res!.token);
    expect(resolveAuth(res!.token).me.authenticated).toBe(false);
  });

  it("resolves a member session to write scope", () => {
    const res = login("member", "member");
    expect(res).not.toBeNull();
    expect(resolveAuth(res!.token).me.scope).toBe("write");
  });

  it("rejects a wrong password", () => {
    expect(login("admin", "nope")).toBeNull();
  });
});

describe("setup", () => {
  it("creates an admin and a session", () => {
    const res = setup("root", "hunter2");
    expect(res.user.role).toBe("admin");
    expect(resolveAuth(res.token).me.scope).toBe("admin");
  });
});

describe("api keys", () => {
  it("returns the secret once, lists without it, and clamps scope to the caller", () => {
    const created = createApiKey("u_admin", "CI", "write", "admin");
    expect(created.key.startsWith("domarinn_")).toBe(true);
    expect(created.scope).toBe("write");

    const listed = listApiKeys("u_admin");
    expect(listed).toHaveLength(1);
    expect(listed[0]).not.toHaveProperty("key");
    expect(listed[0]?.prefix).toBe(created.prefix);
    expect(listed[0]?.revoked).toBe(false);

    // A write-scoped caller cannot mint an admin key.
    const clamped = createApiKey("u_member", "over", "admin", "write");
    expect(clamped.scope).toBe("write");
  });

  it("revokes a key", () => {
    const created = createApiKey("u_admin", "temp", "read", "admin");
    expect(revokeApiKey("u_admin", created.id)).toBe(true);
    expect(listApiKeys("u_admin")[0]?.revoked).toBe(true);
    // A revoked key no longer authenticates.
    expect(resolveAuth(created.key).me.authenticated).toBe(false);
  });
});

describe("users (admin)", () => {
  it("seeds an admin and a member", () => {
    const users = listUsers();
    expect(users.map((u) => u.username).sort()).toEqual(["admin", "member"]);
  });

  it("creates, updates, and rejects duplicate usernames", () => {
    const created = createUser("tester", "pw", "member");
    expect(created?.role).toBe("member");
    expect(createUser("tester", "pw", "member")).toBeNull();

    const promoted = updateUser(created!.id, { role: "admin" });
    expect(promoted).not.toBe("last_admin");
    expect(promoted).not.toBe("not_found");
    if (typeof promoted !== "string") expect(promoted.role).toBe("admin");
  });

  it("blocks removing the last active admin", () => {
    // The seeded "admin" is the only admin.
    expect(deleteUser("u_admin")).toBe("last_admin");
    expect(updateUser("u_admin", { role: "member" })).toBe("last_admin");
    expect(updateUser("u_admin", { disabled: true })).toBe("last_admin");

    // With a second admin, the guard releases.
    const second = createUser("admin2", "pw", "admin")!;
    expect(deleteUser("u_admin")).not.toBe("last_admin");
    expect(listUsers().some((u) => u.id === second.id)).toBe(true);
  });
});
