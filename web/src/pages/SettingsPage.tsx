import { useEffect, useState, type ReactNode } from "react";
import { Link } from "react-router";
import { useMeta } from "@/api/queries";
import { clearToken, getToken, onAuthChange, setToken } from "@/lib/auth";
import { isMockEnabled } from "@/api/client";
import { useAuth } from "@/auth/AuthProvider";
import { Button } from "@/components/ui/Button";
import { Card as ChromeCard } from "@/components/ui/Card";
import { ThemeSegmented } from "@/components/ThemeToggle";
import { ProviderBadge } from "@/components/ProviderBadge";
import { McpSection } from "@/components/McpSection";

export function SettingsPage() {
  const meta = useMeta();
  const { view, logout } = useAuth();
  const [token, setTokenValue] = useState("");
  const [hasToken, setHasToken] = useState(!!getToken());
  const [loggingOut, setLoggingOut] = useState(false);

  useEffect(() => onAuthChange(() => setHasToken(!!getToken())), []);

  return (
    <div className="max-w-2xl space-y-6">
      <div>
        <h1 className="text-lg font-semibold tracking-tight">Settings</h1>
        <p className="text-sm text-muted">Account, appearance, and server info.</p>
      </div>

      <Card title="Account">
        {view.authenticated ? (
          <div className="flex flex-wrap items-center justify-between gap-4">
            <dl className="grid grid-cols-2 gap-x-6 gap-y-1 text-sm">
              <dt className="text-muted">Username</dt>
              <dd className="font-medium">{view.user?.username ?? "—"}</dd>
              <dt className="text-muted">Role</dt>
              <dd className="font-mono">
                {view.role ?? "—"}
                {view.user?.role_managed_by ? (
                  <span className="ml-1 font-sans text-xs text-muted">
                    (managed by{" "}
                    {view.user.role_managed_by.replace(/^.*:/, "")})
                  </span>
                ) : null}
              </dd>
              <dt className="text-muted">Source</dt>
              <dd className="font-mono">{view.source ?? "—"}</dd>
              <dt className="text-muted">Scope</dt>
              <dd className="font-mono">{view.scope}</dd>
              <dt className="text-muted">Sign-in methods</dt>
              <dd className="flex flex-wrap items-center gap-1.5">
                {(view.user?.identities?.length ?? 0) === 0 ? (
                  <span className="text-xs text-muted">password</span>
                ) : (
                  view.user?.identities.map((identity) => (
                    <ProviderBadge
                      key={`${identity.provider}:${identity.subject}`}
                      identity={identity}
                    />
                  ))
                )}
              </dd>
            </dl>
            <Button
              variant="secondary"
              onClick={async () => {
                setLoggingOut(true);
                try {
                  await logout();
                } finally {
                  setLoggingOut(false);
                }
              }}
              disabled={loggingOut}
            >
              {loggingOut ? "Signing out…" : "Log out"}
            </Button>
          </div>
        ) : (
          <div className="flex flex-wrap items-center justify-between gap-4">
            <p className="text-sm text-muted">You are not signed in.</p>
            <Link to="/login">
              <Button variant="primary">Sign in</Button>
            </Link>
          </div>
        )}
      </Card>

      <Card title="Appearance">
        <div className="flex items-center justify-between gap-4">
          <div className="text-sm text-muted">
            Theme follows your system by default; override it here.
          </div>
          <ThemeSegmented />
        </div>
      </Card>

      <Card title="Access token">
        <p className="text-sm text-muted">
          Browser sign-ins now use a secure session cookie — this field is a
          fallback for a static token or an API key, stored locally as{" "}
          <code className="font-mono text-xs">domarinn.token</code> and sent as a
          bearer header. {hasToken ? "A token is currently set." : "No token is set."}
        </p>
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <input
            type="password"
            value={token}
            onChange={(e) => setTokenValue(e.target.value)}
            placeholder={hasToken ? "•••••••• (set)" : "paste token"}
            className="h-9 flex-1 rounded-md border border-border bg-bg px-3 font-mono text-sm outline-none focus:ring-2 focus:ring-ring"
          />
          <Button
            variant="primary"
            onClick={() => {
              if (token.trim()) {
                setToken(token.trim());
                setTokenValue("");
              }
            }}
            disabled={!token.trim()}
          >
            Save
          </Button>
          <Button
            variant="secondary"
            onClick={() => {
              clearToken();
              setTokenValue("");
            }}
            disabled={!hasToken}
          >
            Clear
          </Button>
        </div>
      </Card>

      <Card title="MCP endpoint">
        <McpSection enabled={meta.data?.mcp_enabled ?? false} />
      </Card>

      <Card title="Server">
        <dl className="grid grid-cols-2 gap-y-2 text-sm">
          <dt className="text-muted">Name</dt>
          <dd className="font-mono">{meta.data?.name ?? "—"}</dd>
          <dt className="text-muted">Version</dt>
          <dd className="font-mono">{meta.data?.version ?? "—"}</dd>
          <dt className="text-muted">Auth mode</dt>
          <dd className="font-mono">{meta.data?.auth_mode ?? "—"}</dd>
          <dt className="text-muted">Schema versions</dt>
          <dd className="font-mono">
            {meta.data?.supported_schema_versions.join(", ") ?? "—"}
          </dd>
          <dt className="text-muted">Data source</dt>
          <dd className="font-mono">{isMockEnabled() ? "mock fixture" : "live API"}</dd>
        </dl>
      </Card>
    </div>
  );
}

function Card({ title, children }: { title: string; children: ReactNode }) {
  return (
    <ChromeCard as="section">
      <h2 className="mb-3 text-[10px] font-medium uppercase tracking-wide text-muted">
        {title}
      </h2>
      {children}
    </ChromeCard>
  );
}
