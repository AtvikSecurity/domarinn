// Token storage + a tiny event bus so the 401 handler in the api client can
// prompt the app to open a token modal from outside React.

const TOKEN_KEY = "domarinn.token";

// Sessions moved from a localStorage bearer token to an HttpOnly cookie. A
// token left over from the old scheme (an `mses_` real-server or `sess_` mock
// session token) would still be attached as a bearer header and — because the
// server lets the header win over the cookie — shadow the cookie session and
// lock the user out. Drop any such legacy session token once at module load;
// genuine manual entries (static tokens, `domarinn_` API keys) are preserved.
function dropLegacySessionToken(): void {
  try {
    const token = localStorage.getItem(TOKEN_KEY);
    if (token && (token.startsWith("mses_") || token.startsWith("sess_"))) {
      localStorage.removeItem(TOKEN_KEY);
    }
  } catch {
    /* ignore */
  }
}
dropLegacySessionToken();

export function getToken(): string | null {
  try {
    return localStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

export function setToken(token: string): void {
  try {
    localStorage.setItem(TOKEN_KEY, token);
  } catch {
    /* ignore quota / private-mode errors */
  }
  emitAuthChange();
}

export function clearToken(): void {
  try {
    localStorage.removeItem(TOKEN_KEY);
  } catch {
    /* ignore */
  }
  emitAuthChange();
}

type AuthListener = () => void;
const authListeners = new Set<AuthListener>();

/** Fired when the token changes (set/clear). */
export function onAuthChange(fn: AuthListener): () => void {
  authListeners.add(fn);
  return () => authListeners.delete(fn);
}

function emitAuthChange(): void {
  for (const fn of authListeners) fn();
}

type UnauthorizedListener = () => void;
const unauthorizedListeners = new Set<UnauthorizedListener>();

/** Fired when a request returns 401; the app opens the token modal. */
export function onUnauthorized(fn: UnauthorizedListener): () => void {
  unauthorizedListeners.add(fn);
  return () => unauthorizedListeners.delete(fn);
}

export function emitUnauthorized(): void {
  for (const fn of unauthorizedListeners) fn();
}
