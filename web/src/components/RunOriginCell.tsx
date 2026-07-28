import type { RunListItem } from "@/api";
import { Chip } from "./ui/Chip";
import { Tooltip } from "./ui/Tooltip";

/**
 * Whether a run came from CI.
 *
 * `ci_provider` is the exact signal: the engine's CI detection returns a
 * provider (`"ci"`) even for a bare `CI` environment variable, so no CI run
 * lacks one. There is deliberately no separate boolean that could disagree.
 */
export function isCiRun(run: Pick<RunListItem, "ci_provider">): boolean {
  return run.ci_provider != null;
}

/**
 * Who produced a run, for the runs list: an origin chip plus the actor.
 *
 * Two different people can be involved and the distinction matters, so the
 * tooltip carries both: `actor` is who *ran* it (client-recorded, and the only
 * one a local run has), `uploaded_by` is who *pushed* it (authenticated, and
 * for CI usually a shared token that names nobody). Showing only one would
 * misattribute half the runs on a shared board.
 */
export function RunOriginCell({ run }: { run: RunListItem }) {
  const ci = isCiRun(run);
  const who = run.actor ?? run.uploaded_by;

  const detail = [
    run.actor ? `ran by ${run.actor}` : null,
    run.host ? `on ${run.host}` : null,
    run.uploaded_by && run.uploaded_by !== run.actor
      ? `uploaded by ${run.uploaded_by}`
      : null,
    run.domarinn_version ? `domarinn ${run.domarinn_version}` : null,
    run.note,
  ]
    .filter(Boolean)
    .join(" · ");

  const chip = (
    <Chip tone={ci ? "accent" : "neutral"} size="xs">
      {ci ? "CI" : "local"}
    </Chip>
  );

  return (
    <span className="flex items-center gap-1.5">
      {/* A run from a client that predates provenance has nothing to say about
          itself; an empty tooltip would be worse than none. */}
      {detail ? <Tooltip content={detail}>{chip}</Tooltip> : chip}
      <span className="truncate text-xs text-muted" title={who ?? undefined}>
        {who ?? "—"}
      </span>
    </span>
  );
}
