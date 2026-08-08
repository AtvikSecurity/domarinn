import { useMemo, useState } from "react";
import { useRunConfig } from "@/api/queries";
import { Chip, type ChipTone } from "@/components/ui/Chip";
import { CopyButton } from "@/components/ui/CopyButton";
import {
  CHROME_FRAME,
  OUTLINE_LABEL_BASE,
  OUTLINE_LABEL_TONE,
} from "@/components/ui/chrome";
import { SegmentedControl } from "@/components/ui/SegmentedControl";
import { Spinner } from "@/components/ui/Spinner";
import { ErrorState } from "@/components/States";
import { cn } from "@/lib/cn";
import {
  formatLeaf,
  isPromptPath,
  jsonDiff,
  type JsonDiffEntry,
} from "@/lib/jsonDiff";
import { DiffView } from "./DiffView";

type View = "structured" | "raw";

const VIEW_OPTIONS = [
  { value: "structured" as const, label: "Structured" },
  { value: "raw" as const, label: "Raw" },
];

/** Short form of a `prefix:hex` digest for inline display: `blake3:abab…`. */
function shortDigest(digest: string | null): string {
  if (!digest) return "—";
  const sep = digest.indexOf(":");
  const prefix = sep >= 0 ? digest.slice(0, sep) : "";
  const rest = sep >= 0 ? digest.slice(sep + 1) : digest;
  const short = rest.length > 8 ? `${rest.slice(0, 8)}…` : rest;
  return prefix ? `${prefix}:${short}` : short;
}

function prettyConfig(config: unknown): string {
  try {
    return JSON.stringify(config ?? null, null, 2);
  } catch {
    return String(config);
  }
}

/**
 * The config-drift panel: fetches both runs' config snapshots (only mounted
 * when the `?config=1` panel is open) and renders the digest transition, a
 * structured `path → old → new` diff (prompt-path string changes word-diffed
 * inline), and a raw unified diff of the two pretty-printed configs.
 */
export function ConfigDrift({
  baseId,
  headId,
}: {
  baseId: string;
  headId: string;
}) {
  const [view, setView] = useState<View>("structured");
  const base = useRunConfig(baseId, { enabled: true });
  const head = useRunConfig(headId, { enabled: true });

  const baseConfig = base.data?.config;
  const headConfig = head.data?.config;

  const entries = useMemo(
    () =>
      base.data && head.data ? jsonDiff(baseConfig, headConfig) : [],
    [base.data, head.data, baseConfig, headConfig],
  );

  const baseRaw = useMemo(() => prettyConfig(baseConfig), [baseConfig]);
  const headRaw = useMemo(() => prettyConfig(headConfig), [headConfig]);

  return (
    <div
      data-testid="config-drift"
      className={cn(CHROME_FRAME, "space-y-3 p-4")}
    >
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="font-mono text-[10px] font-medium uppercase tracking-[0.12em] text-muted">
          Config drift
        </h2>
        <SegmentedControl
          ariaLabel="Config diff view"
          size="xs"
          options={VIEW_OPTIONS}
          value={view}
          onChange={setView}
        />
      </div>

      {/* Digest transition line. */}
      <div className="flex flex-wrap items-center gap-2 text-xs">
        <DigestChip label="Base" digest={base.data?.config_digest ?? null} />
        <span aria-hidden className="text-muted">
          →
        </span>
        <DigestChip label="Head" digest={head.data?.config_digest ?? null} />
      </div>

      {base.isPending || head.isPending ? (
        <div className="flex items-center gap-2 p-3 text-xs text-muted">
          <Spinner /> Loading configs…
        </div>
      ) : base.isError ? (
        <ErrorState error={base.error} onRetry={() => base.refetch()} />
      ) : head.isError ? (
        <ErrorState error={head.error} onRetry={() => head.refetch()} />
      ) : view === "raw" ? (
        <DiffView base={baseRaw} head={headRaw} mode="lines" />
      ) : entries.length === 0 ? (
        <p className="p-2 text-xs text-muted">No differences.</p>
      ) : (
        <StructuredDiff entries={entries} />
      )}
    </div>
  );
}

function DigestChip({
  label,
  digest,
}: {
  label: string;
  digest: string | null;
}) {
  return (
    <span
      className={cn(
        OUTLINE_LABEL_BASE,
        OUTLINE_LABEL_TONE.neutral,
        "px-2 py-0.5 normal-case",
      )}
    >
      <span className="text-[10px] font-medium uppercase tracking-wide text-muted">
        {label}
      </span>
      <span className="font-mono text-[11px] text-fg">{shortDigest(digest)}</span>
      {digest ? <CopyButton value={digest} label={`Copy ${label} digest`} iconOnly /> : null}
    </span>
  );
}

const KIND_TONE: Record<JsonDiffEntry["kind"], ChipTone> = {
  added: "pass",
  removed: "fail",
  changed: "amber",
};

function StructuredDiff({ entries }: { entries: JsonDiffEntry[] }) {
  return (
    <ul className="divide-y divide-border/60 overflow-hidden rounded-lg border border-border">
      {entries.map((e) => (
        <li key={e.path} className="px-3 py-2">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-mono text-xs text-fg break-all">{e.path}</span>
            {isPromptPath(e.path) ? (
              <Chip tone="accent" size="xs">
                prompt
              </Chip>
            ) : null}
            <Chip tone={KIND_TONE[e.kind]} size="xs">
              {e.kind}
            </Chip>
          </div>
          <div className="mt-1">
            <ValueChange entry={e} />
          </div>
        </li>
      ))}
    </ul>
  );
}

/** The old→new rendering for one diff entry. A changed prompt-path string is
 *  word-diffed inline; everything else is a plain `old → new` (with the absent
 *  side suppressed for adds/removes). */
function ValueChange({ entry }: { entry: JsonDiffEntry }) {
  const { kind, before, after, path } = entry;

  if (
    kind === "changed" &&
    isPromptPath(path) &&
    typeof before === "string" &&
    typeof after === "string"
  ) {
    return <DiffView base={before} head={after} mode="inline" />;
  }

  if (kind === "added") {
    return <ValuePill tone="pass" value={formatLeaf(after)} />;
  }
  if (kind === "removed") {
    return <ValuePill tone="fail" value={formatLeaf(before)} />;
  }
  return (
    <div className="flex flex-wrap items-center gap-2">
      <ValuePill tone="fail" value={formatLeaf(before)} />
      <span aria-hidden className="text-muted">
        →
      </span>
      <ValuePill tone="pass" value={formatLeaf(after)} />
    </div>
  );
}

function ValuePill({ tone, value }: { tone: "pass" | "fail"; value: string }) {
  return (
    <Chip tone={tone} className="break-all normal-case">
      {value}
    </Chip>
  );
}
