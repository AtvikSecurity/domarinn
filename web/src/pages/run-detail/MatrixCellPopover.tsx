import type { ReactNode } from "react";
import * as RPopover from "@radix-ui/react-popover";
import type { MatrixCell } from "@/api";
import { Popover } from "@/components/ui/Popover";
import { StatusBadge } from "@/components/StatusBadge";
import { useCaseDetail } from "@/api/queries";
import { formatCost, formatLatency } from "@/lib/format";
import { cn } from "@/lib/cn";

/**
 * The per-cell popover: a compact summary of a matrix cell plus one row per
 * repeat (status/score/latency, fetched lazily) that deep-links into the case
 * drawer, and a footer that opens the cross-provider compare modal.
 *
 * Built on the shared `ui/Popover` (Radix) so it inherits focus + dismiss
 * behaviour; `RPopover.Close` is used on the interactive rows so acting on one
 * closes the popover before the drawer/modal takes over.
 */
export function MatrixCellPopover({
  runId,
  cell,
  testLabel,
  providerId,
  promptId,
  trigger,
  onSelectCase,
  onCompare,
}: {
  runId: string;
  cell: MatrixCell;
  testLabel: string;
  providerId: string;
  promptId: string | null;
  trigger: ReactNode;
  onSelectCase: (caseKey: string) => void;
  onCompare: () => void;
}) {
  return (
    <Popover trigger={trigger} align="center" className="w-64">
      <div className="-mx-1 -mt-1 border-b border-border px-3 py-2">
        <div className="truncate text-sm font-semibold" title={testLabel}>
          {testLabel}
        </div>
        <div className="truncate font-mono text-[11px] text-muted">
          {providerId}
          {promptId != null ? ` · ${promptId}` : ""}
        </div>
        <div className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-[11px] tabular-nums text-muted">
          <span>
            <span className="text-pass">{cell.passed}</span>/{cell.total} passed
          </span>
          {cell.score_mean != null ? <span>score {cell.score_mean.toFixed(2)}</span> : null}
          {cell.latency_ms_mean != null ? (
            <span>{formatLatency(cell.latency_ms_mean)}</span>
          ) : null}
          {cell.cost_usd != null ? <span>{formatCost(cell.cost_usd)}</span> : null}
        </div>
        {cell.distinct_outputs > 1 ? (
          <div className="mt-1 text-[11px] text-amber">
            {cell.distinct_outputs} distinct outputs across repeats
          </div>
        ) : null}
        {/* The column is keyed on the CONFIGURED provider, so a cell can be
            filled entirely by someone else's answers and still read as this
            provider's score. `0` covers both "nobody fell back" and a run
            stored before the attribution existed — honestly, since fallback did
            not exist then either. */}
        {cell.fallback_answered > 0 ? (
          <div className="mt-1 text-[11px] text-amber">
            {cell.fallback_answered} of {cell.total} answered by a fallback
          </div>
        ) : null}
      </div>

      <div className="max-h-64 overflow-y-auto p-1">
        {cell.case_keys.map((caseKey, i) => (
          <RepeatRow
            key={caseKey}
            runId={runId}
            caseKey={caseKey}
            repeatIdx={i}
            onSelect={onSelectCase}
          />
        ))}
      </div>

      <div className="-mx-1 -mb-1 border-t border-border p-1">
        <RPopover.Close asChild>
          <button
            type="button"
            onClick={onCompare}
            className="flex w-full items-center justify-center gap-1.5 rounded-md px-2 py-1.5 text-xs font-medium text-accent transition-colors hover:bg-surface-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            Compare across providers
          </button>
        </RPopover.Close>
      </div>
    </Popover>
  );
}

/** A single repeat row inside the cell popover. Fetches the case's detail (only
 *  while the popover — and hence this row — is mounted) to show its real
 *  status/score/latency; clicking opens the case drawer via `?case=`. */
function RepeatRow({
  runId,
  caseKey,
  repeatIdx,
  onSelect,
}: {
  runId: string;
  caseKey: string;
  repeatIdx: number;
  onSelect: (caseKey: string) => void;
}) {
  const detail = useCaseDetail(runId, caseKey);
  return (
    <RPopover.Close asChild>
      <button
        type="button"
        onClick={() => onSelect(caseKey)}
        className={cn(
          "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-xs transition-colors",
          "hover:bg-surface-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
        )}
      >
        <span className="shrink-0 font-mono text-[11px] text-muted">#{repeatIdx}</span>
        {detail.isPending ? (
          <span className="text-muted">loading…</span>
        ) : detail.isError ? (
          <span className="text-fail">unavailable</span>
        ) : detail.data ? (
          <>
            <StatusBadge status={detail.data.status} size="xs" />
            {/* Which repeats the cell's fallback count refers to. Without it a
                cell reading "1 of 2 answered by a fallback" names no repeat,
                and the two rows below it are indistinguishable. */}
            {detail.data.answered_by_provider_id ? (
              <span
                className="min-w-0 truncate font-mono text-[10px] text-amber"
                title="Answered by a fallback provider"
              >
                <span className="sr-only">answered by </span>
                {detail.data.answered_by_provider_id}
              </span>
            ) : null}
            <span className="ml-auto shrink-0 tabular-nums text-[11px] text-muted">
              {detail.data.score.toFixed(2)} · {formatLatency(detail.data.latency_ms)}
            </span>
          </>
        ) : null}
      </button>
    </RPopover.Close>
  );
}
