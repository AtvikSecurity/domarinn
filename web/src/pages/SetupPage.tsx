import { useState } from "react";
import { Navigate, useNavigate } from "react-router";
import { apiErrorMessage } from "@/api/errorMessage";
import { useAuth } from "@/auth/AuthProvider";
import { Button } from "@/components/ui/Button";
import { Card } from "@/components/ui/Card";
import { TextField } from "@/components/ui/TextField";

/** Mirrors the server's `MIN_PASSWORD_LEN` (`accounts.rs`). */
const MIN_PASSWORD_LEN = 8;

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
    if (password.length < MIN_PASSWORD_LEN) {
      setError(`Choose a password of at least ${MIN_PASSWORD_LEN} characters.`);
      return;
    }
    setPending(true);
    try {
      await setup(username.trim(), password);
      void navigate("/", { replace: true });
    } catch (err) {
      // The server's sentence ("username already exists", "setup has already
      // been completed") is the remediation; show it verbatim.
      setError(apiErrorMessage(err, "Setup failed. Please try again."));
      setPending(false);
    }
  }

  return (
    <div className="mx-auto max-w-sm py-10">
      <Card className="p-6">
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
            hint={`At least ${MIN_PASSWORD_LEN} characters.`}
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
      </Card>
    </div>
  );
}
