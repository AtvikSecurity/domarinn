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
import { clearToken, getToken, onAuthChange, setToken } from "@/lib/auth";
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

  const metaQuery = useMeta();
  const meQuery = useMe();

  const login = useCallback(
    async (username: string, password: string) => {
      const res = await apiRequest<AuthSessionResponse>("/auth/login", {
        method: "POST",
        body: { username, password },
        skipAuthRedirect: true,
      });
      setToken(res.token); // fires onAuthChange -> sync + invalidate me
      await queryClient.invalidateQueries({ queryKey: qk.me });
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
      setToken(res.token);
      await queryClient.invalidateQueries({ queryKey: qk.meta });
      await queryClient.invalidateQueries({ queryKey: qk.me });
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
      clearToken();
      await queryClient.invalidateQueries({ queryKey: qk.me });
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
