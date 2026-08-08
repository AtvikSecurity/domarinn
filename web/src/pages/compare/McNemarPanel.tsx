import type { CompareStats, WilsonView } from "@/api";
import { Chip, type ChipTone } from "@/components/ui/Chip";
import { CHROME_FRAME } from "@/components/ui/chrome";
import { Tooltip } from "@/components/ui/Tooltip";
import { cn } from "@/lib/cn";

/** 0..1 fraction → a one-decimal percent number without the sign (e.g. 0.833
 *  → "83.3"). */
function pct(fraction: number): string {
  return (fraction * 100).toFixed(1);
}

/** The Wilson label the brief pins: `83.3% (69.1–92.2)`. */
function wilsonLabel(w: WilsonView): string {
  return `${pct(w.rate)}% (${pct(w.lower)}–${pct(w.upper)})`;
}

const infoIcon = (
  <svg
    width="13"
    height="13"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="2"
    strokeLinecap="round"
    strokeLinejoin="round"
    aria-hidden
  >
    <circle cx="12" cy="12" r="10" />
    <path d="M12 16v-4M12 8h.01" />
  </svg>
);

/** One labelled Wilson-interval bar: the pass rate as a filled bar, with the
 *  `[lower, upper]` interval drawn as a translucent band and whisker ticks. */
function WilsonBar({ label, view }: { label: string; view: WilsonView }) {
  const rate = clampFraction(view.rate);
  const lower = clampFraction(view.lower);
  const upper = clampFraction(view.upper);
  return (
    <div className="min-w-0">
      <div className="flex items-baseline justify-between gap-2">
        <span className="text-[11px] font-medium uppercase tracking-wide text-muted">
          {label}
        </span>
        <span className="font-mono text-xs tabular-nums text-fg">
          {wilsonLabel(view)}
        </span>
      </div>
      <div className="relative mt-1 h-2 rounded-full bg-surface-2">
        {/* Confidence band [lower, upper]. */}
        <div
          className="absolute inset-y-0 rounded-full bg-accent/20"
          style={{ left: `${lower * 100}%`, width: `${(upper - lower) * 100}%` }}
        />
        {/* Filled pass-rate bar. */}
        <div
          className="absolute inset-y-0 left-0 rounded-full bg-accent/70"
          style={{ width: `${rate * 100}%` }}
        />
        {/* Point estimate marker. */}
        <div
          className="absolute inset-y-[-2px] w-0.5 rounded bg-accent"
          style={{ left: `${rate * 100}%` }}
        />
      </div>
    </div>
  );
}

function clampFraction(n: number): number {
  return Math.max(0, Math.min(1, n));
}

/**
 * The compare's statistical significance panel: McNemar regression/fix counts,
 * the χ² statistic, a significance badge, and Wilson pass-rate interval bars
 * for both runs. Placed beside the summary chips.
 */
export function McNemarPanel({ stats }: { stats: CompareStats }) {
  const { mcnemar } = stats;
  const { regressions, fixes, statistic, significant } = mcnemar;

  // When significant, the badge takes the tone of the dominant direction:
  // fail when regressions outweigh fixes, pass when fixes win.
  const worsened = regressions > fixes;
  const badgeTone: ChipTone = significant ? (worsened ? "fail" : "pass") : "neutral";
  const badgeLabel = significant ? "Statistically significant" : "Not significant";

  return (
    <div
      data-testid="mcnemar-panel"
      className={cn(CHROME_FRAME, "p-4")}
    >
      <div className="flex flex-wrap items-center gap-3">
        <div className="flex items-center gap-1.5">
          <h2 className="font-mono text-[10px] font-medium uppercase tracking-[0.12em] text-muted">
            Significance
          </h2>
          <Tooltip
            content="McNemar's test asks whether the pass↔fail flips between the two runs are asymmetric beyond chance (α = 0.05). A significant result means the fixes and regressions are unlikely to be noise."
          >
            <button
              type="button"
              aria-label="About the McNemar significance test"
              className="text-muted transition-colors hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring rounded"
            >
              {infoIcon}
            </button>
          </Tooltip>
        </div>
        <Chip tone={badgeTone}>{badgeLabel}</Chip>
      </div>

      <div className="mt-3 flex flex-wrap items-center gap-x-6 gap-y-2">
        <Stat label="Regressions" value={regressions} tone="text-fail" />
        <Stat label="Fixes" value={fixes} tone="text-pass" />
        <Stat
          label="χ² statistic"
          value={statistic.toFixed(2)}
          tone="text-fg"
        />
      </div>

      <div className="mt-4 grid grid-cols-1 gap-3 sm:grid-cols-2">
        <WilsonBar label="Base pass rate" view={stats.base_pass_rate} />
        <WilsonBar label="Head pass rate" view={stats.head_pass_rate} />
      </div>
    </div>
  );
}

function Stat({
  label,
  value,
  tone,
}: {
  label: string;
  value: number | string;
  tone: string;
}) {
  return (
    <div className="flex flex-col">
      <span className="text-[11px] font-medium uppercase tracking-wide text-muted">
        {label}
      </span>
      <span className={cn("font-mono text-sm font-semibold tabular-nums", tone)}>
        {value}
      </span>
    </div>
  );
}
