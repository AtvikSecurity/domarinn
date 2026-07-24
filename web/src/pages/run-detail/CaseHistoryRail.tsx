import { useState } from "react";
import { Link } from "react-router";
import type { CaseHistoryPoint, CaseHistoryResponse, CaseStatus } from "@/api";
import { useCaseHistory } from "@/api/queries";
import { Sparkline } from "@/components/Sparkline";
import { CollapsibleSection } from "@/components/ui/CollapsibleSection";
import { SegmentedControl } from "@/components/ui/SegmentedControl";
import { Spinner } from "@/components/ui/Spinner";
import { Tooltip } from "@/components/ui/Tooltip";
import {
  formatCost,
  formatDate,
  formatLatency,
  formatRelative,
  formatTokens,
  shortRunId,
} from "@/lib/format";
import {
  HISTORY_METRICS,
  historySeries,
  historySummary,
  metricSpec,
  type HistoryMetric,
} from "@/lib/history";
import { cn } from "@/lib/cn";
import { CaseHistoryTable } from "./CaseHistoryTable";

/** Window sizes offered to the user. The server caps the window at 100. */
const WINDOWS = [
  { value: "20", label: "20" },
  { value: "50", label: "50" },
  { value: "100", label: "100" },
] as const;

/**
 * "Has this case always been like this?" — answered without scrolling.
 *
 * The rail lives in the drawer's fixed verdict strip rather than in the
 * scrolling body, where it used to sit ninth of ten sections, below two
 * arbitrarily tall ones. Whether a failure is new or long-standing is the first
 * thing you want when triaging, so it is now visible at any scroll depth.
 *
 * The squares encode four channels already — fill = status, ring = current run,
 * underline = baseline, diamond = output changed. That is the ceiling for a
 * 14px mark; the remaining per-point fields (tokens, cost, latency, commit,
 * config digest) belong in the expandable table, not here.
 */
export function CaseHistoryRail({
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
  const [limit, setLimit] = useState<(typeof WINDOWS)[number]["value"]>("20");
  const [metric, setMetric] = useState<HistoryMetric>("score");
  const [showTable, setShowTable] = useState(false);

  // Fetched whenever the drawer is open. The rail is above the fold now, so
  // deferring the request until a section is expanded would defeat the point —
  // and it costs nothing extra, since the section defaulted to expanded anyway.
  // `limit` is part of the query key, so widening is a cached refetch.
  const history = useCaseHistory(project, suite, caseKey, {
    enabled: true,
    limit: Number(limit),
  });

  const points = history.data?.points ?? [];
  const { runs, outputChanges } = historySummary(points);
  // Deliberately "changes", not "output changes": inside a case's History the
  // subject is unambiguous, and the word "output" here would collide with the
  // drawer's own Output section for anything matching headings by name.
  const summary =
    runs > 0
      ? `· ${runs} ${runs === 1 ? "run" : "runs"} · ${outputChanges} ${outputChanges === 1 ? "change" : "changes"}`
      : undefined;

  return (
    <CollapsibleSection
      title="History"
      meta={summary}
      actions={
        points.length > 0 ? (
          <div className="flex items-center gap-1.5">
            <button
              type="button"
              aria-expanded={showTable}
              onClick={() => setShowTable((v) => !v)}
              className="rounded px-1.5 py-0.5 text-[11px] font-medium text-muted hover:bg-surface-2 hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              {showTable ? "Hide runs" : "All runs"}
            </button>
            <SegmentedControl
              ariaLabel="History window"
              size="xs"
              options={WINDOWS}
              value={limit}
              onChange={setLimit}
            />
          </div>
        ) : undefined
      }
    >
      {history.isPending ? (
        // Fixed height: the strip is a fixed band, and a band that resizes when
        // data lands is worse than no band at all.
        <div className="flex h-12 items-center gap-2 text-xs text-muted">
          <Spinner /> Loading history…
        </div>
      ) : history.isError ? (
        <p className="flex h-12 items-center text-sm text-muted">
          Case history is unavailable.
        </p>
      ) : history.data && points.length > 0 ? (
        <HistoryTimeline
          data={history.data}
          runId={runId}
          caseKey={caseKey}
          metric={metric}
          onMetricChange={setMetric}
          showTable={showTable}
        />
      ) : (
        <p className="flex h-12 items-center text-sm text-muted">
          No history for this case yet.
        </p>
      )}
    </CollapsibleSection>
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

/** Hit-target pitch: a 24px target (WCAG 2.2 minimum) plus a 4px gap. */
const PITCH = 28;

/** Renders a metric value the way its own column would. */
function formatMetric(metric: HistoryMetric, v: number): string {
  switch (metric) {
    case "score":
      return v.toFixed(2);
    case "latency":
      return formatLatency(v);
    case "cost":
      return formatCost(v);
    case "tokens":
      return formatTokens(v);
  }
}

function HistoryTimeline({
  data,
  runId,
  caseKey,
  metric,
  onMetricChange,
  showTable,
}: {
  data: CaseHistoryResponse;
  runId: string;
  caseKey: string;
  metric: HistoryMetric;
  onMetricChange: (m: HistoryMetric) => void;
  showTable: boolean;
}) {
  // Payload is newest-first; reverse to oldest→newest for the timeline.
  const chronological = [...data.points].reverse();
  const spec = metricSpec(metric);
  // Nulls are kept as gaps rather than dropped, so the sparkline's x-positions
  // stay tied to the squares above it.
  const series = historySeries(chronological, metric);
  const present = series.filter((s): s is number => s != null);
  const baselineRunId = data.baseline_run_id;

  const first = present[0];
  const last = present[present.length - 1];

  return (
    <div className="space-y-2">
      <div className="flex items-start gap-1 overflow-x-auto overscroll-x-contain px-0.5 py-0.5">
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

      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        {present.length >= 2 && first !== undefined && last !== undefined ? (
          <>
            <Sparkline
              values={series}
              {...(spec.min !== undefined ? { min: spec.min } : {})}
              {...(spec.max !== undefined ? { max: spec.max } : {})}
              higherIsBetter={spec.higherIsBetter}
              width={Math.max(96, chronological.length * PITCH)}
              height={22}
              title={`${spec.label} trend`}
            />
            <span className="text-[11px] tabular-nums text-muted">
              {formatMetric(metric, first)} → {formatMetric(metric, last)}
            </span>
          </>
        ) : (
          <span className="text-[11px] text-muted">
            No {spec.label.toLowerCase()} recorded for this window.
          </span>
        )}
        <SegmentedControl
          ariaLabel="History metric"
          size="xs"
          className="ml-auto"
          options={HISTORY_METRICS.map((m) => ({
            value: m.value,
            label: m.label,
          }))}
          value={metric}
          onChange={onMetricChange}
        />
      </div>

      {showTable ? (
        <CaseHistoryTable
          points={chronological}
          runId={runId}
          baselineRunId={baselineRunId}
          caseKey={caseKey}
        />
      ) : null}
    </div>
  );
}

/**
 * One run's square. The 14px tinted mark sits inside a 24px link so the target
 * clears WCAG 2.2's minimum — and so its focus ring fits inside the scroll
 * container instead of being clipped by it.
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
    <div className="flex shrink-0 flex-col items-center gap-0.5">
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
            "grid size-6 place-items-center rounded-[5px] transition",
            isCurrent && "ring-2 ring-accent",
          )}
        >
          <span
            aria-hidden
            className={cn(
              "size-3.5 rounded-[3px] border transition hover:brightness-110",
              STATUS_SQUARE[point.status],
            )}
          />
        </Link>
      </Tooltip>

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
