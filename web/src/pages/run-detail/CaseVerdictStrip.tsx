import type { CaseResult } from "@/api";
import { StatusBadge } from "@/components/StatusBadge";
import { Chip } from "@/components/ui/Chip";
import { StatBlock } from "@/components/ui/StatBlock";
import { formatCost, formatLatency, formatTokens } from "@/lib/format";
import { StopReasonChip } from "./CaseDrawerSections";
import { CaseHistoryRail } from "./CaseHistoryRail";

const STATUS_TONE: Record<string, string> = {
  pass: "text-pass",
  fail: "text-fail",
  error: "text-error",
  skip: "text-skip",
};

/**
 * The drawer's fixed verdict band: what happened, how much it cost, and whether
 * it has always been this way.
 *
 * This replaces a single line of 12px muted text that read
 * `score 0.42 · 500 tok · $0.0012 · 1.24s`. The score is what the whole case
 * reduces to, so it is the largest thing here after the case name; the three
 * numbers become real labelled stats whose sub-lines carry the decomposition the
 * summed headline hides (a prompt-heavy case is a cost problem, an output-heavy
 * one is a truncation problem — indistinguishable when the tokens are summed).
 *
 * It sits outside the scrolling body on purpose: these are the facts you want
 * while reading the output, not facts you scroll past to reach it.
 */
export function CaseVerdictStrip({
  detail,
  project,
  suite,
  runId,
  caseKey,
}: {
  detail: CaseResult;
  project: string;
  suite: string;
  runId: string;
  caseKey: string;
}) {
  const input = detail.usage?.input_tokens ?? 0;
  const output = detail.usage?.output_tokens ?? 0;
  const cacheRead = detail.usage?.cache_read_tokens;

  return (
    <div className="shrink-0 space-y-3 border-b border-border px-5 py-3">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <StatusBadge status={detail.status} />
        <span
          className={`text-2xl font-semibold tabular-nums ${STATUS_TONE[detail.status] ?? ""}`}
        >
          {detail.score.toFixed(2)}
        </span>
        <div className="ml-auto flex flex-wrap items-center justify-end gap-1.5">
          {(detail.tags ?? []).map((t) => (
            <Chip key={t}>{t}</Chip>
          ))}
          {detail.cached ? <Chip>cached</Chip> : null}
          {detail.attempts > 1 ? (
            <Chip tone="amber">{detail.attempts} attempts</Chip>
          ) : null}
          {detail.stop_reason ? (
            <StopReasonChip reason={detail.stop_reason} />
          ) : null}
        </div>
      </div>

      <div className="grid grid-cols-3 gap-3">
        <StatBlock
          label="Tokens"
          variant="bare"
          sub={
            detail.usage ? (
              <>
                {formatTokens(input)} in · {formatTokens(output)} out
                {cacheRead != null && cacheRead > 0
                  ? ` · ${formatTokens(cacheRead)} cached`
                  : ""}
              </>
            ) : undefined
          }
        >
          {formatTokens(input + output)}
        </StatBlock>
        <StatBlock label="Cost" variant="bare">
          {formatCost(detail.cost_usd)}
        </StatBlock>
        <StatBlock label="Latency" variant="bare">
          {formatLatency(detail.latency_ms)}
        </StatBlock>
      </div>

      <CaseHistoryRail
        project={project}
        suite={suite}
        runId={runId}
        caseKey={caseKey}
      />
    </div>
  );
}
