import { useState } from "react";
import { Link } from "react-router";
import { useCacheStats, useMeta, usePruneCache } from "@/api/queries";
import { formatBytes, formatInt, formatPercent, formatRelative } from "@/lib/format";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { ErrorState } from "@/components/States";
import { Button } from "@/components/ui/Button";
import { ApiError } from "@/api/client";
import { useAuth } from "@/auth/AuthProvider";

export function CacheStatsPage() {
  const stats = useCacheStats();
  const meta = useMeta();
  const { view } = useAuth();
  const prune = usePruneCache();
  const [confirming, setConfirming] = useState(false);

  const isOpen = meta.data?.auth_mode === "open";

  if (stats.isPending) return <CenteredSpinner label="Loading cache stats…" />;
  if (stats.isError) return <ErrorState error={stats.error} onRetry={() => stats.refetch()} />;

  const s = stats.data;
  const hitRate = s.hits + s.misses === 0 ? null : s.hits / (s.hits + s.misses);

  return (
    <div className="space-y-5">
      <div className="flex items-start justify-between gap-3">
        <div>
        <h1 className="text-lg font-semibold tracking-tight">Cache</h1>
        <p className="text-sm text-muted">
          Content-addressed cache shared between runs and teammates. Lookup hit rate
          counts lookups this cache served, not a run's own cached-cases rate (that
          one is on the run page and in the CI summary).
        </p>
        </div>
        {/* Hidden rather than disabled for a non-admin: listing entries is
            admin-scoped on the server, so offering the link would only lead to
            a forbidden page. */}
        {view.canAdmin ? (
          <Link
            to="/cache/entries"
            className="shrink-0 rounded-lg border border-border bg-surface px-3 py-1.5 text-sm font-medium text-fg hover:bg-surface-2"
          >
            Browse entries →
          </Link>
        ) : null}
      </div>

      <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-5">
        <Tile label="Entries" value={formatInt(s.entries)} />
        <Tile label="Total size" value={formatBytes(s.total_bytes)} />
        <Tile label="Lookup hit rate" value={formatPercent(hitRate)} accent />
        <Tile label="Hits" value={formatInt(s.hits)} />
        <Tile label="Misses" value={formatInt(s.misses)} />
      </div>

      <div className="rounded-xl border border-border bg-surface p-4">
        <div className="text-sm text-muted">
          Oldest entry:{" "}
          <span className="text-fg">{formatRelative(s.oldest_entry_at)}</span>
        </div>
        {s.unindexed > 0 ? (
          <div className="mt-1 text-sm text-amber">
            {formatInt(s.unindexed)} entries are still being indexed. Until that
            finishes they are listed but cannot be filtered by kind or model, and
            search will not reach them.
          </div>
        ) : null}
      </div>

      <div className="rounded-xl border border-border bg-surface p-4">
        <h2 className="text-sm font-semibold">Prune cache</h2>
        <p className="mt-1 text-sm text-muted">
          Remove expired and least-recently-used entries. This is an admin action
          {isOpen ? "" : " and requires a valid token"}.
        </p>
        <div className="mt-3 flex items-center gap-2">
          {confirming ? (
            <>
              <Button
                variant="danger"
                size="sm"
                onClick={() => {
                  prune.mutate(undefined, { onSettled: () => setConfirming(false) });
                }}
                disabled={prune.isPending}
              >
                {prune.isPending ? "Pruning…" : "Confirm prune"}
              </Button>
              <Button variant="ghost" size="sm" onClick={() => setConfirming(false)}>
                Cancel
              </Button>
            </>
          ) : (
            <Button variant="secondary" size="sm" onClick={() => setConfirming(true)}>
              Prune cache…
            </Button>
          )}
        </div>
        {prune.isError ? (
          <p className="mt-2 text-sm text-fail">
            {prune.error instanceof ApiError && prune.error.status === 401
              ? "Unauthorized — enter an admin token in the modal, then retry."
              : "Prune failed."}
          </p>
        ) : null}
        {prune.isSuccess ? (
          <p className="mt-2 text-sm text-pass">Cache pruned.</p>
        ) : null}
      </div>
    </div>
  );
}

function Tile({
  label,
  value,
  accent,
}: {
  label: string;
  value: string;
  accent?: boolean;
}) {
  return (
    <div className="rounded-xl border border-border bg-surface p-4">
      <div className="text-[11px] font-medium uppercase tracking-wide text-muted">
        {label}
      </div>
      <div
        className={`mt-1 text-2xl font-semibold tabular-nums ${accent ? "text-accent" : ""}`}
      >
        {value}
      </div>
    </div>
  );
}
