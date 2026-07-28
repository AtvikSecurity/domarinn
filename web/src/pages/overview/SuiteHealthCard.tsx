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
import { Sparkline } from "@/components/Sparkline";
import { PassRateBadge } from "@/components/PassRateBadge";
import { Chip } from "@/components/ui/Chip";

/** Border tint by severity, so the card reads before you focus on it. */
const EDGE: Record<Severity, string> = {
  failing: "border-fail/50",
  stale: "border-amber/50",
  drifting: "border-amber/30",
  unknown: "border-border",
  healthy: "border-border",
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
  runs,
  now,
}: {
  project: string;
  suite: string;
  runs: RunListItem[];
  now: number;
}) {
  const canonical = canonicalRun(runs);
  const severity = suiteSeverity(runs, now);
  const series = canonicalSeries(runs);
  const stale = isStale(runs, now);
  const localCount = runs.filter((r) => r.ci_provider == null).length;
  const actors = new Set(runs.map((r) => r.actor).filter(Boolean)).size;
  const runsHref = `/runs?project=${encodeURIComponent(project)}&suite=${encodeURIComponent(suite)}&cached=all`;

  return (
    <div className={`rounded-xl border ${EDGE[severity]} bg-surface p-4`}>
      <div className="flex items-baseline justify-between gap-2">
        <h2 className="text-sm font-semibold">
          <span className="text-muted">{project}</span>
          <span className="text-muted"> / </span>
          {suite}
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
          <div className="mt-1 text-xs text-muted">
            <Link
              to={`/runs/${encodeURIComponent(canonical.id)}`}
              className="font-mono text-accent hover:underline"
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
                  className="text-accent hover:underline"
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
      </div>

      <div className="mt-3 border-t border-border pt-2 text-[11px] text-muted">
        <Link to={runsHref} className="hover:underline">
          {runs.length} run{runs.length === 1 ? "" : "s"} loaded
          {localCount > 0 ? ` · ${localCount} local` : ""}
          {actors > 0 ? ` · ${actors} actor${actors === 1 ? "" : "s"}` : ""}
          {" ›"}
        </Link>
      </div>
    </div>
  );
}
