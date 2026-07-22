import { useState } from "react";
import { Link } from "react-router";
import { useApiKeys, useCreateApiKey, useRevokeApiKey } from "@/api/queries";
import type { ApiKeyCreatedResponse, ApiKeyView, AuthScope } from "@/api";
import { useAuth } from "@/auth/AuthProvider";
import { scopesAtMost } from "@/lib/authz";
import { formatDate, formatRelative } from "@/lib/format";
import { Button } from "@/components/ui/Button";
import { CopyButton } from "@/components/ui/CopyButton";
import { Modal, ModalActions } from "@/components/ui/Modal";
import { TextField } from "@/components/ui/TextField";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { EmptyState, ErrorState } from "@/components/States";
import { cn } from "@/lib/cn";

export function KeysPage() {
  const { view, isLoading } = useAuth();
  const allowedScopes = scopesAtMost(view.scope);
  const canManage = view.canWrite && allowedScopes.length > 0;

  const keysQuery = useApiKeys(canManage);
  const createKey = useCreateApiKey();
  const revokeKey = useRevokeApiKey();

  const [name, setName] = useState("");
  const [scope, setScope] = useState<AuthScope>("read");
  const [secret, setSecret] = useState<ApiKeyCreatedResponse | null>(null);
  const [revoking, setRevoking] = useState<ApiKeyView | null>(null);

  if (isLoading) {
    return (
      <div className="max-w-2xl space-y-5">
        <PageHeader />
        <CenteredSpinner label="Loading…" />
      </div>
    );
  }
  if (view.promptLogin) {
    return (
      <SignInPrompt message="Sign in to create and manage API keys." />
    );
  }
  if (!canManage) {
    return (
      <div className="max-w-2xl space-y-5">
        <PageHeader />
        <div className="rounded-xl border border-border bg-surface p-6 text-sm text-muted">
          Your access level does not permit managing API keys. A write or admin
          scope is required.
        </div>
      </div>
    );
  }

  async function onCreate(e: React.FormEvent) {
    e.preventDefault();
    const created = await createKey.mutateAsync({
      name: name.trim() || "Unnamed key",
      scope,
    });
    setSecret(created);
    setName("");
    setScope("read");
  }

  return (
    <div className="max-w-3xl space-y-5">
      <PageHeader />

      <section className="rounded-xl border border-border bg-surface p-4">
        <h2 className="text-sm font-semibold">Create a key</h2>
        <p className="mt-1 text-sm text-muted">
          The secret is shown once, immediately after creation. Store it
          somewhere safe.
        </p>
        <form
          onSubmit={onCreate}
          className="mt-3 flex flex-wrap items-end gap-3"
        >
          <div className="min-w-[12rem] flex-1">
            <TextField
              label="Name"
              placeholder="e.g. CI pipeline"
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
          </div>
          <label className="flex flex-col gap-1">
            <span className="text-sm font-medium text-fg">Scope</span>
            <select
              aria-label="Key scope"
              className="h-9 rounded-md border border-border bg-bg px-2 text-sm text-fg outline-none focus:ring-2 focus:ring-ring"
              value={scope}
              onChange={(e) => setScope(e.target.value as AuthScope)}
            >
              {allowedScopes.map((s) => (
                <option key={s} value={s}>
                  {s}
                </option>
              ))}
            </select>
          </label>
          <Button
            type="submit"
            variant="primary"
            disabled={createKey.isPending}
          >
            {createKey.isPending ? "Creating…" : "Create key"}
          </Button>
        </form>
        {createKey.isError ? (
          <p className="mt-2 text-sm text-fail">
            Could not create the key. Please try again.
          </p>
        ) : null}
      </section>

      <section className="overflow-hidden rounded-xl border border-border bg-surface">
        <header className="border-b border-border px-4 py-2.5 text-sm font-semibold">
          Your keys
        </header>
        {keysQuery.isPending ? (
          <CenteredSpinner label="Loading keys…" />
        ) : keysQuery.isError ? (
          <div className="p-4">
            <ErrorState
              error={keysQuery.error}
              onRetry={() => keysQuery.refetch()}
            />
          </div>
        ) : keysQuery.data.length === 0 ? (
          <div className="p-4">
            <EmptyState title="No API keys yet">
              Create one above to authenticate CI or scripts.
            </EmptyState>
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full min-w-[720px] text-sm">
              <thead>
                <tr className="border-b border-border text-left text-xs uppercase tracking-wide text-muted">
                  <th className="px-4 py-2 font-medium">Name</th>
                  <th className="px-3 py-2 font-medium">Prefix</th>
                  <th className="px-3 py-2 font-medium">Scope</th>
                  <th className="px-3 py-2 font-medium">Created</th>
                  <th className="px-3 py-2 font-medium">Last used</th>
                  <th className="px-3 py-2 font-medium">Status</th>
                  <th className="px-3 py-2 text-right font-medium">Actions</th>
                </tr>
              </thead>
              <tbody>
                {keysQuery.data.map((k) => (
                  <tr
                    key={k.id}
                    className="border-b border-border/60 last:border-0"
                  >
                    <td className="px-4 py-2 font-medium">{k.name}</td>
                    <td className="px-3 py-2 font-mono text-xs text-muted">
                      {k.prefix}…
                    </td>
                    <td className="px-3 py-2">
                      <ScopeChip scope={k.scope} />
                    </td>
                    <td className="px-3 py-2 text-muted">
                      {formatDate(k.created_at)}
                    </td>
                    <td className="px-3 py-2 text-muted">
                      {k.last_used_at ? formatRelative(k.last_used_at) : "never"}
                    </td>
                    <td className="px-3 py-2">
                      {k.revoked ? (
                        <span className="rounded-full bg-fail/12 px-2 py-0.5 text-[11px] font-medium text-fail ring-1 ring-inset ring-fail/25">
                          revoked
                        </span>
                      ) : (
                        <span className="rounded-full bg-pass/12 px-2 py-0.5 text-[11px] font-medium text-pass ring-1 ring-inset ring-pass/25">
                          active
                        </span>
                      )}
                    </td>
                    <td className="px-3 py-2 text-right">
                      <Button
                        variant="danger"
                        size="sm"
                        disabled={k.revoked}
                        onClick={() => setRevoking(k)}
                      >
                        Revoke
                      </Button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>

      {/* Secret reveal modal (once only). */}
      <Modal
        open={secret !== null}
        onOpenChange={(o) => !o && setSecret(null)}
        title="API key created"
        description="Copy this secret now — it will not be shown again."
      >
        {secret ? (
          <div className="space-y-3">
            <div className="rounded-lg border border-amber/30 bg-amber/8 px-3 py-2 text-xs text-amber">
              This is the only time the full key is displayed. If you lose it,
              revoke it and create a new one.
            </div>
            <div className="flex items-center gap-2 rounded-md border border-border bg-bg p-2">
              <code
                data-testid="api-key-secret"
                className="min-w-0 flex-1 break-all font-mono text-xs"
              >
                {secret.key}
              </code>
              <CopyButton value={secret.key} label="Copy key" />
            </div>
            <div className="text-xs text-muted">
              Name: <span className="text-fg">{secret.name}</span> · Scope:{" "}
              <span className="text-fg">{secret.scope}</span>
            </div>
            <div className="flex justify-end">
              <Button variant="primary" onClick={() => setSecret(null)}>
                Done
              </Button>
            </div>
          </div>
        ) : null}
      </Modal>

      {/* Revoke confirmation. */}
      <Modal
        open={revoking !== null}
        onOpenChange={(o) => !o && setRevoking(null)}
        title="Revoke API key"
        description={
          revoking
            ? `Revoke "${revoking.name}"? Any client using it will stop working.`
            : undefined
        }
      >
        <ModalActions
          onCancel={() => setRevoking(null)}
          confirmLabel="Revoke key"
          confirmVariant="danger"
          pending={revokeKey.isPending}
          onConfirm={async () => {
            if (!revoking) return;
            await revokeKey.mutateAsync(revoking.id);
            setRevoking(null);
          }}
        />
      </Modal>
    </div>
  );
}

function PageHeader() {
  return (
    <div>
      <h1 className="text-lg font-semibold tracking-tight">API keys</h1>
      <p className="text-sm text-muted">
        Programmatic access tokens for CI and scripts. Keys carry a scope no
        greater than your own.
      </p>
    </div>
  );
}

function ScopeChip({ scope }: { scope: AuthScope }) {
  return (
    <span
      className={cn(
        "rounded-full px-2 py-0.5 text-[11px] font-medium ring-1 ring-inset",
        scope === "admin"
          ? "bg-accent/12 text-accent ring-accent/25"
          : scope === "write"
            ? "bg-amber/12 text-amber ring-amber/25"
            : "bg-surface-2 text-muted ring-border",
      )}
    >
      {scope}
    </span>
  );
}

function SignInPrompt({ message }: { message: string }) {
  return (
    <div className="max-w-2xl space-y-5">
      <PageHeader />
      <div className="rounded-xl border border-border bg-surface p-6">
        <p className="text-sm text-muted">{message}</p>
        <Link to="/login" className="mt-3 inline-block">
          <Button variant="primary" size="sm">
            Go to sign in
          </Button>
        </Link>
      </div>
    </div>
  );
}
