import type { ReactNode } from "react";
import { cn } from "@/lib/cn";
import { CHROME_FRAME } from "./chrome";

/**
 * One labelled number.
 *
 * Three near-identical private versions of this existed (the run header's boxed
 * tile, the compare page's bare stat, the McNemar panel's), which is why the
 * case drawer never gained real stats and instead concatenated score, tokens,
 * cost and latency into a single line of 12px muted text.
 *
 * `sub` is the part that earns its keep: it is the slot for the detail the
 * summed headline number hides — the prompt/completion token split, cache
 * hit/miss counts, `cache_read_tokens`, or an outlier comparison against the
 * case's own history. One mechanism, every call site.
 */
export function StatBlock({
  label,
  children,
  sub,
  tone,
  variant = "boxed",
  className,
  title,
}: {
  label: string;
  children: ReactNode;
  /** Secondary line under the value — the decomposition of the headline. */
  sub?: ReactNode;
  /** Tailwind text-colour class for the value, e.g. `text-fail`. */
  tone?: string;
  variant?: "boxed" | "bare";
  className?: string;
  title?: string;
}) {
  return (
    <div
      className={cn(
        variant === "boxed" && cn(CHROME_FRAME, "px-3 py-2"),
        className,
      )}
      title={title}
    >
      <div className="font-mono text-[10px] font-medium uppercase tracking-[0.12em] text-muted">
        {label}
      </div>
      <div className={cn("mt-0.5 text-sm font-medium tabular-nums", tone)}>
        {children}
      </div>
      {sub ? (
        <div className="mt-0.5 text-[11px] tabular-nums text-muted">{sub}</div>
      ) : null}
    </div>
  );
}
