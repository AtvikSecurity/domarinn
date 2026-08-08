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
        "min-w-[3.75rem] justify-center px-[7px] py-[3px] text-[11px] font-semibold tabular-nums",
        OUTLINE_LABEL_TONE[tone],
        className,
      )}
      title={title}
    >
      <span>{formatPercent(rate)}</span>
    </span>
  );
}
