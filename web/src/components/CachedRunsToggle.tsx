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
   * How many runs the hidden view is suppressing. Undefined where a surface
   * cannot know (a cursor-paginated list only learns it from the first page).
   */
  hiddenCount?: number;
  onChange: (next: CachedFilter) => void;
}) {
  if (resolved === "exclude") {
    const n = hiddenCount ?? 0;
    // Nothing suppressed, nothing to say. A "0 hidden" line reads as a bug.
    if (n <= 0) return null;
    return (
      <p className="text-xs text-muted">
        {n} fully cached run{n === 1 ? "" : "s"} hidden{" · "}
        <button type="button" className={linkCls} onClick={() => onChange("all")}>
          Show
        </button>
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
