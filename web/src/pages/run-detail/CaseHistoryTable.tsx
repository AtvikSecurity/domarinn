import { useMemo } from "react";
import { Link } from "react-router";
import type { CaseHistoryPoint } from "@/api";
import { StatusBadge } from "@/components/StatusBadge";
import {
  OUTLINE_LABEL_BASE,
  OUTLINE_LABEL_TONE,
} from "@/components/ui/chrome";
import { Tooltip } from "@/components/ui/Tooltip";
import { ColumnGroup } from "@/components/ui/ColumnGroup";
import { ColumnPicker } from "@/components/ui/ColumnPicker";
import { ResizableTh } from "@/components/ui/ResizableTh";
import { type ColumnDef, visibleColumns } from "@/lib/tableColumns";
import { resetColumns, setColumnVisible, useColumnPrefs } from "@/lib/useColumnPrefs";
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
import { type SortAccessor, sortRows, STATUS_RANK } from "@/lib/sort";
import { useLocalSort } from "@/lib/useTableSort";
import { parseTimestamp } from "@/lib/format";

const HISTORY_TABLE_ID = "caseHistory";

/**
 * Percentage tracks, not pixels.
 *
 * This table lives inside the case drawer, whose width the user drags. Pixel
 * tracks under `table-layout: fixed` would sum past a narrow drawer and force a
 * horizontal scrollbar the table has never had; percentages keep the existing
 * behaviour of dividing whatever width the drawer currently is. A resize still
 * stores pixels, which override the share for that one column.
 */
const HISTORY_COLUMNS: ColumnDef[] = [
  { id: "run", label: "Run", track: "16%", min: 90, alwaysVisible: true },
  { id: "when", label: "When", track: "12%", min: 70 },
  { id: "status", label: "Status", track: "11%", min: 70 },
  { id: "score", label: "Score", track: "8%", min: 50, numeric: true },
  { id: "tokens", label: "Tokens", track: "10%", min: 60, numeric: true },
  { id: "cost", label: "Cost", track: "10%", min: 60, numeric: true },
  { id: "latency", label: "Latency", track: "10%", min: 60, numeric: true },
  { id: "commit", label: "Commit", track: "12%", min: 64, numeric: true },
  { id: "cfg", label: "Cfg", track: "11%", min: 60, numeric: true },
];

/** A history point paired with its own change markers, so a sort can never
 *  separate a row from the amber "changed here" flags computed against its
 *  chronological neighbour. */
interface HistoryRow {
  point: CaseHistoryPoint;
  change: ReturnType<typeof changePoints>[number];
}

/** Sortable columns. `commit`/`cfg` stay inert: they render digests, whose
 *  lexical order means nothing — the change markers are the content there.
 *  Sort state is local, not `?sort=`: this drawer lives on RunDetail, whose
 *  URL sort belongs to the case grid. */
const HISTORY_SORT_FIELDS: Record<string, SortAccessor<HistoryRow>> = {
  run: (r) => r.point.run_id,
  when: (r) => parseTimestamp(r.point.created_at),
  status: (r) => STATUS_RANK[r.point.status],
  score: (r) => r.point.score,
  tokens: (r) =>
    r.point.prompt_tokens == null && r.point.completion_tokens == null
      ? null
      : (r.point.prompt_tokens ?? 0) + (r.point.completion_tokens ?? 0),
  cost: (r) => r.point.cost_usd,
  latency: (r) => r.point.latency_ms,
};

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
  const rows: HistoryRow[] = points
    .map((p, i) => ({ point: p, change: changes[i]! }))
    .reverse();
  const sort = useLocalSort();
  // Cleared sort = the newest-first default (sortRows returns rows unchanged).
  const displayRows = sortRows(rows, sort.sorting, HISTORY_SORT_FIELDS);
  const colPrefs = useColumnPrefs(HISTORY_TABLE_ID);
  const shownCols = useMemo(
    () => visibleColumns(HISTORY_COLUMNS, colPrefs),
    [colPrefs],
  );
  const shownIds = useMemo(() => new Set(shownCols.map((c) => c.id)), [shownCols]);

  return (
    <div className="space-y-2">
      <div className="flex justify-end">
        <ColumnPicker
          columns={HISTORY_COLUMNS}
          prefs={colPrefs}
          onChange={(id, visible) => setColumnVisible(HISTORY_TABLE_ID, id, visible)}
          onReset={() => resetColumns(HISTORY_TABLE_ID)}
        />
      </div>
      <div className="max-h-64 overflow-y-auto overscroll-contain rounded-lg border border-border">
      <table className="w-full table-fixed border-separate border-spacing-0 text-[11px]">
        <ColumnGroup columns={HISTORY_COLUMNS} prefs={colPrefs} />
        <thead>
          <tr className="text-muted">
            {shownCols.map((c, i, arr) => (
              <ResizableTh
                isLast={i === arr.length - 1}
                key={c.id}
                def={c}
                tableId={HISTORY_TABLE_ID}
                prefs={colPrefs}
                sort={
                  HISTORY_SORT_FIELDS[c.id]
                    ? {
                        active: sort.sortFor(c.id),
                        onToggle: () => sort.toggle(c.id),
                      }
                    : undefined
                }
                className={cn(
                  "sticky top-0 z-10 whitespace-nowrap border-b border-border bg-surface-2 px-2 py-1.5 font-medium",
                  c.numeric ? "text-right" : "text-left",
                )}
              >
                {c.label}
              </ResizableTh>
            ))}
          </tr>
        </thead>
        <tbody>
          {displayRows.map(({ point: p, change }) => {
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
                {shownIds.has("run") && (
                  <td className="truncate whitespace-nowrap px-2 py-1">
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
                )}
                {shownIds.has("when") && (
                  <td className="truncate whitespace-nowrap px-2 py-1 text-muted">
                    <Tooltip content={formatDate(p.created_at)}>
                      <time dateTime={p.created_at}>
                        {formatRelative(p.created_at)}
                      </time>
                    </Tooltip>
                  </td>
                )}
                {shownIds.has("status") && (
                  <td className="px-2 py-1">
                    <StatusBadge status={p.status} size="xs" />
                  </td>
                )}
                {shownIds.has("score") && (
                  <td className="px-2 py-1 text-right tabular-nums">
                    {p.score != null ? p.score.toFixed(2) : "—"}
                  </td>
                )}
                {shownIds.has("tokens") && (
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
                )}
                {shownIds.has("cost") && (
                  <td className="px-2 py-1 text-right tabular-nums text-muted">
                    {formatCost(p.cost_usd)}
                  </td>
                )}
                {shownIds.has("latency") && (
                  <td className="px-2 py-1 text-right tabular-nums text-muted">
                    {p.latency_ms != null ? formatLatency(p.latency_ms) : "—"}
                  </td>
                )}
                {shownIds.has("commit") && (
                  <td className="px-2 py-1 text-right">
                    <ChangeCell
                      value={p.git_commit}
                      changed={change.commitChanged}
                      changedLabel="Suite commit changed at this run"
                      format={(v) => v.slice(0, 7)}
                    />
                  </td>
                )}
                {shownIds.has("cfg") && (
                  <td className="px-2 py-1 text-right">
                    <ChangeCell
                      value={p.config_digest}
                      changed={change.configChanged}
                      changedLabel="Resolved config changed at this run"
                      // `blake3:abcdef…` — the algorithm prefix is noise here.
                      format={(v) => v.replace(/^[a-z0-9]+:/, "").slice(0, 7)}
                    />
                  </td>
                )}
              </tr>
            );
          })}
        </tbody>
      </table>
      </div>
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
      <span
        className={cn(
          OUTLINE_LABEL_BASE,
          OUTLINE_LABEL_TONE.amber,
          "px-1 py-0.5 text-[10px] normal-case",
        )}
      >
        {short}
      </span>
    </Tooltip>
  );
}
