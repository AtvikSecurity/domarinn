import { useState } from "react";
import { Navigate, useLocation, useNavigate } from "react-router";
import { ApiError, isMockEnabled } from "@/api/client";
import { useAuth } from "@/auth/AuthProvider";
import { Button } from "@/components/ui/Button";
import { TextField } from "@/components/ui/TextField";

interface LocationState {
  from?: { pathname?: string };
}

export function LoginPage() {
  const { view, login } = useAuth();
  const navigate = useNavigate();
  const location = useLocation();
  const from = (location.state as LocationState | null)?.from?.pathname ?? "/";

  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  // First-run servers must be set up before anyone can log in.
  if (view.setupRequired) return <Navigate to="/setup" replace />;
  // Already signed in with a real session — nothing to do here.
  if (view.hasRealSession) return <Navigate to={from} replace />;

  async function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setPending(true);
    try {
      await login(username.trim(), password);
      navigate(from, { replace: true });
    } catch (err) {
      setError(
        err instanceof ApiError && err.status === 401
          ? "Invalid username or password."
          : "Sign in failed. Please try again.",
      );
      setPending(false);
    }
  }

  return (
    <div className="mx-auto max-w-sm py-10">
      <div className="rounded-xl border border-border bg-surface p-6 shadow-sm">
        <h1 className="text-lg font-semibold tracking-tight">Sign in</h1>
        <p className="mt-1 text-sm text-muted">
          Sign in to manage runs, keys, and accounts.
        </p>

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
            disabled={pending || !username.trim() || !password}
          >
            {pending ? "Signing in…" : "Sign in"}
          </Button>
        </form>

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
