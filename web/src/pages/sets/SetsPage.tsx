import { Link } from "react-router";
import { useSets } from "@/api/queries";
import { Card } from "@/components/ui/Card";
import { Chip } from "@/components/ui/Chip";
import { CopyButton } from "@/components/ui/CopyButton";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { Tooltip } from "@/components/ui/Tooltip";
import { EmptyState, ErrorState } from "@/components/States";
import { Sparkline } from "@/components/Sparkline";
import {
  formatDateAbsolute,
  formatInt,
  formatRelative,
  isoFromEpoch,
} from "@/lib/format";

/** What a suite has to declare before its runs form a set. */
const PROJECT_SNIPPET = `project: checkout-agent\nsuite: regression`;

/**
 * The run-set browser's root: every project this caller may see.
 *
 * A navigation surface, not a report — no filters, because the runs list
 * already has them and the question here is "which sets exist and who can
 * reach them". The listing is never paginated: project cardinality is small by
 * construction, and the server returns it whole.
 */
export function SetsPage() {
  const q = useSets();
  const projects = q.data?.projects ?? [];

  return (
    <div className="space-y-5">
      <div>
        <h1 className="text-lg font-semibold tracking-tight">Run sets</h1>
        <p className="text-sm text-muted">
          Every project you can see, and who may reach it. A set is a
          project/suite pair; restricted ones are visible only to the people
          granted them.
        </p>
      </div>

      {q.isPending ? (
        <CenteredSpinner label="Loading run sets…" />
      ) : q.isError ? (
        <ErrorState error={q.error} onRetry={() => q.refetch()} />
      ) : projects.length === 0 ? (
        <NoSets />
      ) : (
        <Card padding="flush">
          {/* `relative` is load-bearing, not decoration. `sr-only` is
              `position: absolute`, and without a positioned ancestor its
              containing block is the document — so the visually-hidden header
              of the trailing Access column, sitting ~765px into a 760px-wide
              table, widened the *page* rather than this scroller and gave
              /sets a sideways scrollbar at phone widths. Making the scroll
              container the containing block keeps that overflow in here. */}
          <div className="relative overflow-x-auto">
            <table className="w-full min-w-[760px] text-sm">
              <thead>
                <tr className="border-b border-border text-left text-xs uppercase tracking-wide text-muted">
                  <th className="px-4 py-2 font-medium">Project</th>
                  <th className="px-3 py-2 text-right font-medium">Suites</th>
                  <th className="px-3 py-2 text-right font-medium">Runs</th>
                  <th className="px-3 py-2 font-medium">Last activity</th>
                  {/* Not "pass rate": this is one latest rate per suite, a
                      spread across the project rather than a trend. */}
                  <th className="px-3 py-2 font-medium">Suite pass rates</th>
                  <th className="px-3 py-2 text-right font-medium">
                    <span className="sr-only">Access</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {projects.map((p) => {
                  const last = isoFromEpoch(p.last_run_at);
                  return (
                    <tr
                      key={p.project}
                      data-testid={`set-row-${p.project}`}
                      className="border-b border-border/60 last:border-0 hover:bg-surface-2/60"
                    >
                      <td className="px-4 py-2">
                        {/* Project names are words, not ids — no font-mono. */}
                        <Link
                          to={`/sets/${encodeURIComponent(p.project)}`}
                          className="rounded-sm font-medium text-accent hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                        >
                          {p.project}
                        </Link>
                      </td>
                      <td className="px-3 py-2 text-right tabular-nums">
                        {formatInt(p.suite_count)}
                      </td>
                      <td className="px-3 py-2 text-right tabular-nums">
                        {formatInt(p.run_count)}
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
                        {/* Per-suite latest rates, not a time series: the
                            spread across the project's suites at a glance. */}
                        <Sparkline
                          values={p.recent_pass_rates}
                          min={0}
                          max={1}
                          title={`Latest pass rate per suite in ${p.project}`}
                        />
                      </td>
                      <td className="px-3 py-2 text-right">
                        {p.restricted ? (
                          <Chip
                            tone="amber"
                            size="xs"
                            title="Visible only to the people granted this project"
                          >
                            restricted
                          </Chip>
                        ) : null}
                        {p.my_level ? (
                          <Chip
                            size="xs"
                            className="ml-1"
                            title="The grant you hold over this project"
                          >
                            {p.my_level}
                          </Chip>
                        ) : null}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </Card>
      )}
    </div>
  );
}

/**
 * Empty for one of two reasons — nothing has been uploaded, or everything that
 * has is restricted away from this caller — and neither is visible from here.
 * Naming what makes a set is the only useful thing this state can say.
 */
function NoSets() {
  return (
    <EmptyState title="No run sets yet">
      <p className="mx-auto max-w-prose text-sm text-muted">
        A set appears once a run declares which project and suite it belongs to.
        Add them to your suite YAML and upload a run:
      </p>
      <div className="mx-auto mt-3 flex w-fit items-center gap-2 rounded-lg border border-border bg-surface-2 px-3 py-2">
        <pre className="text-left font-mono text-xs text-fg">
          {PROJECT_SNIPPET}
        </pre>
        <CopyButton value={PROJECT_SNIPPET} label="Copy" />
      </div>
      <p className="mx-auto mt-3 max-w-prose text-xs text-muted">
        Sets you have not been granted are not listed here at all.
      </p>
    </EmptyState>
  );
}
