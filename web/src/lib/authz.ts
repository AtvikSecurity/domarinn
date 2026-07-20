// Pure authorization helpers. These are framework-free so they can be unit
// tested in isolation and reused by both the React auth context and the mock.

import type {
  AuthMode,
  AuthScope,
  IdentitySource,
  MetaResponse,
  MeResponse,
  MeUser,
  Role,
} from "@/api";

/** Ordering of scopes; a higher rank subsumes the lower ones. */
export const SCOPE_RANK: Record<AuthScope, number> = {
  read: 0,
  write: 1,
  admin: 2,
};

export const ALL_SCOPES: AuthScope[] = ["read", "write", "admin"];

/** True when `have` is at least as privileged as `need`. */
export function scopeAtLeast(
  have: AuthScope | undefined,
  need: AuthScope,
): boolean {
  if (!have) return false;
  return SCOPE_RANK[have] >= SCOPE_RANK[need];
}

/**
 * The scopes a principal may grant to a new API key: anything at or below its
 * own scope. A read-scoped key can only mint read keys, etc.
 */
export function scopesAtMost(scope: AuthScope | undefined): AuthScope[] {
  const rank = scope ? SCOPE_RANK[scope] : 0;
  return ALL_SCOPES.filter((s) => SCOPE_RANK[s] <= rank);
}

export function isAdminRole(role: Role | undefined): boolean {
  return role === "admin";
}

/**
 * Can this principal perform write actions? In `open` mode writes are always
 * allowed (accounts optional); otherwise the principal needs write scope.
 */
export function canWrite(
  me: MeResponse | undefined,
  mode: AuthMode | undefined,
): boolean {
  if (mode === "open") return true;
  return scopeAtLeast(me?.scope ?? undefined, "write");
}

/** Admin actions always require an authenticated principal with admin scope. */
export function canAdmin(me: MeResponse | undefined): boolean {
  return !!me?.authenticated && scopeAtLeast(me?.scope ?? undefined, "admin");
}

/** A flattened, presentation-ready view of the current auth state. */
export interface AuthView {
  authMode?: AuthMode;
  authenticated: boolean;
  user?: MeUser;
  role?: Role;
  scope: AuthScope;
  source?: IdentitySource;
  setupRequired: boolean;
  canWrite: boolean;
  canAdmin: boolean;
  /** True when the server requires auth for reads and nobody is signed in. */
  needsLogin: boolean;
  /** True when identity comes from a real login (session/apikey), not the
   *  implicit open-mode principal. Used to decide whether /login redirects. */
  hasRealSession: boolean;
}

/** Derive the presentation view from the two server truths: meta + me. */
export function deriveAuthView(
  meta: MetaResponse | undefined,
  me: MeResponse | undefined,
): AuthView {
  const authMode = meta?.auth_mode;
  const authenticated = !!me?.authenticated;
  const scope: AuthScope = me?.scope ?? "read";
  const source = me?.source;
  return {
    authMode,
    authenticated,
    user: me?.user ?? undefined,
    role: me?.user?.role,
    scope,
    source,
    setupRequired: !!meta?.setup_required,
    canWrite: canWrite(me, authMode),
    canAdmin: canAdmin(me),
    needsLogin:
      !authenticated && authMode !== undefined && authMode !== "open",
    hasRealSession:
      authenticated && source !== undefined && source !== "static",
  };
}
