import { Link } from "react-router";
import type { RunListItem } from "@/api";
import { PassRateBadge } from "@/components/PassRateBadge";
import { isCiRun } from "@/components/RunOriginCell";
import { isFullyCached } from "@/lib/cached";
import { formatDateAbsolute, formatInt, formatRelative, shortRunId } from "@/lib/format";
import { runPath, suitePath } from "@/lib/routes";

const LINK = "rounded-sm hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring";

export function RunActivity({ runs }: { runs: RunListItem[] }) {
  // The cursor supplies newest first across all suites. Don't regroup here:
  // an older run must not jump ahead just because its suite ran again.
  return (
    <ol role="list" aria-label="Run activity" className="divide-y divide-border rounded-lg border border-border bg-surface">
      {runs.map((run) => {
        const suiteLabel = [run.project, run.suite].filter(Boolean).join(" / ") || "Unassigned run";
        const setHref = run.project && run.suite ? suitePath(run.project, run.suite) : null;
        const issues = run.fail_count + (run.xpass_count ?? 0);
        const breakdown = [
          issues > 0 ? `${formatInt(issues)} failed` : null,
          run.error_count > 0 ? `${formatInt(run.error_count)} errored` : null,
        ].filter(Boolean).join(", ");
        return (
          <li key={run.id} className="grid min-w-0 grid-cols-[minmax(0,1fr)_auto] items-start gap-x-4 gap-y-2 px-3 py-3 sm:grid-cols-[minmax(0,1fr)_minmax(0,0.65fr)_minmax(6rem,auto)] sm:items-center sm:gap-x-5 sm:px-4">
            <div className="min-w-0">
              {setHref ? (
                <Link to={setHref} className={`${LINK} block w-fit max-w-full text-sm font-semibold wrap-anywhere`}>
                  {suiteLabel}
                </Link>
              ) : <span className="block text-sm font-semibold wrap-anywhere">{suiteLabel}</span>}
              <Link
                to={runPath(run.id)}
                aria-label={run.id}
                title={run.id}
                className={`${LINK} mt-1 inline-block max-w-full truncate font-mono text-xs text-accent`}
              >
                {shortRunId(run.id)}
              </Link>
            </div>
            <div className="col-span-2 row-start-2 flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted sm:col-span-1 sm:col-start-2 sm:row-start-1">
              <span>{isCiRun(run) ? "CI" : "Local"}</span>
              <time dateTime={run.created_at} title={formatDateAbsolute(run.created_at)}>{formatRelative(run.created_at)}</time>
              {run.git_branch ? <span className="max-w-full truncate font-mono" title={run.git_branch}>{run.git_branch}</span> : null}
              {isFullyCached(run) ? <span className="rounded border border-border px-1.5">Cached</span> : null}
            </div>
            <div className="col-start-2 row-start-1 grid justify-items-end gap-1 text-right sm:col-start-3">
              {run.pass_count + issues + run.error_count > 0 ? (
                <PassRateBadge pass={run.pass_count} fail={run.fail_count} error={run.error_count} xpass={run.xpass_count} />
              ) : <span className="text-xs text-muted">No scored cases</span>}
              <span className="text-xs text-muted">{breakdown || `${formatInt(run.case_count)} ${run.case_count === 1 ? "case" : "cases"}`}</span>
            </div>
          </li>
        );
      })}
    </ol>
  );
}
