import { useEffect, useState } from "react";
import {
  Navigate,
  useLocation,
  useNavigate,
  useSearchParams,
} from "react-router";
import { ApiError, isMockEnabled } from "@/api/client";
import { useAuth } from "@/auth/AuthProvider";
import { Button } from "@/components/ui/Button";
import { TextField } from "@/components/ui/TextField";
import { ProviderIcon } from "@/components/icons/ProviderIcon";
import { buildSsoStartUrl, safeRedirectPath, ssoErrorMessage } from "@/lib/sso";

interface LocationState {
  from?: { pathname?: string; search?: string; hash?: string };
}

/** Rebuild the full path (incl. query + hash) the user was heading to. */
function destinationFrom(state: LocationState | null): string {
  const from = state?.from;
  if (!from?.pathname) return "/";
  return `${from.pathname}${from.search ?? ""}${from.hash ?? ""}`;
}

export function LoginPage() {
  const { view, meta, login } = useAuth();
  const navigate = useNavigate();
  const location = useLocation();
  const [searchParams, setSearchParams] = useSearchParams();
  const dest = destinationFrom(location.state as LocationState | null);

  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [redirecting, setRedirecting] = useState(false);
  // An SSO callback failure lands here as ?sso_error=<code>. Read it once into
  // state and strip it from the URL so a refresh/back doesn't resurrect it.
  const [ssoError] = useState(() =>
    ssoErrorMessage(searchParams.get("sso_error")),
  );
  useEffect(() => {
    if (searchParams.has("sso_error") || searchParams.has("provider")) {
      const next = new URLSearchParams(searchParams);
      next.delete("sso_error");
      next.delete("provider");
      setSearchParams(next, { replace: true });
    }
    // Run once on mount; searchParams is intentionally not a dep.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // When the user presses Back from the IdP, browsers restore this page from
  // the bfcache with `redirecting` still true, leaving every control disabled.
  // Reset it on a persisted pageshow so the form is usable again.
  useEffect(() => {
    function onPageShow(event: PageTransitionEvent) {
      if (event.persisted) setRedirecting(false);
    }
    window.addEventListener("pageshow", onPageShow);
    return () => window.removeEventListener("pageshow", onPageShow);
  }, []);

  // First-run servers must be set up before anyone can log in.
  if (view.setupRequired) return <Navigate to="/setup" replace />;
  // Already signed in with a real session — nothing to do here.
  if (view.hasRealSession) return <Navigate to={dest} replace />;

  const providers = meta?.sso_providers ?? [];
  const showLocalForm = true; // password login is always available today

  async function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setPending(true);
    try {
      await login(username.trim(), password);
      void navigate(dest, { replace: true });
    } catch (err) {
      setError(
        err instanceof ApiError && err.status === 401
          ? "Invalid username or password."
          : "Sign in failed. Please try again.",
      );
      setPending(false);
    }
  }

  function startSso(loginUrl: string) {
    setRedirecting(true);
    // A full-page navigation, NOT react-router: the flow leaves the SPA for
    // the IdP and returns via a server redirect.
    window.location.assign(buildSsoStartUrl(loginUrl, safeRedirectPath(dest)));
  }

  return (
    <div className="mx-auto max-w-sm py-10">
      <div className="rounded-xl border border-border bg-surface p-6 shadow-sm">
        <h1 className="text-lg font-semibold tracking-tight">Sign in</h1>
        <p className="mt-1 text-sm text-muted">
          Sign in to manage runs, keys, and accounts.
        </p>

        {ssoError ? (
          <div
            role="alert"
            className="mt-4 rounded-md border border-fail/30 bg-fail/5 px-3 py-2 text-sm text-fail"
          >
            {ssoError}
          </div>
        ) : null}

        {showLocalForm ? (
          <form onSubmit={onSubmit} className="mt-5 space-y-4">
            <TextField
              label="Username"
              autoComplete="username"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              autoFocus
              required
            />
            <TextField
              label="Password"
              type="password"
              autoComplete="current-password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              required
            />

            {error ? (
              <div
                role="alert"
                className="rounded-md border border-fail/30 bg-fail/5 px-3 py-2 text-sm text-fail"
              >
                {error}
              </div>
            ) : null}

            <Button
              type="submit"
              variant="primary"
              className="w-full"
              disabled={
                pending || redirecting || !username.trim() || !password
              }
            >
              {pending ? "Signing in…" : "Sign in"}
            </Button>
          </form>
        ) : null}

        {showLocalForm && providers.length > 0 ? (
          <div className="my-5 flex items-center gap-3" aria-hidden>
            <span className="h-px flex-1 bg-border" />
            <span className="text-xs text-muted">or continue with</span>
            <span className="h-px flex-1 bg-border" />
          </div>
        ) : null}

        {providers.length > 0 ? (
          <div className="space-y-2">
            {providers.map((provider) => (
              <Button
                key={`${provider.kind}:${provider.name}`}
                variant="secondary"
                className="w-full justify-center gap-2"
                disabled={pending || redirecting}
                onClick={() => startSso(provider.login_url)}
              >
                <ProviderIcon provider={provider} />
                Continue with {provider.label}
              </Button>
            ))}
          </div>
        ) : null}

        {!showLocalForm && providers.length === 0 ? (
          <p className="mt-5 text-center text-sm text-muted">
            No sign-in methods are configured on this server.
          </p>
        ) : null}

        {isMockEnabled() ? (
          <p className="mt-4 text-center text-xs text-muted">
            Demo accounts: <code className="font-mono">admin / admin</code> ·{" "}
            <code className="font-mono">member / member</code>
          </p>
        ) : null}
      </div>
    </div>
  );
}
