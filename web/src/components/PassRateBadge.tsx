import { cn } from "@/lib/cn";
import { formatPercent, passRate } from "@/lib/format";

/**
 * Pass-rate pill with a color that shifts green -> amber -> red as the rate
 * drops. Shows a tiny inline meter behind the number.
 */
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
  const rate = passRate(pass, fail, error);
  const pct = rate === null ? 0 : rate * 100;
  const tone =
    rate === null
      ? "text-muted ring-border"
      : pct >= 95
        ? "text-pass ring-pass/25"
        : pct >= 80
          ? "text-amber ring-amber/25"
          : "text-fail ring-fail/25";
  const bar =
    rate === null
      ? "var(--color-skip)"
      : pct >= 95
        ? "var(--color-pass)"
        : pct >= 80
          ? "var(--color-amber)"
          : "var(--color-fail)";

  return (
    <span
      className={cn(
        "relative inline-flex min-w-[3.75rem] items-center justify-center overflow-hidden rounded-md px-2 py-0.5 text-xs font-semibold tabular-nums ring-1 ring-inset",
        tone,
        className,
      )}
      title={`${pass} pass / ${fail} fail / ${error} error`}
    >
      <span
        className="absolute inset-y-0 left-0 opacity-[0.14]"
        style={{ width: `${pct}%`, backgroundColor: bar }}
        aria-hidden
      />
      <span className="relative">{formatPercent(rate)}</span>
    </span>
  );
}
