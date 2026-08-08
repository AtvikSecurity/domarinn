import { useMemo, useState } from "react";
import { Link } from "react-router";
import { useApiKeys, useCreateApiKey, useRevokeApiKey } from "@/api/queries";
import type { ApiKeyCreatedResponse, ApiKeyView, AuthScope } from "@/api";
import { useAuth } from "@/auth/AuthProvider";
import { scopesAtMost } from "@/lib/authz";
import { formatDate, formatRelative } from "@/lib/format";
import { Button } from "@/components/ui/Button";
import { Card } from "@/components/ui/Card";
import { Chip } from "@/components/ui/Chip";
import { CHROME_FRAME } from "@/components/ui/chrome";
import { CopyButton } from "@/components/ui/CopyButton";
import { Modal, ModalActions } from "@/components/ui/Modal";
import { TextField } from "@/components/ui/TextField";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { EmptyState, ErrorState } from "@/components/States";
import { ColumnGroup } from "@/components/ui/ColumnGroup";
import { ColumnPicker } from "@/components/ui/ColumnPicker";
import { ResizableTh } from "@/components/ui/ResizableTh";
import { type ColumnDef, visibleColumns } from "@/lib/tableColumns";
import { resetColumns, setColumnVisible, useColumnPrefs } from "@/lib/useColumnPrefs";
import { cn } from "@/lib/cn";

const KEYS_TABLE_ID = "keys";

/**
 * `status` is structural alongside the name and the revoke button: it is the
 * only thing that distinguishes a live key from a dead one, and a key list
 * that cannot say which is which is worse than no list.
 */
const KEY_COLUMNS: ColumnDef[] = [
  { id: "name", label: "Name", track: "auto", min: 120, alwaysVisible: true },
  { id: "prefix", label: "Prefix", track: "95px", min: 80 },
  { id: "scope", label: "Scope", track: "90px", min: 80 },
  { id: "created", label: "Created", track: "125px", min: 100 },
  { id: "last_used", label: "Last used", track: "105px", min: 90 },
  { id: "status", label: "Status", track: "100px", min: 85, alwaysVisible: true },
  { id: "actions", label: "Actions", track: "110px", min: 100, numeric: true, alwaysVisible: true },
];

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
  // Above the early returns below: hook order cannot depend on auth state.
  const colPrefs = useColumnPrefs(KEYS_TABLE_ID);
  const shownCols = useMemo(
    () => visibleColumns(KEY_COLUMNS, colPrefs),
    [colPrefs],
  );
  const shownIds = useMemo(() => new Set(shownCols.map((c) => c.id)), [shownCols]);

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
        <Card className="p-6 text-sm text-muted">
          Your access level does not permit managing API keys. A write or admin
          scope is required.
        </Card>
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

      <Card as="section">
        <h2 className="font-mono text-[10px] font-medium uppercase tracking-[0.12em] text-muted">
          Create a key
        </h2>
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
      </Card>

      <section className={cn(CHROME_FRAME, "overflow-hidden")}>
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
          <>
            <div className="flex justify-end px-4 pt-3">
              <ColumnPicker
                columns={KEY_COLUMNS}
                prefs={colPrefs}
                onChange={(id, visible) =>
                  setColumnVisible(KEYS_TABLE_ID, id, visible)
                }
                onReset={() => resetColumns(KEYS_TABLE_ID)}
              />
            </div>
            <div className="overflow-x-auto scroll-hint [--scroll-hint-bg:var(--bg)]">
              {/* 745px is the declared tracks' sum, kept under the page's own
                  `max-w-3xl` (768px) so the table scrolls rather than clips. */}
              <table className="w-full min-w-[745px] table-fixed text-sm">
                <ColumnGroup columns={KEY_COLUMNS} prefs={colPrefs} />
                <thead>
                  <tr className="border-b border-border text-left text-xs uppercase tracking-wide text-muted">
                    {shownCols.map((c, i, arr) => (
                      <ResizableTh
                        isLast={i === arr.length - 1}
                        key={c.id}
                        def={c}
                        tableId={KEYS_TABLE_ID}
                        prefs={colPrefs}
                        className={cn(
                          "py-2 font-medium",
                          c.id === "name" ? "px-4" : "px-3",
                          c.numeric && "text-right",
                        )}
                      >
                        {c.label}
                      </ResizableTh>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {keysQuery.data.map((k) => (
                    <tr
                      key={k.id}
                      className="border-b border-border/60 last:border-0"
                    >
                      {shownIds.has("name") && (
                        <td className="truncate px-4 py-2 font-medium">{k.name}</td>
                      )}
                      {shownIds.has("prefix") && (
                        <td className="truncate px-3 py-2 font-mono text-xs text-muted">
                          {k.prefix}…
                        </td>
                      )}
                      {shownIds.has("scope") && (
                        <td className="px-3 py-2">
                          <ScopeChip scope={k.scope} />
                        </td>
                      )}
                      {shownIds.has("created") && (
                        <td className="truncate px-3 py-2 text-muted">
                          {formatDate(k.created_at)}
                        </td>
                      )}
                      {shownIds.has("last_used") && (
                        <td className="truncate px-3 py-2 text-muted">
                          {k.last_used_at ? formatRelative(k.last_used_at) : "never"}
                        </td>
                      )}
                      {shownIds.has("status") && (
                        <td className="px-3 py-2">
                          {k.revoked ? (
                            <Chip tone="fail">revoked</Chip>
                          ) : (
                            <Chip tone="pass">active</Chip>
                          )}
                        </td>
                      )}
                      {shownIds.has("actions") && (
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
                      )}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </>
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
        footer={
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
        }
      />
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
    <Chip tone={scope === "admin" ? "accent" : scope === "write" ? "amber" : "neutral"}>
      {scope}
    </Chip>
  );
}

function SignInPrompt({ message }: { message: string }) {
  return (
    <div className="max-w-2xl space-y-5">
      <PageHeader />
      <Card className="p-6">
        <p className="text-sm text-muted">{message}</p>
        <Link to="/login" className="mt-3 inline-block">
          <Button variant="primary" size="sm">
            Go to sign in
          </Button>
        </Link>
      </Card>
    </div>
  );
}
