import type { CachedFilter } from "@/api";

/**
 * The per-view override for cached runs: one line above a run list saying what
 * is being suppressed, and a control that flips it.
 *
 * It exists because a filter you cannot see is a filter you will blame on the
 * data. A user whose stored preference hides cached runs, opening a page that
 * looks short, has no way to tell a quiet week from a working filter — so the
 * suppression always occupies a line and names its own count rather than
 * showing as absence.
 *
 * It writes to the URL, never to the stored preference: this is "just for this
 * view". Changing the standing default is the filter bar's job, and keeping
 * the two separate means a one-off reveal cannot quietly retrain the rest of
 * the app.
 */
const linkCls =
  "rounded font-medium text-accent hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring";

export function CachedRunsToggle({
  resolved,
  hiddenCount,
  onChange,
}: {
  /** The filter actually in force here, URL and preference already resolved. */
  resolved: CachedFilter;
  /**
   * How many runs the hidden view is suppressing.
   *
   * `"unknown"` is for surfaces that genuinely cannot count — search ranks by
   * bm25 behind a LIMIT, so "suppressed overall" is not "suppressed from this
   * page", and no honest number exists. Those still announce the suppression,
   * just without inventing a figure. Undefined means "nothing to report".
   */
  hiddenCount?: number | "unknown";
  onChange: (next: CachedFilter) => void;
}) {
  if (resolved === "exclude") {
    // Nothing suppressed, nothing to say. A "0 hidden" line reads as a bug.
    if (hiddenCount === undefined || hiddenCount === 0) return null;
    const reveal = (
      <button type="button" className={linkCls} onClick={() => onChange("all")}>
        Show
      </button>
    );
    if (hiddenCount === "unknown") {
      return (
        <p className="text-xs text-muted">
          Hits from fully cached runs are hidden{" · "}
          {reveal}
        </p>
      );
    }
    return (
      <p className="text-xs text-muted">
        {hiddenCount} fully cached run{hiddenCount === 1 ? "" : "s"} hidden
        {" · "}
        {reveal}
      </p>
    );
  }

  if (resolved === "only") {
    return (
      <p className="text-xs text-muted">
        Showing only fully cached runs{" · "}
        <button type="button" className={linkCls} onClick={() => onChange("all")}>
          Show all
        </button>
      </p>
    );
  }

  // Revealed. Say so and keep the way back one click away, so a user who
  // clicked Show is not left hunting the filter bar to undo it.
  return (
    <p className="text-xs text-muted">
      Showing cached runs{" · "}
      <button
        type="button"
        className={linkCls}
        onClick={() => onChange("exclude")}
      >
        Hide
      </button>
    </p>
  );
}
