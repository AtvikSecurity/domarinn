import { Link, useSearchParams } from "react-router";
import type { CaseSearchHit, ProjectSetView, RunSearchHit } from "@/api";
import { useSearch, useSetSearch } from "@/api/queries";
import { Snippet } from "@/components/Snippet";
import { StatusBadge } from "@/components/StatusBadge";
import { Chip } from "@/components/ui/Chip";
import { CHROME_FRAME } from "@/components/ui/chrome";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { ErrorState, EmptyState } from "@/components/States";
import {
  formatDate,
  formatInt,
  formatRelative,
  isoFromEpoch,
  shortRunId,
} from "@/lib/format";
import { runPath, setsPath } from "@/lib/routes";
import { resolveCached } from "@/lib/cached";
import { useCachedPref } from "@/lib/cachedPref";
import { mergeParams } from "@/lib/filters";
import { cn } from "@/lib/cn";
import { CachedRunsToggle } from "@/components/CachedRunsToggle";

/** Higher than the dropdown's — there is room here — but still bounded. */
const SETS_LIMIT = 25;

/** Full results for the header search bar's query (`/search?q=…`). */
export function SearchPage() {
  const [params, setParams] = useSearchParams();
  const q = params.get("q") ?? "";
  const cachedPref = useCachedPref();
  const resolvedCached = resolveCached(params.get("cached"), cachedPref);
  const search = useSearch(q, { limit: 50, cached: params.get("cached") });
  // The dropdown offers a Sets group, so this page has to as well: promising a
  // group and then omitting it on "See all results" is worse than not having
  // it at all.
  const setSearch = useSetSearch(q, {
    enabled: q.trim().length > 0,
    limit: SETS_LIMIT,
  });
  const sets = setSearch.matches;

  return (
    <div className="mx-auto max-w-3xl space-y-5">
      <div>
        <h1 className="text-xl font-semibold tracking-tight">Search</h1>
        <p className="mt-1 text-sm text-muted">
          {q
            ? `Results for “${q}” across sets, runs and cases.`
            : "Type in the search bar above to search prompts, outputs, errors, branches, tags, projects, and suites."}
        </p>
      </div>

      {/* No count here, unlike the runs list. With bm25 ranking and a LIMIT,
          "suppressed overall" is not "suppressed from this page", and the
          COUNT(*) that would answer it cannot use the partial index because
          the row set comes from an FTS match rather than a scan of runs. A
          number we cannot compute honestly is worse than none. */}
      {q ? (
        <CachedRunsToggle
          resolved={resolvedCached}
          hiddenCount="unknown"
          onChange={(next) =>
            setParams(mergeParams(params, { cached: next }), { replace: true })
          }
        />
      ) : null}

      {!q ? null : search.isPending ? (
        <CenteredSpinner label="Searching…" />
      ) : search.isError ? (
        // Deliberately wins over any set matches. This page's promise is
        // full-text search; quietly degrading to a project list while the
        // search itself failed would misreport the failure as an empty result.
        <ErrorState error={search.error} onRetry={() => search.refetch()} />
      ) : search.data ? (
        search.data.runs.length === 0 &&
        search.data.cases.length === 0 &&
        sets.length === 0 ? (
          <EmptyState title="No matches">Try fewer or shorter words.</EmptyState>
        ) : (
          <>
            {sets.length > 0 ? (
              <Section title={`Sets (${sets.length})`}>
                {sets.map((project) => (
                  <SetHit key={project.project} project={project} />
                ))}
              </Section>
            ) : null}
            {search.data.runs.length > 0 ? (
              <Section title={`Runs (${search.data.runs.length})`}>
                {search.data.runs.map((hit) => (
                  <RunHit key={hit.id} hit={hit} />
                ))}
              </Section>
            ) : null}
            {search.data.cases.length > 0 ? (
              <Section title={`Cases (${search.data.cases.length})`}>
                {search.data.cases.map((hit) => (
                  <CaseHit key={`${hit.run_id}-${hit.case_key}`} hit={hit} />
                ))}
              </Section>
            ) : null}
          </>
        )
      ) : null}
    </div>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section>
      <h2 className="mb-2 font-mono text-[10px] font-medium uppercase tracking-[0.12em] text-muted">
        {title}
      </h2>
      <div className={cn(CHROME_FRAME, "divide-y divide-border overflow-hidden")}>
        {children}
      </div>
    </section>
  );
}

function SetHit({ project }: { project: ProjectSetView }) {
  // Epoch-ms, not RFC3339 — see the DTO note in src/api/index.ts.
  const last = isoFromEpoch(project.last_run_at);
  return (
    <Link
      to={setsPath(project.project)}
      className="block px-4 py-3 hover:bg-surface-2"
    >
      <div className="flex items-center gap-2">
        <span className="text-sm font-medium">{project.project}</span>
        {project.restricted ? (
          <Chip tone="amber" size="xs">
            restricted
          </Chip>
        ) : null}
      </div>
      <span className="mt-1 block text-xs text-muted">
        {formatInt(project.suite_count)} suite
        {project.suite_count === 1 ? "" : "s"} · {formatInt(project.run_count)}{" "}
        run{project.run_count === 1 ? "" : "s"}
        {last ? ` · last activity ${formatRelative(last)}` : ""}
      </span>
    </Link>
  );
}

function RunHit({ hit }: { hit: RunSearchHit }) {
  return (
    <Link
      to={runPath(hit.id)}
      className="block px-4 py-3 hover:bg-surface-2"
    >
      <div className="flex items-center gap-2">
        <span className="font-mono text-sm">{shortRunId(hit.id)}</span>
        <span className="text-xs text-muted">
          {hit.project ?? "—"} / {hit.suite ?? "—"} · {formatDate(hit.created_at)}
        </span>
      </div>
      <Snippet text={hit.snippet} className="mt-1 block text-xs text-muted" />
    </Link>
  );
}

function CaseHit({ hit }: { hit: CaseSearchHit }) {
  return (
    <Link
      to={`${runPath(hit.run_id)}?case=${encodeURIComponent(hit.case_key)}`}
      className="block px-4 py-3 hover:bg-surface-2"
    >
      <div className="flex items-center gap-2">
        <StatusBadge status={hit.status} size="xs" />
        <span className="truncate text-sm font-medium">{hit.name ?? hit.case_key}</span>
        <span className="shrink-0 text-xs text-muted">
          {hit.project ?? "—"} / {hit.suite ?? "—"} · run {shortRunId(hit.run_id)}
        </span>
      </div>
      <Snippet text={hit.snippet} className="mt-1 block text-xs text-muted" />
    </Link>
  );
}
