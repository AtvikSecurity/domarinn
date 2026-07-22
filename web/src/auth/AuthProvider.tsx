import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useReducer,
  type ReactNode,
} from "react";
import { useQueryClient } from "@tanstack/react-query";
import { apiRequest } from "@/api/client";
import { qk, useMe, useMeta } from "@/api/queries";
import type { AuthSessionResponse, MetaResponse, MeResponse } from "@/api";
import { clearToken, getToken, onAuthChange, onUnauthorized } from "@/lib/auth";
import { deriveAuthView, type AuthView } from "@/lib/authz";
import { authTokenReducer, initialAuthTokenState } from "./reducer";

interface AuthContextValue {
  view: AuthView;
  meta: MetaResponse | undefined;
  me: MeResponse | undefined;
  token: string | null;
  isLoading: boolean;
  refetchMe: () => void;
  login: (username: string, password: string) => Promise<AuthSessionResponse>;
  setup: (username: string, password: string) => Promise<AuthSessionResponse>;
  logout: () => Promise<void>;
}

const AuthContext = createContext<AuthContextValue | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient();
  const [tokenState, dispatch] = useReducer(
    authTokenReducer,
    getToken(),
    initialAuthTokenState,
  );

  // Keep the in-memory token mirror + `me` in sync with any external write
  // (Settings page, the fallback token modal) that goes through lib/auth.
  useEffect(
    () =>
      onAuthChange(() => {
        dispatch({ type: "sync", token: getToken() });
        void queryClient.invalidateQueries({ queryKey: qk.me });
      }),
    [queryClient],
  );

  // An expired session cookie (or any protected call) surfaces as a 401.
  // Refresh `me` so the derived view flips to unauthenticated and the nav/UI
  // reflect it. `/auth/me` itself never 401s (skipAuthRedirect), so no loop.
  // The actual redirect to /login is driven inside the router by
  // `useUnauthorizedRedirect` (Layout), which fires on the same signal.
  useEffect(
    () =>
      onUnauthorized(() => {
        void queryClient.invalidateQueries({ queryKey: qk.me });
      }),
    [queryClient],
  );

  const metaQuery = useMeta();
  const meQuery = useMe();

  // login/setup now authenticate via the HttpOnly session cookie the server
  // sets; the response token is no longer persisted (localStorage stays a
  // fallback for manually-entered static tokens / API keys only). Any token
  // left over from before this migration (a legacy `mses_`/`sess_` session
  // token) MUST be cleared — apiRequest still attaches a stored token as a
  // bearer header, and the server lets the header win over the cookie, so a
  // stale token would shadow the new cookie session and trap the user in a
  // login loop. Then invalidate all queries so every view refetches under the
  // new identity.
  const login = useCallback(
    async (username: string, password: string) => {
      const res = await apiRequest<AuthSessionResponse>("/auth/login", {
        method: "POST",
        body: { username, password },
        skipAuthRedirect: true,
      });
      clearToken();
      await queryClient.invalidateQueries();
      return res;
    },
    [queryClient],
  );

  const setup = useCallback(
    async (username: string, password: string) => {
      const res = await apiRequest<AuthSessionResponse>("/auth/setup", {
        method: "POST",
        body: { username, password },
        skipAuthRedirect: true,
      });
      clearToken();
      await queryClient.invalidateQueries();
      return res;
    },
    [queryClient],
  );

  const logout = useCallback(async () => {
    try {
      await apiRequest<void>("/auth/logout", {
        method: "POST",
        skipAuthRedirect: true,
      });
    } finally {
      // Drop any manual token, then reset every query so the previous user's
      // data never lingers AND the active meta/me queries refetch (documented
      // resetQueries semantics, unlike clear() which does not guarantee a
      // refetch of mounted observers). In closed mode the me refetch flips
      // `needsLogin`, and RequireAuth redirects to /login declaratively.
      clearToken();
      await queryClient.resetQueries();
    }
  }, [queryClient]);

  const view = useMemo(
    () => deriveAuthView(metaQuery.data, meQuery.data),
    [metaQuery.data, meQuery.data],
  );

  const refetchMe = useCallback(() => {
    void meQuery.refetch();
  }, [meQuery]);

  const value = useMemo<AuthContextValue>(
    () => ({
      view,
      meta: metaQuery.data,
      me: meQuery.data,
      token: tokenState.token,
      isLoading: metaQuery.isLoading || meQuery.isLoading,
      refetchMe,
      login,
      setup,
      logout,
    }),
    [
      view,
      metaQuery.data,
      metaQuery.isLoading,
      meQuery.data,
      meQuery.isLoading,
      tokenState.token,
      refetchMe,
      login,
      setup,
      logout,
    ],
  );

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth(): AuthContextValue {
  const ctx = useContext(AuthContext);
  if (!ctx) throw new Error("useAuth must be used within an AuthProvider");
  return ctx;
}

export function useAuthView(): AuthView {
  return useAuth().view;
}
