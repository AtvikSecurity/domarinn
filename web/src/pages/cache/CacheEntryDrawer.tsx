import { useState } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import type { CacheEntryDetail } from "@/api";
import { useCacheEntry } from "@/api/queries";
import { JsonTree, OutputViewer, RawText } from "@/components/output";
import { ErrorState } from "@/components/States";
import { Chip } from "@/components/ui/Chip";
import { CollapsibleSection } from "@/components/ui/CollapsibleSection";
import { CopyButton } from "@/components/ui/CopyButton";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { StatBlock } from "@/components/ui/StatBlock";
import { DrawerResizer, useDrawerWidth } from "@/pages/run-detail/DrawerResizer";
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
}

export function CacheEntryDrawer({ entryKey, size, onClose }: CacheEntryDrawerProps) {
  const drawer = useDrawerWidth();
  // `raw` is the largest member of an entry and the least often wanted, so the
  // server withholds it until asked. Asking re-fetches under a different key.
  const [withRaw, setWithRaw] = useState(false);
  const query = useCacheEntry(entryKey, { raw: withRaw });
  const open = !!entryKey;
  const entry = query.data;
  const compact = (size ?? 0) > SMALL_ENTRY_BYTES;

  return (
    <Dialog.Root
      open={open}
      onOpenChange={(o) => {
        if (!o) {
          setWithRaw(false);
          onClose();
        }
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/40 backdrop-blur-[1px] data-[state=open]:animate-[overlay-in_120ms_ease-out]" />
        <Dialog.Content
          className="fixed inset-y-0 right-0 z-50 flex max-w-full flex-col border-l border-border bg-surface shadow-2xl outline-none data-[state=open]:animate-[drawer-in_160ms_ease-out]"
          style={{ width: drawer.width }}
          aria-describedby={undefined}
          onOpenAutoFocus={(e) => e.preventDefault()}
        >
          <DrawerResizer
            width={drawer.width}
            onResize={drawer.set}
            onToggle={drawer.toggle}
          />

          <div className="flex shrink-0 items-start justify-between gap-3 border-b border-border px-5 py-3">
            <div className="min-w-0">
              {/* Never `undefined` while pending: the title is the dialog's
                  accessible name, and "undefined" is what a screen reader
                  would otherwise announce. */}
              <Dialog.Title className="truncate text-sm font-semibold">
                {entry?.model ?? "Cache entry"}
              </Dialog.Title>
              {entryKey ? (
                <p className="truncate font-mono text-xs text-muted" title={entryKey}>
                  sha256:{shortCacheKey(entryKey)}
                </p>
              ) : null}
            </div>
            <div className="flex shrink-0 items-center gap-1">
              {entryKey ? <CopyButton value={entryKey} label="Copy key" /> : null}
              <Dialog.Close
                className="rounded-md px-2 py-1 text-sm text-muted hover:bg-surface-2 hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                aria-label="Close"
              >
                ✕
              </Dialog.Close>
            </div>
          </div>

          <div className="flex-1 space-y-5 overflow-y-auto px-5 py-4">
            {query.isPending ? (
              <CenteredSpinner label="Loading entry…" />
            ) : query.isError ? (
              <ErrorState error={query.error} onRetry={() => query.refetch()} />
            ) : entry ? (
              <EntryBody
                entry={entry}
                compact={compact}
                withRaw={withRaw}
                onRequestRaw={() => setWithRaw(true)}
              />
            ) : null}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function EntryBody({
  entry,
  compact,
  withRaw,
  onRequestRaw,
}: {
  entry: CacheEntryDetail;
  compact: boolean;
  withRaw: boolean;
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

      <CollapsibleSection title="Provider metadata" defaultOpen={false}>
        {withRaw ? (
          entry.raw === null ? (
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
