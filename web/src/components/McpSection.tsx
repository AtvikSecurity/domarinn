import { Link } from "react-router";
import { CopyButton } from "@/components/ui/CopyButton";

/**
 * The MCP endpoint's operator surface.
 *
 * The endpoint is opt-in and an unauthenticated probe cannot tell "disabled"
 * from "wrong URL" — both answer with a JSON 404 — so the two states are shown
 * explicitly, each with the one command that moves the operator forward:
 * the connect line when it is on, the env var when it is off.
 */
export function McpSection({ enabled }: { enabled: boolean }) {
  const endpoint = `${window.location.origin}/api/v1/mcp`;
  const connect = `claude mcp add --transport http domarinn ${endpoint} --header "Authorization: Bearer $DOMARINN_TOKEN"`;

  if (!enabled) {
    return (
      <div className="space-y-3">
        <p className="text-sm text-muted">
          <span className="font-medium text-fg">Not enabled.</span> Agents cannot
          read this server's eval history. Start the server with the variable
          below and restart it to mount the endpoint.
        </p>
        <CommandRow value="DOMARINN_MCP_ENABLED=true" label="Copy variable" />
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2 text-sm">
        <dt className="text-muted">Status</dt>
        <dd className="font-medium text-pass">Accepting connections</dd>
        <dt className="text-muted">Endpoint</dt>
        <dd className="flex min-w-0 items-center gap-1">
          <code className="truncate font-mono text-xs">{endpoint}</code>
          <CopyButton value={endpoint} label="Copy endpoint" iconOnly />
        </dd>
        <dt className="text-muted">Access</dt>
        <dd>
          Read-only. Eight tools and three prompts over your eval history — no
          tool can start a run or change stored data.
        </dd>
      </dl>

      <div className="space-y-2">
        <p className="text-sm text-muted">
          Connect Claude Code, with a{" "}
          <Link to="/keys" className="text-fg underline underline-offset-2 hover:text-accent">
            read-scoped API key
          </Link>{" "}
          in <code className="font-mono text-xs">$DOMARINN_TOKEN</code>:
        </p>
        <CommandRow value={connect} label="Copy command" />
      </div>
    </div>
  );
}

/**
 * A shell line the operator is meant to run. Scrolls rather than wraps: a
 * broken-across-lines command invites a partial copy-paste.
 */
function CommandRow({ value, label }: { value: string; label: string }) {
  return (
    <div className="flex items-center gap-2 rounded-lg border border-border bg-bg py-2 pl-3 pr-2">
      <code className="min-w-0 flex-1 overflow-x-auto whitespace-pre font-mono text-xs text-fg">
        {value}
      </code>
      <CopyButton value={value} label={label} className="shrink-0" />
    </div>
  );
}
