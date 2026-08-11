import type { AssertFlip } from "@/api";
import { StatusBadge } from "@/components/StatusBadge";
import { Chip } from "@/components/ui/Chip";
import { CHROME_FRAME } from "@/components/ui/chrome";
import { cn } from "@/lib/cn";

/**
 * The per-assertion flips for one case, taken verbatim from the compare row's
 * server-computed `assert_flips` (see `AssertFlip`). The server is the single
 * source of truth for the base↔head assert pairing, so this list and the row's
 * "N flips" chip always agree. A flip carries only pass/fail booleans (the lean
 * assert records have no per-assert status), so each side renders as a pass or
 * fail badge. Only changed assertions are present; an empty list ⇒ renders
 * nothing (no empty section).
 */
export function AssertTransitions({ flips }: { flips: AssertFlip[] }) {
  if (flips.length === 0) return null;

  return (
    <div
      data-testid="assert-transitions"
      className={cn(CHROME_FRAME, "px-3 py-2")}
    >
      <div className="mb-1.5 text-[10px] font-medium uppercase tracking-wide text-muted">
        Assertion changes
      </div>
      <ul className="space-y-1.5">
        {flips.map((flip, i) => (
          <li key={`${flip.kind}#${i}`} className="flex flex-wrap items-center gap-2">
            <Chip size="xs" className="normal-case">
              {flip.kind}
            </Chip>
            <StatusBadge status={flip.base_passed ? "pass" : "fail"} size="xs" />
            <span aria-hidden className="text-muted">
              →
            </span>
            <StatusBadge status={flip.head_passed ? "pass" : "fail"} size="xs" />
            <span className="font-mono text-[11px] tabular-nums text-muted">
              {flip.base_score.toFixed(2)} → {flip.head_score.toFixed(2)}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}
