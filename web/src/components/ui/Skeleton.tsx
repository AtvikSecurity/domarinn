import type { ReactNode } from "react";
import { cn } from "@/lib/cn";

/**
 * Loading placeholders, for the case where a spinner is the wrong answer.
 *
 * `CenteredSpinner` is right when a whole surface is blank and about to be
 * replaced. It is wrong inside a drawer that has already slid in with a header:
 * the shell opens immediately so the click does not read as dead, and a spinner
 * floating in the body says "nothing here yet" where a skeleton says "content
 * of roughly this shape, shortly". The drawer is the only current caller, so
 * the recipe lives in one place rather than being inlined there.
 *
 * Three pieces, split by who announces what:
 *   - {@link Skeleton} — one bar. Decorative, so `aria-hidden`: a screen reader
 *     should hear one status message, not a stack of anonymous boxes.
 *   - {@link SkeletonText} — a paragraph's worth of staggered bars.
 *   - {@link SkeletonFrame} — the announcing wrapper. The ONLY place
 *     `role="status"` lives; compose it once per surface, never per row.
 */

/**
 * The bar recipe. Size is deliberately not baked in — compose it at the call
 * site as `<Skeleton className="h-4 w-32" />`.
 *
 * `.skeleton-bar` carries nothing visual; it exists so the
 * `prefers-reduced-motion` rule in `index.css` can stop the pulse without every
 * call site reaching for a media-query hook.
 */
export const SKELETON_BASE = "skeleton-bar animate-pulse rounded bg-surface-2";

/** One placeholder bar. Decorative — announced by its frame. */
export function Skeleton({ className }: { className?: string }) {
  return <div aria-hidden className={cn(SKELETON_BASE, className)} />;
}

/**
 * Widths for successive lines. Staggering reads as prose rather than as a
 * block, and the last line stopping short mimics a real paragraph's ragged
 * edge.
 */
const TEXT_LINE_WIDTHS = ["w-full", "w-[92%]", "w-[85%]", "w-[70%]", "w-[60%]"] as const;

/** A paragraph-shaped run of bars. */
export function SkeletonText({
  lines = 3,
  lineClassName = "h-4",
  className,
}: {
  /** Number of lines. Widths cycle through the staggered set. */
  lines?: number;
  /** Height utility for each line. */
  lineClassName?: string;
  className?: string;
}) {
  return (
    <div className={cn("space-y-2", className)}>
      {Array.from({ length: lines }, (_, i) => (
        <Skeleton
          key={i}
          className={cn(lineClassName, TEXT_LINE_WIDTHS[i % TEXT_LINE_WIDTHS.length])}
        />
      ))}
    </div>
  );
}

/**
 * The announcing wrapper for a whole loading surface.
 *
 * Assistive tech should hear "Loading case…" once, not one message per bar, so
 * this owns `role="status"` / `aria-busy` and every bar inside stays
 * `aria-hidden`.
 */
export function SkeletonFrame({
  label,
  children,
  className,
  "data-testid": testId,
}: {
  /** What is loading, e.g. "case". Announced once, politely. */
  label: string;
  children: ReactNode;
  className?: string;
  "data-testid"?: string;
}) {
  return (
    <div role="status" aria-busy className={className} data-testid={testId}>
      <span className="sr-only">Loading {label}…</span>
      {children}
    </div>
  );
}
