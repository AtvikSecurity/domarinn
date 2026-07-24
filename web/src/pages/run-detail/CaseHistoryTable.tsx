import { Link } from "react-router";
import type { CaseHistoryPoint } from "@/api";
import { StatusBadge } from "@/components/StatusBadge";
import { Tooltip } from "@/components/ui/Tooltip";
import {
  formatCost,
  formatDate,
  formatLatency,
  formatRelative,
  formatTokens,
  shortRunId,
} from "@/lib/format";
import { changePoints } from "@/lib/history";
import { cn } from "@/lib/cn";

/**
 * The per-run detail behind the history rail.
 *
 * Every column here comes from data the client already had and discarded: six
 * of the twelve fields on a history point were fetched on every drawer open and
 * never rendered. The `commit` and `cfg` columns are the point of the table —
 * marking the run where the suite's git commit or resolved config digest
 * changed turns "it broke somewhere in the last 12 runs" into "it broke at this
 * commit, where the config also changed".
 */
export function CaseHistoryTable({
  points,
  runId,
  baselineRunId,
  caseKey,
}: {
  /** Chronological (oldest → newest); rendered newest-first. */
  points: readonly CaseHistoryPoint[];
  runId: string;
  baselineRunId: string | null;
  caseKey: string;
}) {
  const changes = changePoints(points);
  // Newest-first for reading, keeping each point paired with its own marker.
  const rows = points
    .map((p, i) => ({ point: p, change: changes[i]! }))
    .reverse();

  return (
    <div className="max-h-64 overflow-y-auto overscroll-contain rounded-lg border border-border">
      <table className="w-full border-separate border-spacing-0 text-[11px]">
        <thead>
          <tr className="text-muted">
            {["Run", "When", "Status", "Score", "Tokens", "Cost", "Latency", "Commit", "Cfg"].map(
              (h) => (
                <th
                  key={h}
                  scope="col"
                  className={cn(
                    "sticky top-0 z-10 whitespace-nowrap border-b border-border bg-surface-2 px-2 py-1.5 font-medium",
                    h === "Run" || h === "When" || h === "Status"
                      ? "text-left"
                      : "text-right",
                  )}
                >
                  {h}
                </th>
              ),
            )}
          </tr>
        </thead>
        <tbody>
          {rows.map(({ point: p, change }) => {
            const isCurrent = p.run_id === runId;
            const isBaseline = baselineRunId != null && p.run_id === baselineRunId;
            const tokens =
              p.prompt_tokens == null && p.completion_tokens == null
                ? null
                : (p.prompt_tokens ?? 0) + (p.completion_tokens ?? 0);
            return (
              <tr
                key={p.run_id}
                className={cn(
                  "border-b border-border/60",
                  isCurrent && "bg-accent/8",
                )}
              >
                <td className="whitespace-nowrap px-2 py-1">
                  <Link
                    to={`/runs/${encodeURIComponent(p.run_id)}?case=${encodeURIComponent(caseKey)}`}
                    className="font-mono text-accent hover:underline"
                  >
                    {shortRunId(p.run_id)}
                  </Link>
                  {isCurrent ? (
                    <span className="ml-1 text-muted">· current</span>
                  ) : null}
                  {isBaseline ? (
                    <span className="ml-1 text-muted">· baseline</span>
                  ) : null}
                </td>
                <td className="whitespace-nowrap px-2 py-1 text-muted">
                  <Tooltip content={formatDate(p.created_at)}>
                    <time dateTime={p.created_at}>
                      {formatRelative(p.created_at)}
                    </time>
                  </Tooltip>
                </td>
                <td className="px-2 py-1">
                  <StatusBadge status={p.status} size="xs" />
                </td>
                <td className="px-2 py-1 text-right tabular-nums">
                  {p.score != null ? p.score.toFixed(2) : "—"}
                </td>
                <td className="px-2 py-1 text-right tabular-nums text-muted">
                  {tokens != null ? (
                    <Tooltip
                      content={`${formatTokens(p.prompt_tokens ?? 0)} in · ${formatTokens(p.completion_tokens ?? 0)} out`}
                    >
                      <span>{formatTokens(tokens)}</span>
                    </Tooltip>
                  ) : (
                    "—"
                  )}
                </td>
                <td className="px-2 py-1 text-right tabular-nums text-muted">
                  {formatCost(p.cost_usd)}
                </td>
                <td className="px-2 py-1 text-right tabular-nums text-muted">
                  {p.latency_ms != null ? formatLatency(p.latency_ms) : "—"}
                </td>
                <td className="px-2 py-1 text-right">
                  <ChangeCell
                    value={p.git_commit}
                    changed={change.commitChanged}
                    changedLabel="Suite commit changed at this run"
                    format={(v) => v.slice(0, 7)}
                  />
                </td>
                <td className="px-2 py-1 text-right">
                  <ChangeCell
                    value={p.config_digest}
                    changed={change.configChanged}
                    changedLabel="Resolved config changed at this run"
                    // `blake3:abcdef…` — the algorithm prefix is noise here.
                    format={(v) => v.replace(/^[a-z0-9]+:/, "").slice(0, 7)}
                  />
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

/**
 * A short digest, amber-marked when it differs from the previous (older) run.
 * The marker is what makes the column scannable — the digests themselves are
 * unmemorable, so what matters is *where* they change.
 */
function ChangeCell({
  value,
  changed,
  changedLabel,
  format,
}: {
  value: string | null;
  changed: boolean;
  changedLabel: string;
  format: (v: string) => string;
}) {
  if (value == null) return <span className="text-muted">—</span>;
  const short = format(value);
  if (!changed) {
    return <span className="font-mono text-muted/70">{short}</span>;
  }
  return (
    <Tooltip content={`${changedLabel} (${value})`}>
      <span className="rounded bg-amber/12 px-1 py-px font-mono font-medium text-amber">
        {short}
      </span>
    </Tooltip>
  );
}
