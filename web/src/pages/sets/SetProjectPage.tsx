import { useState } from "react";
import { Link, useParams } from "react-router";
import { useSetProject } from "@/api/queries";
import { ApiError } from "@/api/client";
import { useAuthView } from "@/auth/AuthProvider";
import { Breadcrumb } from "@/components/ui/Breadcrumb";
import { Button } from "@/components/ui/Button";
import { Card } from "@/components/ui/Card";
import { Chip } from "@/components/ui/Chip";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { Tooltip } from "@/components/ui/Tooltip";
import { NotFound } from "@/components/NotFound";
import { ErrorState } from "@/components/States";
import { RateBadge } from "@/components/PassRateBadge";
import { Sparkline } from "@/components/Sparkline";
import {
  formatDateAbsolute,
  formatInt,
  formatRelative,
  isoFromEpoch,
} from "@/lib/format";
import { suitePath } from "@/lib/routes";
import { useRowNav } from "@/lib/useRowNav";
import { AccessPanel } from "./AccessPanel";

/**
 * One project's suites.
 *
 * A 404 renders the app's not-found page rather than an error: the server
 * refuses to distinguish "restricted away from you" from "never existed", and
 * a page saying "request failed (404)" would leak that the difference exists.
 */
export function SetProjectPage() {
  const { project = "" } = useParams();
  const view = useAuthView();
  const q = useSetProject(project);
  const [accessOpen, setAccessOpen] = useState(false);
  const rowNav = useRowNav();

  if (q.isPending) return <CenteredSpinner label="Loading project…" />;
  if (q.isError) {
    if (q.error instanceof ApiError && q.error.status === 404) return <NotFound />;
    return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  }

  const detail = q.data;
  const runCount = detail.suites.reduce((n, s) => n + s.run_count, 0);
  // Reading the panel is read-scoped, so a viewer-role manager may open it;
  // the panel itself is what gates the mutations inside.
  const canOpenAccess = view.canAdmin || detail.my_level === "manage";

  return (
    <div className="space-y-5">
      <div>
        <Breadcrumb items={[{ label: "Sets", to: "/sets" }, { label: project }]} />
        <div className="mt-1 flex flex-wrap items-center gap-2">
          <h1 className="text-lg font-semibold tracking-tight">{project}</h1>
          {detail.restricted ? (
            <Chip
              tone="amber"
              size="xs"
              title="Visible only to the people granted this project"
            >
              restricted
            </Chip>
          ) : null}
          {detail.my_level ? (
            <Chip size="xs" title="The grant you hold over this project">
              {detail.my_level}
            </Chip>
          ) : null}
          {canOpenAccess ? (
            <Button
              className="ml-auto"
              variant="secondary"
              size="sm"
              onClick={() => setAccessOpen(true)}
            >
              Access
            </Button>
          ) : null}
        </div>
        <p className="text-sm text-muted tabular-nums">
          {formatInt(detail.suites.length)}{" "}
          {detail.suites.length === 1 ? "suite" : "suites"} ·{" "}
          {formatInt(runCount)} {runCount === 1 ? "run" : "runs"}
        </p>
      </div>

      <Card padding="flush">
        {/* `relative`: contains the trailing Flags column's `sr-only` header,
            which is `position: absolute` and would otherwise widen the page
            instead of this scroller. See SetsPage for the full account. */}
        <div className="relative overflow-x-auto">
          <table className="w-full min-w-[820px] text-sm">
            <thead>
              <tr className="border-b border-border text-left text-xs uppercase tracking-wide text-muted">
                <th className="px-4 py-2 font-medium">Suite</th>
                <th className="px-3 py-2 text-right font-medium">Runs</th>
                <th className="px-3 py-2 font-medium">Last activity</th>
                <th className="px-3 py-2 font-medium">Latest pass rate</th>
                <th className="px-3 py-2 font-medium">Trend</th>
                <th className="px-3 py-2 text-right font-medium">
                  <span className="sr-only">Flags</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {detail.suites.map((s) => {
                const last = isoFromEpoch(s.last_run_at);
                return (
                  <tr
                    key={s.suite}
                    data-testid={`suite-row-${s.suite}`}
                    {...rowNav(suitePath(project, s.suite))}
                    className="cursor-pointer border-b border-border/60 last:border-0 hover:bg-surface-2/60"
                  >
                    <td className="px-4 py-2">
                      <Link
                        to={suitePath(project, s.suite)}
                        className="rounded-sm font-medium text-accent hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                      >
                        {s.suite}
                      </Link>
                    </td>
                    <td className="px-3 py-2 text-right tabular-nums">
                      {formatInt(s.run_count)}
                    </td>
                    <td className="px-3 py-2 text-muted">
                      {last ? (
                        <Tooltip content={formatDateAbsolute(last)}>
                          <time dateTime={last}>{formatRelative(last)}</time>
                        </Tooltip>
                      ) : (
                        "-"
                      )}
                    </td>
                    <td className="px-3 py-2">
                      <RateBadge
                        rate={s.latest_pass_rate}
                        title={`Newest run of ${s.suite}`}
                      />
                    </td>
                    <td className="px-3 py-2">
                      <Sparkline
                        values={s.sparkline}
                        min={0}
                        max={1}
                        title={`Pass rate trend for ${s.suite}`}
                      />
                    </td>
                    <td className="px-3 py-2 text-right">
                      <span className="inline-flex items-center gap-1">
                        {s.baseline_run_id ? (
                          <Chip
                            tone="accent"
                            size="xs"
                            title={`Pinned comparison baseline: ${s.baseline_run_id}`}
                          >
                            baseline
                          </Chip>
                        ) : null}
                        {/* Only when the suite's own lock is what restricts it:
                            inside an already-restricted project the chip would
                            repeat the heading's, on every row. */}
                        {s.restricted && !detail.restricted ? (
                          <Chip
                            tone="amber"
                            size="xs"
                            title="This suite is restricted on its own"
                          >
                            restricted
                          </Chip>
                        ) : null}
                      </span>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </Card>

      <AccessPanel
        project={project}
        suite={null}
        // The panel's own payload is exact-scope; only the browse detail knows
        // whether anything covers the set.
        coveringRestricted={detail.restricted}
        open={accessOpen}
        onClose={() => setAccessOpen(false)}
      />
    </div>
  );
}
