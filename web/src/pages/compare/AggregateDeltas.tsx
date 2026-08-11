import { CHROME_FRAME } from "@/components/ui/chrome";
import { StatBlock } from "@/components/ui/StatBlock";
import { formatCost, formatDuration, formatInt, formatTokens } from "@/lib/format";
import { cn } from "@/lib/cn";

/**
 * The per-run aggregates AggregateDeltas needs. Deliberately a plain shape (not
 * a wire type) so its data source can be swapped later: today the compare page
 * feeds it the two already-fetched `RunDetailResponse`s; Task 13 can feed it
 * `CompareResponse.totals` instead without touching this component.
 */
export interface AggregateStatsInput {
  prompt_tokens: number;
  completion_tokens: number;
  cost_usd: number | null;
  duration_ms: number;
  case_count: number;
}

// Unicode minus (U+2212) reads better than a hyphen next to a formatted value.
const MINUS = "−";

/** Render one signed delta as `+X`/`−X`/`X`(zero), formatting the magnitude
 *  with the supplied helper. */
function signedText(delta: number, formatAbs: (n: number) => string): string {
  const sign = delta > 0 ? "+" : delta < 0 ? MINUS : "";
  return `${sign}${formatAbs(Math.abs(delta))}`;
}

/** Tone for a delta. `lowerIsBetter` metrics (tokens/cost/duration) are green
 *  when they shrink; `neutral` metrics (case count) stay muted regardless. */
function toneFor(delta: number, lowerIsBetter: boolean, neutral: boolean): string {
  if (delta === 0 || neutral) return "text-muted";
  const better = lowerIsBetter ? delta < 0 : delta > 0;
  return better ? "text-pass" : "text-fail";
}

interface Metric {
  key: string;
  label: string;
  delta: number;
  formatAbs: (n: number) => string;
  lowerIsBetter: boolean;
  neutral?: boolean;
}

/**
 * A compact head-vs-base stat row: total tokens (prompt+completion), cost,
 * duration and case count. Signed, tinted green when "better" (fewer
 * tokens/cost/duration), red when worse, muted when unchanged.
 */
export function AggregateDeltas({
  base,
  head,
}: {
  base: AggregateStatsInput;
  head: AggregateStatsInput;
}) {
  const baseTokens = base.prompt_tokens + base.completion_tokens;
  const headTokens = head.prompt_tokens + head.completion_tokens;

  const metrics: Metric[] = [
    {
      key: "tokens",
      label: "Tokens",
      delta: headTokens - baseTokens,
      formatAbs: formatTokens,
      lowerIsBetter: true,
    },
    {
      key: "cost",
      label: "Cost",
      delta: (head.cost_usd ?? 0) - (base.cost_usd ?? 0),
      formatAbs: formatCost,
      lowerIsBetter: true,
    },
    {
      key: "duration",
      label: "Duration",
      delta: head.duration_ms - base.duration_ms,
      formatAbs: formatDuration,
      lowerIsBetter: true,
    },
    {
      key: "cases",
      label: "Cases",
      delta: head.case_count - base.case_count,
      formatAbs: formatInt,
      lowerIsBetter: false,
      neutral: true,
    },
  ];

  return (
    <div
      data-testid="aggregate-deltas"
      className={cn(CHROME_FRAME, "flex flex-wrap gap-x-6 gap-y-2 px-4 py-3")}
    >
      {metrics.map((m) => (
        <StatBlock key={m.key} label={m.label} variant="bare">
          <span
            className={cn(
              "font-mono font-semibold",
              toneFor(m.delta, m.lowerIsBetter, m.neutral ?? false),
            )}
          >
            {signedText(m.delta, m.formatAbs)}
          </span>
        </StatBlock>
      ))}
    </div>
  );
}
