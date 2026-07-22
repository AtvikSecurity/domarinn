import { useState } from "react";
import { Link } from "react-router";
import type { CaseHistoryPoint, CaseHistoryResponse, CaseStatus } from "@/api";
import { useCaseHistory } from "@/api/queries";
import { Sparkline } from "@/components/Sparkline";
import { Spinner } from "@/components/ui/Spinner";
import { Tooltip } from "@/components/ui/Tooltip";
import { formatDate, formatRelative, shortRunId } from "@/lib/format";
import { cn } from "@/lib/cn";

/**
 * Collapsible "History" section in the run-detail case drawer: how THIS case
 * (its deterministic `case_key`) evolved across the suite's recent runs —
 * status squares, an output-changed timeline, and a score sparkline, each square
 * deep-linking to the same case in that run.
 *
 * The history window is fetched only once the section is expanded (the `enabled`
 * gate on {@link useCaseHistory}). The payload's `points` are newest-first; the
 * timeline reverses them for a natural oldest→newest left-to-right reading.
 */
export function CaseHistorySection({
  project,
  suite,
  runId,
  caseKey,
}: {
  project: string;
  suite: string;
  runId: string;
  caseKey: string;
}) {
  // Expanded (and therefore fetching) by default; the `enabled` gate still
  // stops the window from refetching while the section is tucked away.
  const [expanded, setExpanded] = useState(true);
  const history = useCaseHistory(project, suite, caseKey, {
    enabled: expanded,
    limit: 20,
  });

  return (
    <section>
      <button
        type="button"
        aria-expanded={expanded}
        onClick={() => setExpanded((v) => !v)}
        className="flex w-full items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted hover:text-fg"
      >
        <svg
          width="14"
          height="14"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          className={cn("shrink-0 transition-transform", expanded && "rotate-90")}
          aria-hidden
        >
          <path d="M9 6l6 6-6 6" />
        </svg>
        <span>History</span>
      </button>

      {expanded ? (
        <div className="mt-2">
          {history.isPending ? (
            <div className="flex items-center gap-2 p-3 text-xs text-muted">
              <Spinner /> Loading history…
            </div>
          ) : history.isError ? (
            // Muted, retry-less — consistent with the baseline-diff section. A
            // 404 (shouldn't happen for the case's own history) lands here too.
            <p className="text-sm text-muted">Case history is unavailable.</p>
          ) : history.data && history.data.points.length > 0 ? (
            <HistoryTimeline
              data={history.data}
              runId={runId}
              caseKey={caseKey}
            />
          ) : (
            <p className="text-sm text-muted">No history for this case yet.</p>
          )}
        </div>
      ) : null}
    </section>
  );
}

// Filled status squares, one per run; the border keeps them legible on the
// tinted fill. Colours are CSS tokens, so light/dark tracks automatically.
const STATUS_SQUARE: Record<CaseStatus, string> = {
  pass: "bg-pass/70 border-pass/50",
  fail: "bg-fail/70 border-fail/50",
  error: "bg-error/70 border-error/50",
  skip: "bg-skip/70 border-skip/50",
};

const STATUS_LABEL: Record<CaseStatus, string> = {
  pass: "pass",
  fail: "fail",
  error: "error",
  skip: "skip",
};

function HistoryTimeline({
  data,
  runId,
  caseKey,
}: {
  data: CaseHistoryResponse;
  runId: string;
  caseKey: string;
}) {
  // Payload is newest-first; reverse to oldest→newest for the timeline.
  const chronological = [...data.points].reverse();
  // Sparkline plots a contiguous `number[]` (no gap support), so points with a
  // null score are dropped from the series rather than gapped; the remaining
  // scores stay in chronological order.
  const scores = chronological
    .map((p) => p.score)
    .filter((s): s is number => s != null);
  const changeCount = data.points.filter(
    (p) => p.output_changed === true,
  ).length;
  const baselineRunId = data.baseline_run_id;
  const runCount = data.points.length;

  const first = scores[0];
  const last = scores[scores.length - 1];

  return (
    <div className="space-y-3">
      <div className="flex items-start gap-1 overflow-x-auto pb-1">
        {chronological.map((p) => (
          <HistorySquare
            key={p.run_id}
            point={p}
            isCurrent={p.run_id === runId}
            isBaseline={baselineRunId != null && p.run_id === baselineRunId}
            caseKey={caseKey}
          />
        ))}
      </div>

      {scores.length >= 2 && first !== undefined && last !== undefined ? (
        <div className="flex items-center gap-2">
          <Sparkline
            values={scores}
            min={0}
            max={1}
            width={Math.max(96, scores.length * 10)}
            height={26}
            title="Score trend"
          />
          <span className="text-[11px] tabular-nums text-muted">
            {first.toFixed(2)} → {last.toFixed(2)}
          </span>
        </div>
      ) : null}

      <div className="text-[11px] text-muted">
        {runCount} {runCount === 1 ? "run" : "runs"} · {changeCount} output{" "}
        {changeCount === 1 ? "change" : "changes"}
      </div>
    </div>
  );
}

/**
 * One run's square: status-tinted, ring-highlighted for the current run and
 * marked with an accent underline for the suite's baseline run. A short amber
 * diamond sits under the square when this run's output changed vs the previous
 * one. The whole square deep-links to the same case in that run (the drawer
 * re-mounts on the target run's page).
 */
function HistorySquare({
  point,
  isCurrent,
  isBaseline,
  caseKey,
}: {
  point: CaseHistoryPoint;
  isCurrent: boolean;
  isBaseline: boolean;
  caseKey: string;
}) {
  const changed = point.output_changed === true;
  const to = `/runs/${encodeURIComponent(point.run_id)}?case=${encodeURIComponent(
    caseKey,
  )}`;
  const scoreText = point.score != null ? point.score.toFixed(2) : "—";

  const tooltip = (
    <div className="space-y-0.5">
      <div className="font-mono">
        {shortRunId(point.run_id)}
        {isCurrent ? " · current" : ""}
        {isBaseline ? " · baseline" : ""}
      </div>
      <div>
        {formatDate(point.created_at)} · {formatRelative(point.created_at)}
      </div>
      <div>
        {STATUS_LABEL[point.status]} · score {scoreText}
      </div>
      {changed ? <div>Output changed vs previous run</div> : null}
    </div>
  );

  return (
    <div className="flex shrink-0 flex-col items-center gap-1">
      <span
        className={cn(
          "inline-flex rounded-[5px] p-px",
          isCurrent ? "ring-2 ring-accent" : "ring-2 ring-transparent",
        )}
      >
        <Tooltip content={tooltip}>
          <Link
            to={to}
            data-history-square=""
            data-run-id={point.run_id}
            data-current={isCurrent ? "true" : undefined}
            aria-label={`Run ${shortRunId(point.run_id)}: ${STATUS_LABEL[point.status]}${
              isCurrent ? " (current)" : ""
            }`}
            className={cn(
              "block size-3.5 rounded-[3px] border transition hover:brightness-110",
              STATUS_SQUARE[point.status],
            )}
          />
        </Tooltip>
      </span>

      {/* Baseline marker: a short accent underline directly beneath the square
          (a transparent placeholder keeps every column the same height). */}
      <span
        aria-hidden
        data-baseline={isBaseline ? "true" : undefined}
        className={cn(
          "h-0.5 w-3.5 rounded-full",
          isBaseline ? "bg-accent" : "bg-transparent",
        )}
      />

      {/* Output-changed marker: a small amber diamond under the timeline. */}
      <span className="flex h-2 items-center justify-center">
        {changed ? (
          <Tooltip content="Output changed vs previous run">
            <span
              data-output-changed=""
              aria-label="Output changed vs previous run"
              className="size-1.5 rotate-45 rounded-[1px] bg-amber"
            />
          </Tooltip>
        ) : null}
      </span>
    </div>
  );
}
