// Token storage + a tiny event bus so the 401 handler in the api client can
// prompt the app to open a token modal from outside React.

const TOKEN_KEY = "measurellm.token";

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
