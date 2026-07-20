import type { AssertStatus, CaseStatus } from "@/api";
import { cn } from "@/lib/cn";

/**
 * Both a case's status (`CaseStatus`) and a single assertion's status
 * (`AssertStatus`, from the full `CaseResult` returned by the case-detail
 * endpoint) render through this badge. They agree on pass/fail/error but
 * spell the fourth state differently ("skip" vs "skipped") — cover both.
 */
type BadgeStatus = CaseStatus | AssertStatus;

const STATUS_STYLE: Record<BadgeStatus, string> = {
  pass: "bg-pass/12 text-pass ring-pass/25",
  fail: "bg-fail/12 text-fail ring-fail/25",
  error: "bg-error/12 text-error ring-error/25",
  skip: "bg-skip/12 text-skip ring-skip/25",
  skipped: "bg-skip/12 text-skip ring-skip/25",
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
        "inline-flex items-center gap-1 rounded-full font-medium ring-1 ring-inset",
        size === "xs" ? "px-1.5 py-0.5 text-[11px]" : "px-2 py-0.5 text-xs",
        STATUS_STYLE[status],
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

/** A small square dot used inside dense assert-grid cells. */
export function AssertDot({
  passed,
  title,
}: {
  passed: boolean;
  title?: string;
}) {
  return (
    <span
      title={title}
      className={cn(
        "inline-block size-2.5 rounded-[3px] ring-1 ring-inset",
        passed ? "bg-pass/70 ring-pass/40" : "bg-fail/70 ring-fail/40",
      )}
      aria-label={passed ? "passed" : "failed"}
    />
  );
}
