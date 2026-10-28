import { useState } from "react";
import { Link } from "react-router";
import { useCacheStats, useMeta, usePruneCache } from "@/api/queries";
import { formatBytes, formatInt, formatPercent, formatRelative } from "@/lib/format";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { ErrorState } from "@/components/States";
import { Button } from "@/components/ui/Button";
import { Card } from "@/components/ui/Card";
import { StatBlock } from "@/components/ui/StatBlock";
import { TextField } from "@/components/ui/TextField";
import { ApiError } from "@/api/client";
import { apiErrorMessage } from "@/api/errorMessage";
import { useAuth } from "@/auth/AuthProvider";

export function CacheStatsPage() {
  const stats = useCacheStats();
  const meta = useMeta();
  const { view } = useAuth();
  const prune = usePruneCache();
  const [confirming, setConfirming] = useState(false);
  // Raw field text; blank means "the configured retention".
  const [days, setDays] = useState("");
  const olderThanDays = parseDays(days);

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

      <Card>
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
      </Card>

      <Card>
        <h2 className="text-[10px] font-medium uppercase tracking-wide text-muted">
          Prune cache
        </h2>
        {/* The endpoint is admin-scoped, so the control is shown only to
            someone it would not refuse — as "Browse entries" above is. */}
        {view.canAdmin ? (
          <>
            <p className="mt-1 text-sm text-muted">
              Leave the age blank to apply this server's configured retention
              (age and size limits). Give an age to remove only entries
              older than that many days.
            </p>
            <div className="mt-3 flex flex-wrap items-end gap-2">
              <div className="w-40">
                <TextField
                  label="Older than (days)"
                  type="number"
                  inputMode="numeric"
                  min={0}
                  step={1}
                  placeholder="retention"
                  value={days}
                  onChange={(e) => {
                    setDays(e.target.value);
                    setConfirming(false);
                  }}
                  disabled={prune.isPending}
                />
              </div>
              {confirming ? (
                <>
                  <Button
                    variant="danger"
                    size="sm"
                    onClick={() => {
                      prune.mutate(
                        { olderThanDays: olderThanDays ?? undefined },
                        { onSettled: () => setConfirming(false) },
                      );
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
                <Button
                  variant="secondary"
                  size="sm"
                  onClick={() => {
                    prune.reset();
                    setConfirming(true);
                  }}
                  disabled={olderThanDays === null && days.trim() !== ""}
                >
                  Prune cache…
                </Button>
              )}
            </div>
            {confirming ? (
              <p className="mt-2 text-sm text-muted">
                {olderThanDays === null
                  ? "Entries past the configured retention limits will be removed."
                  : `Every entry older than ${formatInt(olderThanDays)} ${
                      olderThanDays === 1 ? "day" : "days"
                    } will be removed.`}{" "}
                This cannot be undone.
              </p>
            ) : null}
            {olderThanDays === null && days.trim() !== "" ? (
              <p className="mt-2 text-sm text-fail">
                Enter a whole number of days, or leave it blank.
              </p>
            ) : null}
            {prune.isError ? (
              <p role="alert" className="mt-2 text-sm text-fail">
                {prune.error instanceof ApiError && prune.error.status === 401
                  ? "Your session has expired. Sign in again, then retry."
                  : apiErrorMessage(prune.error, "Prune failed.")}
              </p>
            ) : null}
            {prune.isSuccess ? (
              <p role="status" className="mt-2 text-sm text-pass">
                {prune.data.pruned === 0
                  ? "Nothing to prune."
                  : `Pruned ${formatInt(prune.data.pruned)} ${
                      prune.data.pruned === 1 ? "entry" : "entries"
                    }.`}
              </p>
            ) : null}
          </>
        ) : (
          <p className="mt-1 text-sm text-muted">
            Expired and least-recently-used entries are removed on this
            server's retention schedule. Pruning by hand is an admin action
            {isOpen ? "" : "; sign in as an admin to do it"}.
          </p>
        )}
      </Card>
    </div>
  );
}

/**
 * The age field as `older_than_days`: a non-negative whole number, or `null`
 * for blank or anything else. The server refuses a negative window (it would
 * put the cutoff in the future and match every entry), so it never leaves here.
 */
function parseDays(raw: string): number | null {
  const text = raw.trim();
  if (!/^\d+$/.test(text)) return null;
  const n = Number(text);
  return Number.isSafeInteger(n) ? n : null;
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
    <StatBlock label={label} tone={accent ? "text-accent" : undefined} className="p-4">
      <span className="text-2xl font-semibold tabular-nums">{value}</span>
    </StatBlock>
  );
}
