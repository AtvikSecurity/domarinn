import { useEffect, useMemo, useRef } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useCompare, useRun, useRuns } from "@/api/queries";
import type {
  CaseStatus,
  CompareCaseRow,
  CompareSummary,
  RunTotals,
} from "@/api";
import { DELTA_LABEL, formatScoreDelta } from "@/lib/compare";
import { isFullyCached } from "@/lib/cached";
import { mergeParams } from "@/lib/filters";
import { StatusBadge } from "@/components/StatusBadge";
import { Card } from "@/components/ui/Card";
import { Chip } from "@/components/ui/Chip";
import { PillButton } from "@/components/ui/PillButton";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { ErrorState, EmptyState } from "@/components/States";
import { CHROME_FRAME, type OutlineTone } from "@/components/ui/chrome";
import { cn } from "@/lib/cn";
import { ColumnPicker } from "@/components/ui/ColumnPicker";
import { ColumnResizer } from "@/components/ui/ColumnResizer";
import {
  type ColumnDef,
  effectiveWidth,
  gridTemplateFor,
  minWidthFor,
  visibleColumns,
} from "@/lib/tableColumns";
import {
  resetColumnWidth,
  resetColumns,
  setColumnVisible,
  setColumnWidth,
  useColumnPrefs,
} from "@/lib/useColumnPrefs";
import { CompareRowExpansion } from "./compare/CompareRowExpansion";
import { AggregateDeltas, type AggregateStatsInput } from "./compare/AggregateDeltas";
import { McNemarPanel } from "./compare/McNemarPanel";
import { ConfigDrift } from "./compare/ConfigDrift";
import { changedComponents, componentLabel } from "@/lib/digests";
import type { DiffMode } from "./compare/DiffView";

type ChipKey =
  | "newly_failing"
  | "newly_passing"
  | "still_failing"
  | "output_changed"
  | "added"
  | "removed";

const CHIP_META: { key: ChipKey; label: string; tone: OutlineTone }[] = [
  { key: "newly_failing", label: "Newly failing", tone: "fail" },
  { key: "newly_passing", label: "Newly passing", tone: "pass" },
  { key: "output_changed", label: "Output changed", tone: "amber" },
  { key: "still_failing", label: "Still failing", tone: "error" },
  { key: "added", label: "Added", tone: "neutral" },
  { key: "removed", label: "Removed", tone: "neutral" },
];

/** `?diff=` is a client-only URL param (never sent to the compare endpoint);
 *  absent/unknown ⇒ the default side-by-side mode. */
function parseDiffMode(value: string | null): DiffMode {
  return value === "inline" || value === "lines" ? value : "side";
}

export function ComparePage() {
  const { id = "", other } = useParams();
  const [params, setParams] = useSearchParams();
  const navigate = useNavigate();
  const activeDelta = params.get("delta") as ChipKey | null;
  const diffMode = parseDiffMode(params.get("diff"));
  // `?case=` (the expanded row) is client-only too — same convention as the
  // run-detail drawer, lifted here so a row expansion is shareable/deep-linkable.
  const expandedCase = params.get("case");
  // `?config=1` (client-only) opens the config-drift panel; the header chip
  // toggles it and it is deep-linkable.
  const configOpen = params.get("config") === "1";

  const compare = useCompare(id, other);
  // The real CompareResponse carries `base`/`head` as plain run ids, not
  // embedded run rows (see generated CompareResponse.ts) — fetch each run's
  // row via the existing `useRun` hook once the compare response resolves.
  const baseRun = useRun(compare.data?.base ?? "", { enabled: !!compare.data });
  const headRun = useRun(compare.data?.head ?? "", { enabled: !!compare.data });
  // `cached: "all"` explicitly: these rows fill the base/head pickers, and a
  // run missing from a picker is indistinguishable from a run that does not
  // exist. Comparing against a fully-cached run is a legitimate thing to want
  // — that is how you check a config change against a known-identical
  // baseline. Cached options are labelled rather than withheld. This used to
  // inherit the hidden default by passing no `cached` at all.
  const suiteRuns = useRuns({
    project: headRun.data?.project ?? undefined,
    suite: headRun.data?.suite ?? undefined,
    cached: "all",
  });

  const rows = useMemo(() => {
    const all = compare.data?.cases ?? [];
    if (!activeDelta) return all;
    if (activeDelta === "output_changed") return all.filter((r) => r.output_changed);
    return all.filter((r) => r.delta === activeDelta);
  }, [compare.data, activeDelta]);

  function setDelta(key: ChipKey) {
    setParams(
      mergeParams(params, { delta: activeDelta === key ? undefined : key }),
      { replace: true },
    );
  }

  function setDiffMode(mode: DiffMode) {
    // Default mode drops the param entirely so a shared URL stays clean.
    setParams(mergeParams(params, { diff: mode === "side" ? undefined : mode }), {
      replace: true,
    });
  }

  function toggleCase(caseKey: string) {
    setParams(
      mergeParams(params, {
        case: expandedCase === caseKey ? undefined : caseKey,
      }),
      { replace: true },
    );
  }

  function toggleConfig() {
    setParams(mergeParams(params, { config: configOpen ? undefined : "1" }), {
      replace: true,
    });
  }

  if (compare.isPending) return <CenteredSpinner label="Computing comparison…" />;
  if (compare.isError)
    return <ErrorState error={compare.error} onRetry={() => compare.refetch()} />;
  if (baseRun.isPending || headRun.isPending) {
    return <CenteredSpinner label="Loading runs…" />;
  }
  if (baseRun.isError)
    return <ErrorState error={baseRun.error} onRetry={() => baseRun.refetch()} />;
  if (headRun.isError)
    return <ErrorState error={headRun.error} onRetry={() => headRun.refetch()} />;

  const { summary, stats, totals, config } = compare.data;
  const base = baseRun.data;
  const head = headRun.data;
  const runOptions = suiteRuns.data?.pages.flatMap((p) => p.runs) ?? [];
  // A native <option> renders text and nothing else, so the cached marker that
  // is a Chip everywhere else has to be part of the label here. Marking beats
  // omitting: a run absent from the picker reads as a run that never happened.
  const runOptionLabel = (r: (typeof runOptions)[number]) =>
    isFullyCached(r) ? `${r.id} · cached` : r.id;
  const configChanged = config.changed === true;
  // Name what moved when the component digests can say; fall back to the
  // generic label when one side predates them.
  const changed = changedComponents(config.components);
  const changedLabel =
    changed.length > 0
      ? `${changed.map(componentLabel).join(" + ")} changed`
      : "Config changed";

  return (
    <div className="space-y-5">
      <Card>
        <div className="flex flex-wrap items-center gap-3">
          <Link to={`/runs/${encodeURIComponent(id)}`} className="text-sm text-muted hover:text-fg">
            ← Back to run
          </Link>
          <h1 className="text-lg font-semibold tracking-tight">Compare</h1>
        </div>
        <div className="mt-2 flex flex-wrap items-center gap-2 text-sm">
          <span className="text-muted">base</span>
          <select
            aria-label="Base run"
            className="h-8 rounded-md border border-border bg-surface px-2 font-mono text-xs outline-none focus:ring-2 focus:ring-ring"
            value={base.id}
            onChange={(e) =>
              // First url segment = base (see `id`/`other` above), so the new
              // base value goes first and the (unchanged) head stays second.
              navigate(
                `/runs/${encodeURIComponent(e.target.value)}/compare/${encodeURIComponent(head.id)}`,
              )
            }
          >
            {runOptions.length === 0 ? (
              <option value={base.id}>{base.id}</option>
            ) : (
              runOptions.map((r) => (
                <option key={r.id} value={r.id} disabled={r.id === head.id}>
                  {runOptionLabel(r)}
                </option>
              ))
            )}
          </select>
          <span className="text-muted">→ head</span>
          <select
            aria-label="Head run"
            className="h-8 rounded-md border border-border bg-surface px-2 font-mono text-xs outline-none focus:ring-2 focus:ring-ring"
            value={head.id}
            onChange={(e) =>
              // Second url segment = head, so the (unchanged) base stays
              // first and the new head value goes second.
              navigate(
                `/runs/${encodeURIComponent(base.id)}/compare/${encodeURIComponent(e.target.value)}`,
              )
            }
          >
            {runOptions.length === 0 ? (
              <option value={head.id}>{head.id}</option>
            ) : (
              runOptions.map((r) => (
                <option key={r.id} value={r.id} disabled={r.id === base.id}>
                  {runOptionLabel(r)}
                </option>
              ))
            )}
          </select>
          {base.id === head.id ? (
            <span className="text-xs text-amber">
              Pick two different runs to compare.
            </span>
          ) : null}
          {configChanged ? (
            <PillButton
              tone="amber"
              pressed={configOpen}
              onClick={toggleConfig}
              className="gap-1.5"
            >
              <svg
                width="12"
                height="12"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2"
                strokeLinecap="round"
                strokeLinejoin="round"
                aria-hidden
              >
                <path d="M12 2v4M12 18v4M4.93 4.93l2.83 2.83M16.24 16.24l2.83 2.83M2 12h4M18 12h4M4.93 19.07l2.83-2.83M16.24 7.76l2.83-2.83" />
              </svg>
              {/* Names the components that moved rather than saying "config
                  changed", which is the same answer for a prompt rewrite and a
                  typo in a description. Falls back to the generic label only
                  when the digests cannot say (one side predates them). */}
              {changedLabel}
            </PillButton>
          ) : null}
        </div>
      </Card>

      {configOpen ? <ConfigDrift baseId={base.id} headId={head.id} /> : null}

      {/* Head-vs-base aggregate deltas — server-authoritative, from
          CompareResponse.totals (Task 4) rather than the two run details. */}
      <AggregateDeltas base={totalsToInput(totals.base)} head={totalsToInput(totals.head)} />

      {/* Summary chips (filter the grid) */}
      <div className="flex flex-wrap gap-2">
        {CHIP_META.map((chip) => (
          <PillButton
            key={chip.key}
            tone={chip.tone}
            pressed={activeDelta === chip.key}
            onClick={() => setDelta(chip.key)}
            className={cn(
              "gap-1.5",
              activeDelta && activeDelta !== chip.key && "opacity-40",
            )}
          >
            {chip.label}
            <span className="tabular-nums">{summaryCount(summary, chip.key)}</span>
          </PillButton>
        ))}
        {activeDelta ? (
          <PillButton
            onClick={() => setParams(mergeParams(params, { delta: undefined }), { replace: true })}
          >
            Clear filter
          </PillButton>
        ) : null}
      </div>

      {/* Statistical significance: McNemar + Wilson pass-rate intervals,
          placed below the summary chips. */}
      <McNemarPanel stats={stats} />

      {rows.length === 0 ? (
        <EmptyState title="No cases in this delta group" />
      ) : (
        <DeltaTable
          baseId={base.id}
          headId={head.id}
          rows={rows}
          expandedCase={expandedCase}
          onToggleCase={toggleCase}
          diffMode={diffMode}
          onDiffModeChange={setDiffMode}
        />
      )}
    </div>
  );
}

function summaryCount(summary: CompareSummary, key: ChipKey): number {
  return summary[key];
}

/** Map a server `RunTotals` onto the plain shape `AggregateDeltas` consumes.
 *  `duration_ms` is nullable on the wire (a run may lack timing) — treat a
 *  missing duration as zero so the delta stays well-defined. */
function totalsToInput(t: RunTotals): AggregateStatsInput {
  return {
    prompt_tokens: t.prompt_tokens,
    completion_tokens: t.completion_tokens,
    cost_usd: t.cost_usd,
    duration_ms: t.duration_ms ?? 0,
    case_count: t.case_count,
  };
}

const SCORE_DELTA_TONE: Record<"pass" | "fail" | "muted", string> = {
  pass: "text-pass",
  fail: "text-fail",
  muted: "text-muted",
};

/** The "Δ score" cell: signed 2-dec `score_delta`, pass/fail tinted, muted for
 *  zero, em-dash when a score was missing on either side. */
function ScoreDeltaCell({ delta }: { delta: number | null }) {
  const { text, tone } = formatScoreDelta(delta);
  return (
    <span className={cn("font-mono text-xs tabular-nums", SCORE_DELTA_TONE[tone])}>
      {text}
    </span>
  );
}

function StatusCell({ status }: { status: CaseStatus | null }) {
  if (status === null) return <span className="text-xs text-muted">—</span>;
  return <StatusBadge status={status} size="xs" />;
}

const DELTA_TABLE_ID = "compareDelta";

/**
 * The delta table's columns. Header and rows both derive their
 * `grid-template-columns` from this one list, which is what keeps them
 * aligned — they were previously kept in step by a shared string constant.
 *
 * `case` and `delta` are structural: the row has to say which case it is, and
 * the delta is the entire reason the page exists.
 */
const DELTA_COLUMNS: ColumnDef[] = [
  { id: "case", label: "Case", track: "minmax(240px, 1.5fr)", min: 200, alwaysVisible: true },
  { id: "base", label: "Base", track: "76px", min: 64 },
  { id: "head", label: "Head", track: "76px", min: 64 },
  { id: "delta", label: "Delta", track: "140px", min: 120, alwaysVisible: true },
  { id: "score_delta", label: "Δ score", track: "84px", min: 70 },
  { id: "output", label: "Output", track: "76px", min: 70, numeric: true },
];

const deltaTone: Record<string, string> = {
  newly_failing: "text-fail",
  newly_passing: "text-pass",
  still_failing: "text-error",
  added: "text-muted",
  removed: "text-muted",
  unchanged: "text-muted",
};

function DeltaTable({
  baseId,
  headId,
  rows,
  expandedCase,
  onToggleCase,
  diffMode,
  onDiffModeChange,
}: {
  baseId: string;
  headId: string;
  rows: CompareCaseRow[];
  expandedCase: string | null;
  onToggleCase: (caseKey: string) => void;
  diffMode: DiffMode;
  onDiffModeChange: (mode: DiffMode) => void;
}) {
  const parentRef = useRef<HTMLDivElement>(null);
  const prefs = useColumnPrefs(DELTA_TABLE_ID);
  const shown = useMemo(() => visibleColumns(DELTA_COLUMNS, prefs), [prefs]);
  // Cells are omitted, not blanked: a stray one shifts every column after it
  // out of alignment with the header above.
  const shownIds = useMemo(() => new Set(shown.map((c) => c.id)), [shown]);
  const gridTemplate = useMemo(
    () => gridTemplateFor(DELTA_COLUMNS, prefs),
    [prefs],
  );
  // `px-4` on both sides of the header and every row.
  const minWidth = useMemo(() => minWidthFor(DELTA_COLUMNS, prefs, 32), [prefs]);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 46,
    overscan: 12,
    getItemKey: (i) => rows[i]?.case_key ?? String(i),
  });

  // Deep-link support: scroll the (possibly off-screen, virtualized) expanded
  // row into view exactly once, and ONLY for a `?case=` present at mount. The
  // target is captured at mount so a session that started without a deep link
  // never auto-scrolls — otherwise the first user row-click would recenter the
  // viewport (scrollToIndex scrolls unconditionally in react-virtual v3).
  const deepLinkedCase = useRef(expandedCase);
  const didInitialScroll = useRef(false);
  useEffect(() => {
    if (didInitialScroll.current) return;
    didInitialScroll.current = true;
    const target = deepLinkedCase.current;
    if (!target) return;
    const index = rows.findIndex((r) => r.case_key === target);
    if (index >= 0) virtualizer.scrollToIndex(index, { align: "center" });
  }, [rows, virtualizer]);

  return (
    <div
      data-testid="delta-table"
      className={cn(CHROME_FRAME, "overflow-hidden")}
    >
      <div className="flex justify-end border-b border-border px-4 py-2">
        <ColumnPicker
          columns={DELTA_COLUMNS}
          prefs={prefs}
          onChange={(id, visible) =>
            setColumnVisible(DELTA_TABLE_ID, id, visible)
          }
          onReset={() => resetColumns(DELTA_TABLE_ID)}
        />
      </div>
      {/* One horizontal scroller around the header AND the body. They are
          separate elements — the body owns the vertical scroll the virtualizer
          measures — so a column dragged past the container's width has to move
          both, or the labels drift off the values they name. */}
      <div className="overflow-x-auto">
        <div style={{ minWidth }}>
      <div
        className="grid items-center border-b border-border bg-surface-2/95 px-4 py-2 text-xs font-medium uppercase tracking-wide text-muted"
        style={{ gridTemplateColumns: gridTemplate }}
      >
        {shown.map((c, i, arr) => {
          const labelId = `delta-h-${c.id}`;
          return (
            // `relative` so the handle can straddle this cell's right edge.
            <span key={c.id} className={cn("relative", c.numeric && "text-right")}>
              <span id={labelId}>{c.label}</span>
              <ColumnResizer
                def={c}
                width={effectiveWidth(c, prefs)}
                headerId={labelId}

                edge={i === arr.length - 1}
                onResize={(px) => setColumnWidth(DELTA_TABLE_ID, c.id, px)}
                onReset={() => resetColumnWidth(DELTA_TABLE_ID, c.id)}
              />
            </span>
          );
        })}
      </div>
      <div ref={parentRef} className="max-h-[64vh] overflow-y-auto">
        <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
          {virtualizer.getVirtualItems().map((vi) => {
            const row = rows[vi.index];
            if (!row) return null;
            const isOpen = expandedCase === row.case_key;
            return (
              <div
                key={row.case_key}
                data-index={vi.index}
                ref={virtualizer.measureElement}
                className="absolute left-0 w-full border-b border-border/50"
                style={{ top: 0, transform: `translateY(${vi.start}px)` }}
              >
                <button
                  onClick={() => onToggleCase(row.case_key)}
                  aria-expanded={isOpen}
                  // Names the row for the e2e specs, which previously picked
                  // rows out by `aria-expanded` alone — true only for as long
                  // as nothing else on the page happened to be expandable.
                  data-delta-row=""
                  className={cn(
                    "grid w-full items-center px-4 py-2 text-left text-sm outline-none hover:bg-surface-2 focus-visible:bg-surface-2",
                    isOpen && "bg-surface-2",
                  )}
                  style={{ gridTemplateColumns: gridTemplate }}
                >
                  <span className="flex min-w-0 items-center gap-2">
                    <svg
                      width="14"
                      height="14"
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="2"
                      className={cn("shrink-0 text-muted transition-transform", isOpen && "rotate-90")}
                    >
                      <path d="M9 6l6 6-6 6" />
                    </svg>
                    <span className="min-w-0">
                      <span className="block truncate font-medium">
                        {row.name ?? row.case_key}
                      </span>
                      <span className="block truncate font-mono text-[11px] text-muted">
                        {row.case_key}
                      </span>
                    </span>
                  </span>
                  {shownIds.has("base") && <StatusCell status={row.base_status} />}
                  {shownIds.has("head") && <StatusCell status={row.head_status} />}
                  <span className="flex min-w-0 flex-wrap items-center gap-1.5">
                    <span className={cn("text-xs font-medium", deltaTone[row.delta])}>
                      {DELTA_LABEL[row.delta]}
                    </span>
                    {row.assert_flips.length > 0 ? (
                      <Chip tone="accent" size="xs" className="tabular-nums">
                        {row.assert_flips.length} flips
                      </Chip>
                    ) : null}
                  </span>
                  {shownIds.has("score_delta") && (
                    <ScoreDeltaCell delta={row.score_delta} />
                  )}
                  {shownIds.has("output") && (
                    <span className="text-right">
                      {row.output_changed ? (
                        <Chip tone="amber">changed</Chip>
                      ) : (
                        <span className="text-[11px] text-muted">same</span>
                      )}
                    </span>
                  )}
                </button>
                {isOpen ? (
                  <CompareRowExpansion
                    baseRunId={baseId}
                    headRunId={headId}
                    caseKey={row.case_key}
                    assertFlips={row.assert_flips}
                    mode={diffMode}
                    onModeChange={onDiffModeChange}
                  />
                ) : null}
              </div>
            );
          })}
        </div>
      </div>
        </div>
      </div>
    </div>
  );
}
