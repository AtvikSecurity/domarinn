import { useMemo } from "react";
import type { AssertFlip } from "@/api";
import { useCaseDetail } from "@/api/queries";
import { Spinner } from "@/components/ui/Spinner";
import { outputToString } from "@/components/output";
import { SegmentedControl } from "@/components/ui/SegmentedControl";
import { DiffView, type DiffMode } from "./DiffView";
import { resolveDiffGuard } from "./diffGuard";
import { AssertTransitions } from "./AssertTransitions";

export function CompareRowExpansion({
  baseRunId,
  headRunId,
  caseKey,
  assertFlips,
  mode,
  onModeChange,
}: {
  baseRunId: string;
  headRunId: string;
  caseKey: string;
  assertFlips: AssertFlip[];
  mode: DiffMode;
  onModeChange: (mode: DiffMode) => void;
}) {
  const base = useCaseDetail(baseRunId, caseKey);
  const head = useCaseDetail(headRunId, caseKey);

  // Memoised so the (potentially large) output stringify doesn't rerun on every
  // render — the expansion re-renders on each scroll frame while it is open.
  const baseText = useMemo(() => outputToString(base.data?.output), [base.data]);
  const headText = useMemo(() => outputToString(head.data?.output), [head.data]);

  const loading = base.isPending || head.isPending;

  if (loading) {
    return (
      <div className="border-t border-border bg-bg/40 px-4 py-3">
        <div className="flex items-center gap-2 p-3 text-xs text-muted">
          <Spinner /> Loading outputs…
        </div>
      </div>
    );
  }

  // Perf guard: very large outputs force the unified line diff and lock out the
  // Side/Inline options (shared with the case-drawer baseline diff).
  const { oversized, effectiveMode, options } = resolveDiffGuard(
    baseText,
    headText,
    mode,
  );

  return (
    <div className="space-y-3 border-t border-border bg-bg/40 px-4 py-3">
      <div className="flex flex-wrap items-center gap-2">
        <SegmentedControl
          ariaLabel="Diff mode"
          size="xs"
          options={options}
          value={effectiveMode}
          onChange={onModeChange}
        />
        {oversized ? (
          <span className="text-[11px] text-muted">
            large output — unified diff
          </span>
        ) : null}
      </div>

      <AssertTransitions flips={assertFlips} />

      <DiffView base={baseText} head={headText} mode={effectiveMode} />
    </div>
  );
}
