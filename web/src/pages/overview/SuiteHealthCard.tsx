import { Link } from "react-router";
import type { RunListItem } from "@/api";
import {
  canonicalDelta,
  canonicalRun,
  canonicalSeries,
  isStale,
  suiteSeverity,
  type Severity,
} from "@/lib/signals";
import { formatRelative, shortRunId } from "@/lib/format";
import { isFullyCached } from "@/lib/cached";
import { runPath, runsFilterHref } from "@/lib/routes";
import { Sparkline } from "@/components/Sparkline";
import { PassRateBadge } from "@/components/PassRateBadge";
import { Chip } from "@/components/ui/Chip";
import { CHROME_FRAME } from "@/components/ui/chrome";
import { cn } from "@/lib/cn";

/** Border tint by severity, so the card reads before you focus on it. */
const EDGE: Record<Severity, string | undefined> = {
  failing: "border-fail/50",
  stale: "border-amber/50",
  drifting: "border-amber/30",
  unknown: "border-skip/40",
  healthy: undefined,
};

function DeltaLabel({ points }: { points: number | null }) {
  // `null` is a suite's first canonical run: there is nothing to compare
  // against, which is different from "no change".
  if (points === null) return <span className="text-xs text-muted">first run</span>;
  if (Math.abs(points) < 0.05) return <span className="text-xs text-muted">=</span>;
  const down = points < 0;
  return (
    <span className={`text-xs ${down ? "text-fail" : "text-pass"}`}>
      {down ? "▼" : "▲"} {Math.abs(points).toFixed(1)} pts
    </span>
  );
}

/**
 * One suite's current state.
 *
 * The headline is the newest CI run; developer runs are never hidden but are
 * summarized in the footer, which links into the stream. That split is the
 * whole design: a shared board needs one authoritative number, and someone
 * else's scratch iteration is not it.
 */
export function SuiteHealthCard({
  project,
  suite,
  to,
  runs,
  now,
}: {
  project: string;
  suite: string;
  /** This suite's set page, or null when the runs declared no project/suite. */
  to: string | null;
  runs: RunListItem[];
  now: number;
}) {
  const canonical = canonicalRun(runs);
  const severity = suiteSeverity(runs, now);
  const series = canonicalSeries(runs);
  const stale = isStale(runs, now);
  const localCount = runs.filter((r) => r.ci_provider == null).length;
  const actors = new Set(runs.map((r) => r.actor).filter(Boolean)).size;
  const runsHref = runsFilterHref(project, suite);

  // A card IS a set — `project`/`suite` is exactly what identifies one — so the
  // whole card navigates to it. Done as a stretched link rather than an onClick
  // on the div: this stays one real anchor, so ⌘-click, middle-click, "open in
  // new tab" and the browser's URL preview all keep working, and the card's
  // accessible name is the suite it names.
  //
  // `isolate` because the raises below are only meant to beat this card's own
  // overlay, not to paint over a neighbour in the grid.
  return (
    <div
      data-testid="suite-health-card"
      className={cn("relative isolate p-4", CHROME_FRAME, EDGE[severity])}
    >
      <div className="flex items-baseline justify-between gap-2">
        <h2 className="text-sm font-semibold">
          {to ? (
            // The focus ring goes on the ::after, not the text: the overlay is
            // the real hit target, so ringing the title alone would point at
            // the wrong thing. Heading colour is kept — accent here would read
            // as one more inline link rather than as the card's identity.
            //
            // `after:z-[1]` puts the stretched hit target above the card body.
            // The explicit links below are raised higher so they remain separate
            // destinations instead of being intercepted by the card overlay.
            <Link
              to={to}
              className="rounded-sm hover:underline focus-visible:outline-none after:absolute after:inset-0 after:z-[1] after:rounded-lg focus-visible:after:ring-2 focus-visible:after:ring-ring focus-visible:after:ring-offset-1 focus-visible:after:ring-offset-bg"
            >
              <span className="text-muted">{project}</span>
              <span className="text-muted"> / </span>
              {suite}
            </Link>
          ) : (
            <>
              <span className="text-muted">{project}</span>
              <span className="text-muted"> / </span>
              {suite}
            </>
          )}
        </h2>
        {canonical ? (
          <span className="text-[11px] text-muted">
            ci · {canonical.git_branch ?? "—"}
          </span>
        ) : null}
      </div>

      {canonical ? (
        <>
          <div className="mt-3 flex items-center gap-3">
            <PassRateBadge
              pass={canonical.pass_count}
              fail={canonical.fail_count}
              error={canonical.error_count}
            />
            <DeltaLabel points={canonicalDelta(runs)} />
          </div>
          {/* `relative z-10` on every link below the heading, and it is not
              optional: the heading's ::after is a positioned box, and CSS
              paints all positioned boxes above all non-positioned in-flow
              content whatever the DOM order — so without a stacking level of
              their own these anchors sit underneath the card overlay and
              become unclickable. `relative` alone does nothing; z-index is
              ignored on statically positioned elements. */}
          <div className="mt-1 text-xs text-muted">
            <Link
              to={runPath(canonical.id)}
              className="relative z-10 font-mono text-accent hover:underline"
            >
              {shortRunId(canonical.id)}
            </Link>
            <span> · {formatRelative(canonical.created_at)}</span>
            {canonical.actor ? <span> · {canonical.actor}</span> : null}
            {canonical.ci_run_url ? (
              <>
                {" · "}
                <a
                  href={canonical.ci_run_url}
                  target="_blank"
                  rel="noreferrer"
                  className="relative z-10 text-accent hover:underline"
                >
                  CI ↗
                </a>
              </>
            ) : null}
          </div>
          {series.length > 1 ? (
            <div className="mt-3">
              {/* Canonical runs only. Feeding this every run is what made the
                  existing suite trend meaningless: one broken scratch run drags
                  the line down and it stops saying anything about the product. */}
              <Sparkline values={series} />
            </div>
          ) : null}
        </>
      ) : (
        <div className="mt-3">
          <div className="text-2xl font-semibold text-muted">—</div>
          <p className="mt-1 text-xs text-muted">
            No CI run for this suite yet.
            {runs.length > 0 ? " Only developer runs so far." : ""}
          </p>
        </div>
      )}

      <div className="mt-3 flex flex-wrap items-center gap-1.5">
        {stale ? (
          <Chip tone="amber" size="xs">
            stale — no CI run recently
          </Chip>
        ) : null}
        {canonical && canonical.error_count > 0 ? (
          <Chip tone="error" size="xs">
            {canonical.error_count} errored
          </Chip>
        ) : null}
        {/* This card keeps showing a fully-cached headline run rather than
            falling back to an older fresh one — a cached run still has a real
            verdict, and skipping it would report a stale number as current.
            But "these results were replayed, not re-measured" changes how much
            the number is worth, so it is said rather than left to infer. */}
        {canonical && isFullyCached(canonical) ? (
          <Chip tone="neutral" size="xs">
            cached
          </Chip>
        ) : null}
      </div>

      <div className="mt-3 border-t border-border pt-2 text-[11px] text-muted">
        <Link to={runsHref} className="relative z-10 hover:underline">
          {runs.length} run{runs.length === 1 ? "" : "s"} loaded
          {localCount > 0 ? ` · ${localCount} local` : ""}
          {actors > 0 ? ` · ${actors} actor${actors === 1 ? "" : "s"}` : ""}
          {" ›"}
        </Link>
      </div>
    </div>
  );
}
