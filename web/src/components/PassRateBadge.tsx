import { cn } from "@/lib/cn";
import { formatPercent, passRate } from "@/lib/format";
import {
  OUTLINE_LABEL_BASE,
  OUTLINE_LABEL_TONE,
  type OutlineTone,
} from "@/components/ui/chrome";

/** Pass-rate outline label with a tone that shifts as the rate drops. */
export function PassRateBadge({
  pass,
  fail,
  error,
  className,
}: {
  pass: number;
  fail: number;
  error: number;
  className?: string;
}) {
  return (
    <RateBadge
      rate={passRate(pass, fail, error)}
      title={`${pass} pass / ${fail} fail / ${error} error`}
      className={className}
    />
  );
}

/**
 * The same label, for a payload that carries a rate rather than counts.
 *
 * The run-set browser is the one such caller: its `latest_pass_rate` is the
 * newest run's, while the counts beside it are the set's lifetime totals.
 */
/** The meter fill, in the same hue the outline is already using. */
const METER: Record<OutlineTone, string> = {
  neutral: "bg-skip",
  info: "bg-info",
  accent: "bg-accent",
  pass: "bg-pass",
  fail: "bg-fail",
  error: "bg-error",
  amber: "bg-amber",
  skip: "bg-skip",
};

export function RateBadge({
  rate,
  title,
  className,
}: {
  rate: number | null;
  title?: string;
  className?: string;
}) {
  const pct = rate === null ? 0 : rate * 100;
  const tone: OutlineTone =
    rate === null ? "neutral" : pct >= 95 ? "pass" : pct >= 80 ? "amber" : "fail";

  return (
    <span
      className={cn(
        OUTLINE_LABEL_BASE,
        // `relative` + `overflow-hidden` for the meter: it is absolutely
        // positioned against this box and clipped to the pill's corner radius.
        "relative min-w-[3.75rem] justify-center overflow-hidden px-[7px] py-[3px] text-[11px] font-semibold tabular-nums",
        OUTLINE_LABEL_TONE[tone],
        className,
      )}
      title={title}
    >
      {/* The rate, read as a bar as well as a number — the width is the value.
          Kept faint enough that the percentage stays the thing you read first,
          and the outline recipe's own fill is transparent, so this is the only
          thing painting inside the border. */}
      <span
        className={cn("absolute inset-y-0 left-0 opacity-[0.14]", METER[tone])}
        style={{ width: `${pct}%` }}
        aria-hidden
      />
      {/* `relative` so the label paints above the meter: both sit in this
          stacking context, and a positioned box otherwise covers static
          in-flow content whatever the DOM order. */}
      <span className="relative">{formatPercent(rate)}</span>
    </span>
  );
}
