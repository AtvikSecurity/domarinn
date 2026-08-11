import type { AssertStatus, CaseStatus } from "@/api";
import { cn } from "@/lib/cn";
import {
  OUTLINE_LABEL_BASE,
  OUTLINE_LABEL_TONE,
  type OutlineTone,
} from "@/components/ui/chrome";

/**
 * Both a case's status (`CaseStatus`) and a single assertion's status
 * (`AssertStatus`, from the full `CaseResult` returned by the case-detail
 * endpoint) render through this badge. They agree on pass/fail/error but
 * spell the fourth state differently ("skip" vs "skipped") — cover both.
 */
type BadgeStatus = CaseStatus | AssertStatus;

const STATUS_TONE: Record<BadgeStatus, OutlineTone> = {
  pass: "pass",
  fail: "fail",
  error: "error",
  skip: "skip",
  skipped: "skip",
};

const STATUS_LABEL: Record<BadgeStatus, string> = {
  pass: "Pass",
  fail: "Fail",
  error: "Error",
  skip: "Skip",
  skipped: "Skip",
};

export function StatusBadge({
  status,
  className,
  size = "sm",
}: {
  status: BadgeStatus;
  className?: string;
  size?: "sm" | "xs";
}) {
  return (
    <span
      className={cn(
        OUTLINE_LABEL_BASE,
        size === "xs"
          ? "px-1 py-0.5 text-[10px]"
          : "px-[7px] py-[3px] text-[11px]",
        OUTLINE_LABEL_TONE[STATUS_TONE[status]],
        className,
      )}
    >
      <span
        className="size-1.5 rounded-full"
        style={{ backgroundColor: "currentColor" }}
        aria-hidden
      />
      {STATUS_LABEL[status]}
    </span>
  );
}

/**
 * A small square dot used inside dense assert-grid cells.
 *
 * `role="img"` is required, not decorative: `aria-label` is ignored on a
 * role-less generic element, so without it the grid's entire assert area is
 * announced as nothing at all and pass/fail is conveyed by colour alone.
 * `label` should carry the full description (assertion name and outcome), since
 * this is the only text a screen reader gets for the cell.
 */
export function AssertDot({
  passed,
  label,
}: {
  passed: boolean;
  label?: string;
}) {
  return (
    <span
      role="img"
      aria-label={label ?? (passed ? "passed" : "failed")}
      className={cn(
        "inline-block size-2.5 rounded-[3px] ring-1 ring-inset",
        passed ? "bg-pass/70 ring-pass/40" : "bg-fail/70 ring-fail/40",
      )}
    />
  );
}
