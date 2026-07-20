import { useState } from "react";
import { Navigate, useNavigate } from "react-router";
import { useAuth } from "@/auth/AuthProvider";
import { Button } from "@/components/ui/Button";
import { TextField } from "@/components/ui/TextField";

export function SetupPage() {
  const { view, setup } = useAuth();
  const navigate = useNavigate();

  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  // Setup is only reachable on a fresh, un-provisioned server.
  if (!view.setupRequired) {
    return <Navigate to={view.hasRealSession ? "/" : "/login"} replace />;
  }

  async function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    if (password !== confirm) {
      setError("Passwords do not match.");
      return;
    }
    if (password.length < 4) {
      setError("Choose a password of at least 4 characters.");
      return;
    }
    setPending(true);
    try {
      await setup(username.trim(), password);
      navigate("/", { replace: true });
    } catch {
      setError("Setup failed. Please try again.");
      setPending(false);
    }
  }

  return (
    <div className="mx-auto max-w-sm py-10">
      <div className="rounded-xl border border-border bg-surface p-6 shadow-sm">
        <h1 className="text-lg font-semibold tracking-tight">
          Create the first admin
        </h1>
        <p className="mt-1 text-sm text-muted">
          This server has not been set up yet. Create the initial administrator
          account to get started.
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
            autoComplete="new-password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            required
          />
          <TextField
            label="Confirm password"
            type="password"
            autoComplete="new-password"
            value={confirm}
            onChange={(e) => setConfirm(e.target.value)}
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
            disabled={pending || !username.trim() || !password || !confirm}
          >
            {pending ? "Creating…" : "Create admin & continue"}
          </Button>
        </form>
      </div>
    </div>
  );
}
