import { useEffect, useMemo, useState } from "react";
import { Link, useSearchParams } from "react-router";
import { useCacheEntries, useCacheFacets, useCacheStats } from "@/api/queries";
import { useFillViewport } from "@/components/AppShell";
import { EmptyState, ErrorState } from "@/components/States";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { CopyButton } from "@/components/ui/CopyButton";
import {
  activeCacheFilterCount,
  mergeParams,
  parseCacheFilters,
} from "@/lib/filters";
import { formatBytes, formatInt } from "@/lib/format";
import { parseSort, serializeSort } from "@/lib/sort";
import { CacheEntryDrawer } from "./CacheEntryDrawer";
import { CacheEntryGrid } from "./CacheEntryGrid";
import { CacheFilterBar } from "./CacheFilterBar";

/** The two lines a suite needs to start filling this server's tier. */
const LAYERED_SNIPPET = `cache:\n  backend: layered`;

export function CacheEntriesPage() {
  // The grid owns the viewport at `lg`; below that the shell scrolls the page
  // and the grid falls back to a bounded height.
  useFillViewport();

  const [params, setParams] = useSearchParams();
  const filters = useMemo(() => parseCacheFilters(params), [params]);
  const activeCount = activeCacheFilterCount(params);

  const stats = useCacheStats();
  const facets = useCacheFacets();
  const entriesQ = useCacheEntries(filters);

  const patch = (next: Record<string, string | undefined>) => {
    // Reset the drawer whenever the result set changes: the open entry may not
    // be in it any more, and a drawer showing a row that is no longer listed
    // is a confusing place to be.
    setParams(mergeParams(params, { ...next, entry: undefined }), {
      replace: true,
    });
  };

  // Debounced search box -> ?q=. Mirrors RunDetail, including the
  // adjust-during-render reset so switching filters never commits a frame with
  // a stale query in the box.
  const [search, setSearch] = useState(filters.q ?? "");
  const [prevQ, setPrevQ] = useState(filters.q ?? "");
  if (prevQ !== (filters.q ?? "")) {
    setPrevQ(filters.q ?? "");
    setSearch(filters.q ?? "");
  }
  useEffect(() => {
    const handle = setTimeout(() => {
      if ((filters.q ?? "") !== search) {
        setParams(mergeParams(params, { q: search || undefined, entry: undefined }), {
          replace: true,
        });
      }
    }, 300);
    return () => clearTimeout(handle);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [search]);

  const sorting = useMemo(() => parseSort(params.get("sort")), [params]);

  const entries = useMemo(
    () => entriesQ.data?.pages.flatMap((p) => p.entries) ?? [],
    [entriesQ.data],
  );
  const selectedKey = filters.entry;
  const selectedSize = entries.find((e) => e.key === selectedKey)?.size;

  if (entriesQ.isPending) return <CenteredSpinner label="Loading cache entries…" />;
  if (entriesQ.isError) {
    return <ErrorState error={entriesQ.error} onRetry={() => entriesQ.refetch()} />;
  }

  const total = stats.data?.entries ?? 0;
  const empty = entries.length === 0;

  return (
    <div className="flex flex-col gap-4 lg:min-h-0 lg:flex-1">
      <div className="shrink-0">
        <div className="flex items-baseline justify-between gap-3">
          <h1 className="text-lg font-semibold tracking-tight">Cache entries</h1>
          <Link to="/cache" className="text-sm text-accent hover:underline">
            ← Cache stats
          </Link>
        </div>
        <p className="text-sm text-muted tabular-nums">
          {formatInt(total)} entries · {formatBytes(stats.data?.total_bytes ?? 0)}
          {stats.data && stats.data.unindexed > 0 ? (
            <span className="text-amber">
              {" · "}
              {formatInt(stats.data.unindexed)} still indexing
            </span>
          ) : null}
        </p>
      </div>

      <CacheFilterBar
        filters={filters}
        facets={facets.data}
        activeCount={activeCount}
        onPatch={patch}
        search={search}
        onSearch={setSearch}
      />

      {empty && total === 0 ? (
        <ServerTierEmpty />
      ) : empty ? (
        <EmptyState title="No entries match these filters">
          <button
            type="button"
            className="font-medium text-accent hover:underline"
            onClick={() =>
              patch(
                Object.fromEntries(
                  ["kind", "model", "q", "since", "until"].map((k) => [k, undefined]),
                ),
              )
            }
          >
            Clear filters
          </button>
        </EmptyState>
      ) : (
        <CacheEntryGrid
          entries={entries}
          busy={entriesQ.isPlaceholderData}
          // Never page off placeholder data: the cursor belongs to the previous
          // filter, so its next page would be appended into this result set and
          // read as "the filter does not work".
          hasNextPage={entriesQ.hasNextPage && !entriesQ.isPlaceholderData}
          isFetchingNextPage={entriesQ.isFetchingNextPage}
          fetchNextPage={entriesQ.fetchNextPage}
          sorting={sorting}
          onSortingChange={(next) =>
            setParams(
              mergeParams(params, {
                sort: serializeSort(next) ?? undefined,
                entry: undefined,
              }),
              { replace: true },
            )
          }
          selectedKey={selectedKey}
          onSelect={(key) =>
            setParams(mergeParams(params, { entry: key }), { replace: true })
          }
        />
      )}

      <CacheEntryDrawer
        entryKey={selectedKey}
        size={selectedSize}
        onClose={() =>
          setParams(mergeParams(params, { entry: undefined }), { replace: true })
        }
      />
    </div>
  );
}

/**
 * The likeliest first-run experience, because `cache.backend` defaults to
 * `disk`. Saying "no entries" would be true and useless — the reason this tier
 * is empty has one common cause, and naming it is the whole value of the state.
 */
function ServerTierEmpty() {
  return (
    <EmptyState title="This server's cache is empty">
      <p className="mx-auto max-w-prose text-sm text-muted">
        Nothing has been written to the shared tier yet. domarinn caches to your
        own machine by default (<code className="font-mono">cache.backend: disk</code>),
        so runs never ask this server for anything. A suite starts filling this
        tier when it points at this server and sets:
      </p>
      <div className="mx-auto mt-3 flex w-fit items-center gap-2 rounded-lg border border-border bg-surface-2 px-3 py-2">
        <pre className="text-left font-mono text-xs text-fg">{LAYERED_SNIPPET}</pre>
        <CopyButton value={LAYERED_SNIPPET} label="Copy" />
      </div>
    </EmptyState>
  );
}
