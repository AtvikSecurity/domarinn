// In-memory auth backend for the fetch mock. It implements the accounts API
// contract (login/setup/logout, /auth/me, api keys, users) with mutable state
// so the whole accounts UI runs against the fixture with no Rust backend.
//
// Seeding is deliberate: by default the server is already set up and an
// unauthenticated request resolves to a static admin principal. This means the
// existing e2e specs (which never log in) keep working — they browse as an
// implicit admin, exactly as an `open`-mode dev server would behave.
//
// Every function that models a response wire shape returns (or is wrapped
// into, by the mock handler) the exact generated response type — see `@/api`.

import type {
  ApiKeyCreatedResponse,
  ApiKeyView,
  AuthScope,
  AuthSessionResponse,
  MeResponse,
  MeUser,
  Role,
  UserView,
} from "@/api";
import { scopeAtLeast } from "@/lib/authz";

interface UserRecord {
  id: string;
  username: string;
  password: string;
  role: Role;
  disabled: boolean;
  created_at: number; // epoch millis internally; RFC3339 on the wire
}

interface ApiKeyRecord {
  id: string;
  name: string;
  prefix: string;
  scope: AuthScope;
  created_at: number;
  last_used_at?: number;
  revoked: boolean;
  ownerId: string;
  secret: string;
}

interface MockAuthState {
  users: Map<string, UserRecord>;
  sessions: Map<string, string>; // session token -> user id
  apikeys: Map<string, ApiKeyRecord>; // key id -> record
  seq: number;
  setupCompleted: boolean;
}

/** Fixed timestamps so seeded rows are deterministic across reloads/tests. */
const SEED_TIME = Date.UTC(2026, 5, 1, 12, 0, 0);
const SETUP_FLAG = "measurellm.mock.setup";
const STATIC_ADMIN_ID = "u_admin";

function toIso(ms: number): string {
  return new Date(ms).toISOString();
}

function seededState(): MockAuthState {
  const users = new Map<string, UserRecord>();
  users.set(STATIC_ADMIN_ID, {
    id: STATIC_ADMIN_ID,
    username: "admin",
    password: "admin",
    role: "admin",
    disabled: false,
    created_at: SEED_TIME,
  });
  users.set("u_member", {
    id: "u_member",
    username: "member",
    password: "member",
    role: "member",
    disabled: false,
    created_at: SEED_TIME + 60_000,
  });
  return {
    users,
    sessions: new Map(),
    apikeys: new Map(),
    seq: 1,
    setupCompleted: false,
  };
}

let state = seededState();

/** Test hook: restore the seeded accounts + clear sessions/keys. */
export function resetMockAuth(): void {
  state = seededState();
}

// --- helpers ---------------------------------------------------------------

function nextId(prefix: string): string {
  return `${prefix}_${state.seq++}`;
}

function randomToken(prefix: string): string {
  const rand = () => Math.random().toString(36).slice(2, 12);
  return `${prefix}_${rand()}${rand()}`;
}

function scopeForRole(role: Role): AuthScope {
  return role === "admin" ? "admin" : "write";
}

function pubUser(u: UserRecord): MeUser {
  return { id: u.id, username: u.username, role: u.role };
}

function pubUserView(u: UserRecord): UserView {
  return {
    id: u.id,
    username: u.username,
    role: u.role,
    disabled: u.disabled,
    created_at: toIso(u.created_at),
  };
}

function pubKey(k: ApiKeyRecord): ApiKeyView {
  return {
    id: k.id,
    name: k.name,
    prefix: k.prefix,
    scope: k.scope,
    created_at: toIso(k.created_at),
    last_used_at: k.last_used_at !== undefined ? toIso(k.last_used_at) : null,
    revoked: k.revoked,
  };
}

function findByUsername(username: string): UserRecord | undefined {
  for (const u of state.users.values()) {
    if (u.username === username) return u;
  }
  return undefined;
}

function countActiveAdmins(excludeId?: string): number {
  let n = 0;
  for (const u of state.users.values()) {
    if (u.role === "admin" && !u.disabled && u.id !== excludeId) n++;
  }
  return n;
}

// --- setup gate ------------------------------------------------------------

/**
 * Whether first-run setup is required. Off by default; e2e specs flip it on via
 * localStorage before boot. Once setup completes it stays off for the session.
 */
export function setupRequired(): boolean {
  if (state.setupCompleted) return false;
  try {
    return localStorage.getItem(SETUP_FLAG) === "1";
  } catch {
    return false;
  }
}

// --- identity resolution ---------------------------------------------------

export interface ResolvedAuth {
  me: MeResponse;
  userId?: string;
}

/** Resolve a bearer token (or its absence) into an identity + scope. */
export function resolveAuth(token: string | null): ResolvedAuth {
  if (token) {
    const sessionUserId = state.sessions.get(token);
    if (sessionUserId) {
      const u = state.users.get(sessionUserId);
      if (u && !u.disabled) {
        return {
          me: {
            authenticated: true,
            user: pubUser(u),
            source: "session",
            scope: scopeForRole(u.role),
          },
          userId: u.id,
        };
      }
    }
    for (const k of state.apikeys.values()) {
      if (k.secret === token && !k.revoked) {
        k.last_used_at = Date.now();
        const owner = state.users.get(k.ownerId);
        return {
          me: {
            authenticated: true,
            user: owner ? pubUser(owner) : null,
            source: "apikey",
            scope: k.scope,
          },
          userId: k.ownerId,
        };
      }
    }
    // A token we don't recognise: report unauthenticated (never 401 from /me).
    return { me: { authenticated: false, user: null, source: "anonymous", scope: null } };
  }

  // No token. During first-run setup nobody is signed in; otherwise fall back
  // to the implicit static admin (dev/open-mode default).
  if (setupRequired()) {
    return { me: { authenticated: false, user: null, source: "anonymous", scope: null } };
  }
  const admin = state.users.get(STATIC_ADMIN_ID);
  return {
    me: {
      authenticated: true,
      user: admin ? pubUser(admin) : { id: STATIC_ADMIN_ID, username: "admin", role: "admin" },
      source: "static",
      scope: "admin",
    },
    userId: STATIC_ADMIN_ID,
  };
}

// --- auth actions ----------------------------------------------------------

export function login(username: string, password: string): AuthSessionResponse | null {
  const u = findByUsername(username);
  if (!u || u.disabled || u.password !== password) return null;
  const token = randomToken("sess");
  state.sessions.set(token, u.id);
  return { token, user: pubUserView(u) };
}

export function setup(username: string, password: string): AuthSessionResponse {
  const id = nextId("u");
  const rec: UserRecord = {
    id,
    username,
    password,
    role: "admin",
    disabled: false,
    created_at: Date.now(),
  };
  state.users.set(id, rec);
  state.setupCompleted = true;
  const token = randomToken("sess");
  state.sessions.set(token, id);
  return { token, user: pubUserView(rec) };
}

export function logout(token: string | null): void {
  if (token) state.sessions.delete(token);
}

// --- api keys --------------------------------------------------------------

export function listApiKeys(ownerId: string | undefined): ApiKeyView[] {
  return [...state.apikeys.values()]
    .filter((k) => !ownerId || k.ownerId === ownerId)
    .sort((a, b) => b.created_at - a.created_at)
    .map(pubKey);
}

export function createApiKey(
  ownerId: string | undefined,
  name: string | undefined,
  scope: AuthScope | undefined,
  callerScope: AuthScope,
): ApiKeyCreatedResponse {
  const wanted: AuthScope = scope ?? "read";
  // Clamp the granted scope to at most the caller's own scope.
  const granted: AuthScope = scopeAtLeast(callerScope, wanted)
    ? wanted
    : callerScope;
  const id = nextId("key");
  const secret = randomToken("mllm");
  const prefix = secret.slice(0, 12);
  const rec: ApiKeyRecord = {
    id,
    name: name?.trim() || "Unnamed key",
    prefix,
    scope: granted,
    created_at: Date.now(),
    revoked: false,
    ownerId: ownerId ?? STATIC_ADMIN_ID,
    secret,
  };
  state.apikeys.set(id, rec);
  return { key: secret, ...pubKey(rec) };
}

export function revokeApiKey(
  ownerId: string | undefined,
  id: string,
): boolean {
  const k = state.apikeys.get(id);
  if (!k) return false;
  if (ownerId && k.ownerId !== ownerId) return false;
  k.revoked = true;
  return true;
}

// --- users (admin) ---------------------------------------------------------

export function listUsers(): UserView[] {
  return [...state.users.values()]
    .sort((a, b) => a.created_at - b.created_at)
    .map(pubUserView);
}

export function createUser(
  username: string | undefined,
  password: string | undefined,
  role: Role | undefined,
): UserView | null {
  if (!username || findByUsername(username)) return null;
  const id = nextId("u");
  const rec: UserRecord = {
    id,
    username,
    password: password ?? "",
    role: role === "admin" ? "admin" : "member",
    disabled: false,
    created_at: Date.now(),
  };
  state.users.set(id, rec);
  return pubUserView(rec);
}

export interface UserPatch {
  role?: Role;
  disabled?: boolean;
  password?: string;
}

export type UserMutationResult = UserView | "last_admin" | "not_found";

export function updateUser(id: string, patch: UserPatch): UserMutationResult {
  const u = state.users.get(id);
  if (!u) return "not_found";
  const willBeAdmin = patch.role !== undefined ? patch.role === "admin" : u.role === "admin";
  const willBeDisabled =
    patch.disabled !== undefined ? !!patch.disabled : u.disabled;
  const wasActiveAdmin = u.role === "admin" && !u.disabled;
  const losesAdmin = wasActiveAdmin && (!willBeAdmin || willBeDisabled);
  if (losesAdmin && countActiveAdmins(id) === 0) return "last_admin";

  if (patch.role !== undefined) u.role = patch.role === "admin" ? "admin" : "member";
  if (patch.disabled !== undefined) u.disabled = !!patch.disabled;
  if (patch.password) u.password = patch.password;
  return pubUserView(u);
}

export function deleteUser(id: string): UserMutationResult {
  const u = state.users.get(id);
  if (!u) return "not_found";
  if (u.role === "admin" && !u.disabled && countActiveAdmins(id) === 0) {
    return "last_admin";
  }
  state.users.delete(id);
  return pubUserView(u);
}
