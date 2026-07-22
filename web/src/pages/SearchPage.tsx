import { Link, useSearchParams } from "react-router";
import type { CaseSearchHit, RunSearchHit } from "@/api";
import { useSearch } from "@/api/queries";
import { Snippet } from "@/components/Snippet";
import { StatusBadge } from "@/components/StatusBadge";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { ErrorState, EmptyState } from "@/components/States";
import { formatDate, shortRunId } from "@/lib/format";

/** Full results for the header search bar's query (`/search?q=…`). */
export function SearchPage() {
  const [params] = useSearchParams();
  const q = params.get("q") ?? "";
  const search = useSearch(q, { limit: 50 });

  return (
    <div className="mx-auto max-w-3xl space-y-5">
      <div>
        <h1 className="text-xl font-semibold tracking-tight">Search</h1>
        <p className="mt-1 text-sm text-muted">
          {q
            ? `Results for “${q}” across runs and cases.`
            : "Type in the search bar above to search prompts, outputs, errors, branches, tags, projects, and suites."}
        </p>
      </div>

      {!q ? null : search.isPending ? (
        <CenteredSpinner label="Searching…" />
      ) : search.isError ? (
        <ErrorState error={search.error} onRetry={() => search.refetch()} />
      ) : search.data ? (
        search.data.runs.length === 0 && search.data.cases.length === 0 ? (
          <EmptyState title="No matches">Try fewer or shorter words.</EmptyState>
        ) : (
          <>
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
      <h2 className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted">
        {title}
      </h2>
      <div className="divide-y divide-border rounded-xl border border-border bg-surface">
        {children}
      </div>
    </section>
  );
}

function RunHit({ hit }: { hit: RunSearchHit }) {
  return (
    <Link
      to={`/runs/${encodeURIComponent(hit.id)}`}
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
      to={`/runs/${encodeURIComponent(hit.run_id)}?case=${encodeURIComponent(hit.case_key)}`}
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
