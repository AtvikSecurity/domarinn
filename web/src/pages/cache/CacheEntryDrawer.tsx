import { useState } from "react";
import type { CacheEntryDetail } from "@/api";
import { Link } from "react-router";
import { useCacheEntry, useCacheEntryRuns } from "@/api/queries";
import { JsonTree, OutputViewer, RawText } from "@/components/output";
import { ErrorState } from "@/components/States";
import { Chip } from "@/components/ui/Chip";
import { CollapsibleSection } from "@/components/ui/CollapsibleSection";
import { CopyButton } from "@/components/ui/CopyButton";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { StatBlock } from "@/components/ui/StatBlock";
import { DetailDrawer } from "@/components/ui/DetailDrawer";
import {
  formatBytes,
  formatCost,
  formatLatency,
  formatRelative,
  formatTokens,
  shortCacheKey,
} from "@/lib/format";

/**
 * Above this, the Output and Request sections start collapsed.
 *
 * The case drawer's convention is `defaultOpen`, and this deliberately breaks
 * it. A `CaseResult`'s `raw` is capped at 64 KiB by the runner, but a cache
 * entry's `output` has no separate cap below the 4 MiB entry limit — a 4 MiB
 * entry is 4 MiB of output. The decision is made from the row's `size`, which
 * the list already carries, so it happens before the detail request even
 * lands.
 */
const SMALL_ENTRY_BYTES = 256 * 1024;

/**
 * Above this, a payload goes to `RawText` instead of `JsonTree`.
 *
 * `JsonTree` mounts a component with its own state per node and its collapse
 * threshold only sets *initial* open state — it does not bound the tree. On a
 * provider response with per-token logprobs that is tens of thousands of
 * components and a hung tab. The case drawer is safe from this only because
 * the runner caps what it stores; the cache path does not.
 */
const JSON_TREE_MAX_BYTES = 128 * 1024;

function Payload({ value }: { value: unknown }) {
  const text = JSON.stringify(value, null, 2) ?? "";
  if (text.length > JSON_TREE_MAX_BYTES) {
    return <RawText text={text} wrap maxHeight="24rem" />;
  }
  return <JsonTree data={value} />;
}

export interface CacheEntryDrawerProps {
  entryKey?: string;
  /** From the list row, so section defaults are decided before the fetch lands. */
  size?: number;
  onClose: () => void;
  /** Step to the neighbouring entry. Omitted at the ends of the loaded rows. */
  onPrev?: () => void;
  onNext?: () => void;
  position?: { index: number; total: number };
}

export function CacheEntryDrawer({
  entryKey,
  size,
  onClose,
  onPrev,
  onNext,
  position,
}: CacheEntryDrawerProps) {
  // `raw` is the largest member of an entry and the least often wanted, so the
  // server withholds it until asked. Asking re-fetches under a different key.
  //
  // Stored as *which* entry was asked about rather than a bare flag: a flag
  // set on one entry stays set as you step, so every later entry would fetch
  // its largest payload off the back of a single click on the first.
  const [rawFor, setRawFor] = useState<string | undefined>(undefined);
  const withRaw = rawFor !== undefined && rawFor === entryKey;
  const query = useCacheEntry(entryKey, { raw: withRaw });
  const compact = (size ?? 0) > SMALL_ENTRY_BYTES;

  return (
    <DetailDrawer
      open={!!entryKey}
      item={query.data}
      error={query.isError ? query.error : undefined}
      onRetry={() => query.refetch()}
      onClose={() => {
        setRawFor(undefined);
        onClose();
      }}
      onPrev={onPrev}
      onNext={onNext}
      position={position}
      navItemLabel="cache entry"
      // Both from the shown entry, falling back to the selection so a cold open
      // and a failure still name the hash that is already in the URL. Keyed off
      // the entry once it lands, so "Copy key" can never hand back the hash of
      // a row that is still loading behind this one.
      renderHeaderActions={(entry) => {
        const key = entry?.key ?? entryKey;
        return key ? <CopyButton value={key} label="Copy key" /> : null;
      }}
      renderEyebrow={(entry) => {
        const key = entry?.key ?? entryKey;
        return key ? <span title={key}>sha256:{shortCacheKey(key)}</span> : null;
      }}
      renderTitle={(entry) => entry.model ?? "Cache entry"}
      renderBody={(entry) => (
        <div className="flex-1 space-y-5 overflow-y-auto px-5 py-4">
          {/* Keyed by entry: the sections below decide `defaultOpen` from the
              row's size, and `Used by runs` only queries once opened. Reusing
              the subtree across a step carries both decisions onto an entry
              they were not made for — the compact guard stops protecting the
              4 MiB entry you just stepped onto, and its runs lookup fires
              without anyone asking. */}
          <EntryBody
            key={entry.key}
            entry={entry}
            compact={compact}
            withRaw={withRaw}
            rawPending={withRaw && query.isFetching}
            onRequestRaw={() => setRawFor(entryKey)}
          />
        </div>
      )}
    />
  );
}

function EntryBody({
  entry,
  compact,
  withRaw,
  rawPending,
  onRequestRaw,
}: {
  entry: CacheEntryDetail;
  compact: boolean;
  withRaw: boolean;
  /** The raw re-fetch is in flight, so `entry.raw` is still the lean response. */
  rawPending: boolean;
  onRequestRaw: () => void;
}) {
  const tokens =
    entry.input_tokens === null && entry.output_tokens === null
      ? null
      : (entry.input_tokens ?? 0) + (entry.output_tokens ?? 0);

  return (
    <>
      <div className="flex flex-wrap items-start gap-x-6 gap-y-3">
        <StatBlock label="Size" variant="bare">
          {formatBytes(entry.size)}
        </StatBlock>
        <StatBlock
          label="Tokens"
          variant="bare"
          sub={
            tokens === null
              ? undefined
              : `${formatTokens(entry.input_tokens)} in · ${formatTokens(entry.output_tokens)} out`
          }
        >
          {tokens === null ? "–" : formatTokens(tokens)}
        </StatBlock>
        <StatBlock label="Cost" variant="bare">
          {entry.cost_usd === null ? "–" : formatCost(entry.cost_usd)}
        </StatBlock>
        <StatBlock label="Latency" variant="bare">
          {formatLatency(entry.provider_latency_ms)}
        </StatBlock>
        <StatBlock label="Created" variant="bare">
          {formatRelative(entry.entry_created_at ?? entry.created_at)}
        </StatBlock>
        <StatBlock label="Last used" variant="bare">
          {formatRelative(entry.last_access_at)}
        </StatBlock>
      </div>

      <div className="flex flex-wrap items-center gap-2">
        {entry.kind ? (
          <Chip tone="neutral" size="xs" mono>
            {entry.kind}
          </Chip>
        ) : null}
        {entry.stop_reason ? (
          <Chip tone="neutral" size="xs" mono>
            {entry.stop_reason}
          </Chip>
        ) : null}
        {/* `amber`, not `neutral`: this entry has no gradeable output, and it
            is replayed on every run until someone evicts it. The output panel
            below is blank, so without this the drawer would show nothing and
            explain nothing. */}
        {entry.empty_reason ? (
          <Chip tone="amber" size="xs" mono>
            empty: {entry.empty_reason}
          </Chip>
        ) : null}
        {entry.attempts !== null ? (
          <Chip tone="neutral" size="xs">
            {entry.attempts} {entry.attempts === 1 ? "attempt" : "attempts"}
          </Chip>
        ) : null}
        {entry.domarinn_version ? (
          <Chip tone="neutral" size="xs" mono>
            v{entry.domarinn_version}
          </Chip>
        ) : null}
      </div>

      {entry.parseable === false ? <OpaqueNotice /> : null}

      {entry.output !== null ? (
        <CollapsibleSection
          title="Output"
          defaultOpen={!compact}
          meta={compact ? "large — expand to load" : undefined}
        >
          <OutputViewer value={entry.output} maxHeight="36rem" />
        </CollapsibleSection>
      ) : null}

      <CollapsibleSection title="Request" defaultOpen={!compact}>
        {entry.request !== null ? (
          <Payload value={entry.request} />
        ) : entry.provider_fingerprint !== null ? (
          <PreRequestNotice fingerprint={entry.provider_fingerprint} />
        ) : (
          <p className="text-sm text-muted">
            This entry records neither a request nor a provider fingerprint.
          </p>
        )}
      </CollapsibleSection>

      {entry.reasoning ? (
        <CollapsibleSection title="Reasoning" defaultOpen={false}>
          <OutputViewer value={entry.reasoning} maxHeight="24rem" />
        </CollapsibleSection>
      ) : null}

      {entry.tool_calls.length > 0 ? (
        <CollapsibleSection
          title="Tool calls"
          meta={String(entry.tool_calls.length)}
        >
          <Payload value={entry.tool_calls} />
        </CollapsibleSection>
      ) : null}

      <UsedByRuns entryKey={entry.key} />

      <CollapsibleSection title="Provider metadata" defaultOpen={false}>
        {withRaw ? (
          // Until the raw re-fetch lands, `entry` is still the lean response,
          // whose `raw` is null because it was withheld — not because the
          // entry has none. Saying so would be a wrong answer to the question
          // the user just asked.
          rawPending ? (
            <CenteredSpinner label="Loading metadata…" />
          ) : entry.raw === null ? (
            <p className="text-sm text-muted">This entry recorded no metadata.</p>
          ) : (
            <Payload value={entry.raw} />
          )
        ) : (
          <button
            type="button"
            onClick={onRequestRaw}
            className="text-sm font-medium text-accent hover:underline"
          >
            Load raw metadata
          </button>
        )}
      </CollapsibleSection>

      <CollapsibleSection
        title="Entry JSON"
        defaultOpen={false}
        actions={
          <CopyButton value={JSON.stringify(entry, null, 2)} label="Copy JSON" />
        }
      >
        <Payload value={entry} />
      </CollapsibleSection>
    </>
  );
}

/**
 * Which runs used this entry.
 *
 * Controlled rather than `defaultOpen`, so the query only fires when someone
 * asks: it is the one section whose answer costs a lookup against the runs
 * database, and most drawer opens never expand it.
 */
function UsedByRuns({ entryKey }: { entryKey?: string }) {
  const [open, setOpen] = useState(false);
  const runs = useCacheEntryRuns(entryKey, { enabled: open });
  const cases = runs.data?.cases ?? [];

  return (
    <CollapsibleSection
      title="Used by runs"
      open={open}
      onOpenChange={setOpen}
      meta={runs.data ? String(cases.length) : undefined}
    >
      {runs.isPending ? (
        <CenteredSpinner label="Looking up runs…" />
      ) : runs.isError ? (
        <ErrorState error={runs.error} onRetry={() => runs.refetch()} />
      ) : cases.length === 0 ? (
        // Not "this entry is unused". A run only carries the key if it was
        // recorded by a version that wrote one, and older runs cannot be
        // backfilled — the key is derived from ingredients a stored run
        // document does not contain.
        <p className="text-sm text-muted">
          No run on this server records having used this entry. Runs recorded
          before cache keys were tracked cannot be linked, so this is not
          evidence that the entry is unused.
        </p>
      ) : (
        <ul className="space-y-1">
          {cases.map((c) => (
            <li key={`${c.run_id}:${c.case_key}`} className="text-sm">
              <Link
                to={`/runs/${encodeURIComponent(c.run_id)}?case=${encodeURIComponent(c.case_key)}`}
                className="text-accent hover:underline"
              >
                {c.name ?? c.case_key}
              </Link>
              <span className="text-muted">
                {" · "}
                {c.project ?? "—"}/{c.suite ?? "—"} · {formatRelative(c.created_at)}
              </span>
              {c.cached ? (
                <Chip tone="neutral" size="xs" className="ml-2">
                  cached
                </Chip>
              ) : null}
            </li>
          ))}
        </ul>
      )}
    </CollapsibleSection>
  );
}

/** An entry this build of the server cannot read — not an error. */
function OpaqueNotice() {
  return (
    <div className="rounded-lg border border-border bg-surface-2 px-3 py-2 text-sm text-muted">
      This server could not read this entry, so none of its details are
      available here. The stored bytes are unchanged and still served to clients
      that understand them — most often that means the entry was written by a
      newer domarinn.
    </div>
  );
}

/**
 * Entries written before 0.5 recorded what *selected* the provider rather than
 * the request itself. Genuinely either/or: the fingerprint stopped being
 * written once the request was recorded, so nothing carries both.
 */
function PreRequestNotice({ fingerprint }: { fingerprint: unknown }) {
  return (
    <div className="space-y-2">
      <div className="rounded-lg border border-border bg-surface-2 px-3 py-2 text-sm text-muted">
        This entry predates request capture (0.5), so only a fingerprint of the
        provider that answered was stored.
      </div>
      <Payload value={fingerprint} />
    </div>
  );
}
