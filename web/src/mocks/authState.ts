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
  AuthMode,
  AuthScope,
  AuthSessionResponse,
  MeResponse,
  MeUser,
  Role,
  UserIdentityView,
  UserView,
} from "@/api";
import { scopeAtLeast } from "@/lib/authz";

interface IdentityRecord {
  provider: string;
  kind: "oidc" | "saml";
  subject: string;
  email?: string;
  last_login_at?: number;
}

interface UserRecord {
  id: string;
  username: string;
  /** Empty string models an SSO-only account with no password login. */
  password: string;
  role: Role;
  disabled: boolean;
  created_at: number; // epoch millis internally; RFC3339 on the wire
  email?: string;
  identities: IdentityRecord[];
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
const SETUP_FLAG = "domarinn.mock.setup";
/** e2e override: set to "closed"/"protect-writes"/"open" before boot. */
const AUTHMODE_FLAG = "domarinn.mock.authmode";
/**
 * Mock stand-in for the HttpOnly session cookie. The real browser sends the
 * cookie automatically; the fetch mock can't see cookies, so the "current
 * session" is persisted here and consulted by `resolveAuth`.
 */
const SESSION_FLAG = "domarinn.mock.session";
const STATIC_ADMIN_ID = "u_admin";

function readFlag(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeFlag(key: string, value: string | null): void {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    /* ignore */
  }
}

/** The effective mock auth mode (default "open" to keep legacy e2e green). */
export function mockAuthMode(): AuthMode {
  const raw = readFlag(AUTHMODE_FLAG);
  if (raw === "closed" || raw === "protect-writes" || raw === "open") return raw;
  return "open";
}

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
    identities: [],
  });
  // A pure local account (no SSO identity) so its role stays admin-editable.
  users.set("u_member", {
    id: "u_member",
    username: "member",
    password: "member",
    role: "member",
    disabled: false,
    created_at: SEED_TIME + 60_000,
    identities: [],
  });
  // An SSO-only account: no password, role managed by the IdP.
  users.set("u_sso", {
    id: "u_sso",
    username: "sso.only",
    password: "",
    role: "member",
    disabled: false,
    created_at: SEED_TIME + 180_000,
    email: "sso.only@example.com",
    identities: [
      {
        provider: "oidc:google",
        kind: "oidc",
        subject: "google-sub-ssoonly",
        email: "sso.only@example.com",
        last_login_at: SEED_TIME + 200_000,
      },
    ],
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

/** Test hook: restore seeded accounts + clear sessions/keys/cookie/mode. */
export function resetMockAuth(): void {
  state = seededState();
  writeFlag(SESSION_FLAG, null);
  writeFlag(AUTHMODE_FLAG, null);
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

function pubIdentities(u: UserRecord): UserIdentityView[] {
  return u.identities.map((i) => ({
    provider: i.provider,
    kind: i.kind,
    subject: i.subject,
    email: i.email ?? null,
    last_login_at: i.last_login_at !== undefined ? toIso(i.last_login_at) : null,
  }));
}

function roleManagedBy(u: UserRecord): string | null {
  return u.identities[0]?.provider ?? null;
}

function pubUser(u: UserRecord): MeUser {
  return {
    id: u.id,
    username: u.username,
    role: u.role,
    identities: pubIdentities(u),
    role_managed_by: roleManagedBy(u),
  };
}

function pubUserView(u: UserRecord): UserView {
  return {
    id: u.id,
    username: u.username,
    role: u.role,
    disabled: u.disabled,
    created_at: toIso(u.created_at),
    email: u.email ?? null,
    has_password: u.password !== "",
    identities: pubIdentities(u),
    role_managed_by: roleManagedBy(u),
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

const ANONYMOUS: MeResponse = {
  authenticated: false,
  user: null,
  source: "anonymous",
  scope: null,
};

function sessionUser(userId: string): ResolvedAuth | null {
  const u = state.users.get(userId);
  if (!u || u.disabled) return null;
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

/** Resolve a bearer token (or its absence) into an identity + scope. */
export function resolveAuth(token: string | null): ResolvedAuth {
  if (token) {
    const sessionUserId = state.sessions.get(token);
    if (sessionUserId) {
      const resolved = sessionUser(sessionUserId);
      if (resolved) return resolved;
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
    return { me: { ...ANONYMOUS } };
  }

  // No bearer token: consult the mock "cookie" session (set by login/setup).
  const cookieSession = readFlag(SESSION_FLAG);
  if (cookieSession) {
    const resolved = sessionUser(cookieSession);
    if (resolved) return resolved;
  }

  // No session at all. In closed mode (and during first-run setup) that means
  // anonymous; in open mode fall back to the implicit static admin so the
  // legacy e2e specs keep browsing as an open-mode dev server would.
  if (setupRequired() || mockAuthMode() === "closed") {
    return { me: { ...ANONYMOUS } };
  }
  const admin = state.users.get(STATIC_ADMIN_ID);
  return {
    me: {
      authenticated: true,
      user: admin
        ? pubUser(admin)
        : { id: STATIC_ADMIN_ID, username: "admin", role: "admin", identities: [], role_managed_by: null },
      source: "static",
      scope: "admin",
    },
    userId: STATIC_ADMIN_ID,
  };
}

// --- auth actions ----------------------------------------------------------

export function login(username: string, password: string): AuthSessionResponse | null {
  const u = findByUsername(username);
  // An SSO-only account (empty password) can never password-login.
  if (!u || u.disabled || u.password === "" || u.password !== password) {
    return null;
  }
  const token = randomToken("sess");
  state.sessions.set(token, u.id);
  writeFlag(SESSION_FLAG, u.id); // model the Set-Cookie the server sends
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
    identities: [],
  };
  state.users.set(id, rec);
  state.setupCompleted = true;
  const token = randomToken("sess");
  state.sessions.set(token, id);
  writeFlag(SESSION_FLAG, id);
  return { token, user: pubUserView(rec) };
}

export function logout(token: string | null): void {
  if (token) state.sessions.delete(token);
  writeFlag(SESSION_FLAG, null); // clear the mock session cookie
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
  const secret = randomToken("domarinn");
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
    identities: [],
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
