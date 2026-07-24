import { useMemo, useState } from "react";
import { Link } from "react-router";
import { useCaseDetail, useSuites } from "@/api/queries";
import type { Output } from "@/api";
import { outputToString } from "@/components/output";
import { Spinner } from "@/components/ui/Spinner";
import { CollapsibleSection } from "@/components/ui/CollapsibleSection";
import { SegmentedControl } from "@/components/ui/SegmentedControl";
import { DiffView, type DiffMode } from "@/pages/compare/DiffView";
import { resolveDiffGuard } from "@/pages/compare/diffGuard";
import { shortRunId } from "@/lib/format";

/**
 * Collapsible "Diff vs baseline" section in the run-detail case drawer: diffs
 * THIS case's output against the same case (deterministic `case_key`) in the
 * suite's pinned baseline run, without leaving the drawer.
 *
 * Rendered unconditionally by the drawer so its hooks keep a stable order; it
 * returns `null` (hides itself) when there is no baseline to diff against. The
 * baseline case is fetched only once the section is expanded (see the `enabled`
 * gate on `useCaseDetail`).
 */
export function BaselineDiffSection({
  project,
  suite,
  runId,
  caseKey,
  currentOutput,
}: {
  project: string;
  suite: string;
  runId: string;
  caseKey: string;
  currentOutput: Output;
}) {
  const suites = useSuites(project);
  const baselineRunId =
    suites.data?.suites.find((s) => s.suite === suite)?.baseline_run_id ?? null;

  // Hidden when the suite has no pinned baseline, or when this run *is* the
  // baseline (nothing meaningful to diff against). Computed after all hooks so
  // hook order stays stable across renders.
  const show = !!baselineRunId && baselineRunId !== runId;

  // Expanded (and therefore fetching) by default, like every drawer section.
  const [expanded, setExpanded] = useState(true);
  const [mode, setMode] = useState<DiffMode>("side");

  // Enabled gate: the baseline case is not fetched while the section is
  // tucked away (or hidden). The query key is unchanged, so it shares cache
  // with any other reader of the same (baselineRunId, caseKey).
  const baseline = useCaseDetail(baselineRunId ?? "", caseKey, {
    enabled: expanded && show,
  });

  const baseText = useMemo(
    () => outputToString(baseline.data?.output),
    [baseline.data],
  );
  const headText = useMemo(() => outputToString(currentOutput), [currentOutput]);

  if (!show) return null;

  return (
    <CollapsibleSection
      title="Diff vs baseline"
      meta={
        <span className="font-mono">· {shortRunId(baselineRunId)}</span>
      }
      open={expanded}
      onOpenChange={setExpanded}
    >
      <div className="space-y-3">
        {baseline.isPending ? (
          <div className="flex items-center gap-2 p-3 text-xs text-muted">
            <Spinner /> Loading baseline…
          </div>
        ) : baseline.isError ? (
          <p className="text-sm text-muted">
            This case does not exist in the baseline run.
          </p>
        ) : baseText === headText ? (
          <p className="text-sm text-muted">Output identical to baseline.</p>
        ) : (
          <BaselineDiff
            base={baseText}
            head={headText}
            mode={mode}
            onModeChange={setMode}
          />
        )}

        <div className="pt-0.5">
          <Link
            to={`/runs/${encodeURIComponent(baselineRunId)}/compare/${encodeURIComponent(
              runId,
            )}?case=${encodeURIComponent(caseKey)}`}
            className="text-xs font-medium text-accent hover:underline"
          >
            Open full compare →
          </Link>
        </div>
      </div>
    </CollapsibleSection>
  );
}

/** The diff-mode control + `DiffView`, sharing the compare view's 50k perf
 *  guard (large outputs force the unified line diff). */
function BaselineDiff({
  base,
  head,
  mode,
  onModeChange,
}: {
  base: string;
  head: string;
  mode: DiffMode;
  onModeChange: (mode: DiffMode) => void;
}) {
  const { oversized, effectiveMode, options } = resolveDiffGuard(base, head, mode);
  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <SegmentedControl
          ariaLabel="Diff mode"
          size="xs"
          options={options}
          value={effectiveMode}
          onChange={onModeChange}
        />
        {oversized ? (
          <span className="text-[11px] text-muted">large output — unified diff</span>
        ) : null}
      </div>
      <DiffView base={base} head={head} mode={effectiveMode} />
    </div>
  );
}
