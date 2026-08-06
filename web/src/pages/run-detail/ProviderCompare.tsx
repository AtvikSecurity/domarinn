import { useCallback, useEffect, useMemo, useState } from "react";
import type { MatrixCell, MatrixColumn, MatrixRow } from "@/api";
import { Modal } from "@/components/ui/Modal";
import { Button } from "@/components/ui/Button";
import { SegmentedControl } from "@/components/ui/SegmentedControl";
import { OutputViewer, outputToString } from "@/components/output";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { ErrorState } from "@/components/States";
import { useCaseDetail } from "@/api/queries";
import { distinctProviders } from "@/lib/matrix";
import { DiffView } from "../compare/DiffView";
import { resolveDiffGuard } from "../compare/diffGuard";
import { cn } from "@/lib/cn";

/** What each provider panel reports back so the parent can decide whether a
 *  two-way diff is offered and diff the two loaded outputs directly. */
interface PanelState {
  loaded: boolean;
  hasOutput: boolean;
  text: string;
}

/**
 * Cross-provider side-by-side output comparison for one test (within one prompt
 * when the run has >1 prompt). One panel per provider that ran the test:
 * provider name, a per-repeat status strip, and the first repeat's output. When
 * exactly two providers produced output, a Diff toggle collapses the panels into
 * a single word diff (honouring the large-output guard). Header ‹ › navigate the
 * matrix rows in the same prompt context.
 */
export function ProviderCompare({
  open,
  onOpenChange,
  runId,
  columns,
  rows,
  testId,
  promptId,
  showPrompt,
  onNavigate,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  runId: string;
  columns: MatrixColumn[];
  rows: MatrixRow[];
  testId: string | null;
  promptId: string | null;
  showPrompt: boolean;
  onNavigate: (testId: string) => void;
}) {
  // Providers are stable for the run's lifetime, so this list — and therefore the
  // number of child panels (and their hooks) — never changes between renders.
  const providers = useMemo(
    () =>
      distinctProviders({
        run_id: runId,
        columns,
        rows: [],
        provider_costs: [],
        next_cursor: null,
      }),
    [runId, columns],
  );

  const rowIndex = rows.findIndex((r) => r.test_id === testId);
  const row = rowIndex >= 0 ? rows[rowIndex] : undefined;
  const testLabel = row?.name ?? testId ?? "";

  // For the current (test, prompt), the cell + first-repeat key per provider.
  const perProvider = useMemo(
    () =>
      providers.map((provider) => {
        const colIndex = columns.findIndex(
          (c) => c.provider_id === provider && c.prompt_id === promptId,
        );
        const cell: MatrixCell | null =
          colIndex >= 0 ? (row?.cells[colIndex] ?? null) : null;
        return { provider, cell, firstKey: cell?.case_keys[0] };
      }),
    [providers, columns, promptId, row],
  );

  // Output presence per provider, reported up from the panels. Reset whenever the
  // (test, prompt) context changes so a stale diff can't survive navigation.
  const [outputs, setOutputs] = useState<Record<string, PanelState>>({});
  const [diffMode, setDiffMode] = useState(false);
  const ctxKey = `${testId}|${promptId}`;
  const [prevCtx, setPrevCtx] = useState(ctxKey);
  if (prevCtx !== ctxKey) {
    setPrevCtx(ctxKey);
    setOutputs({});
    setDiffMode(false);
  }

  const handleOutput = useCallback((provider: string, state: PanelState) => {
    setOutputs((prev) => {
      const cur = prev[provider];
      if (
        cur &&
        cur.loaded === state.loaded &&
        cur.hasOutput === state.hasOutput &&
        cur.text === state.text
      ) {
        return prev;
      }
      return { ...prev, [provider]: state };
    });
  }, []);

  const panelProviders = perProvider.filter((p) => p.cell);
  const allLoaded =
    panelProviders.length > 0 &&
    panelProviders.every((p) => outputs[p.provider]?.loaded);
  const withOutput = panelProviders.filter((p) => outputs[p.provider]?.hasOutput);
  const canDiff = allLoaded && withOutput.length === 2;
  const showDiff = diffMode && canDiff;

  // Prev/next over the matrix rows, keeping the prompt context.
  const prevTest = rowIndex > 0 ? rows[rowIndex - 1] : undefined;
  const nextTest = rowIndex >= 0 && rowIndex < rows.length - 1 ? rows[rowIndex + 1] : undefined;

  const title = showPrompt && promptId != null ? `${testLabel} · ${promptId}` : testLabel;

  return (
    // `xl`: three provider panels side by side in a 38rem dialog left ~190px
    // each, which the OutputViewer toolbar alone wraps onto three lines.
    <Modal open={open} onOpenChange={onOpenChange} title={title} size="xl">
      <div className="space-y-4">
        <div className="flex flex-wrap items-center gap-2">
          <div className="flex items-center gap-1">
            <Button
              size="sm"
              variant="ghost"
              aria-label="Previous test"
              disabled={!prevTest}
              onClick={() => prevTest && onNavigate(prevTest.test_id)}
            >
              ‹
            </Button>
            <Button
              size="sm"
              variant="ghost"
              aria-label="Next test"
              disabled={!nextTest}
              onClick={() => nextTest && onNavigate(nextTest.test_id)}
            >
              ›
            </Button>
          </div>
          {canDiff ? (
            <SegmentedControl
              ariaLabel="Compare view"
              options={[
                { value: "panels", label: "Panels" },
                { value: "diff", label: "Diff" },
              ]}
              value={showDiff ? "diff" : "panels"}
              onChange={(v) => setDiffMode(v === "diff")}
            />
          ) : null}
          <span className="ml-auto text-xs text-muted">
            {panelProviders.length} provider{panelProviders.length === 1 ? "" : "s"}
          </span>
        </div>

        {panelProviders.length === 0 ? (
          <p className="text-sm text-muted">This test did not run on any provider.</p>
        ) : showDiff ? (
          <TwoProviderDiff
            base={{ provider: withOutput[0]!.provider, text: outputs[withOutput[0]!.provider]!.text }}
            head={{ provider: withOutput[1]!.provider, text: outputs[withOutput[1]!.provider]!.text }}
          />
        ) : (
          <div
            className={cn(
              "grid gap-3",
              panelProviders.length >= 3
                ? "md:grid-cols-3"
                : panelProviders.length === 2
                  ? "md:grid-cols-2"
                  : "grid-cols-1",
            )}
          >
            {panelProviders.map((p) => (
              <ProviderPanel
                key={p.provider}
                runId={runId}
                provider={p.provider}
                cell={p.cell!}
                caseKey={p.firstKey!}
                open={open}
                onOutput={handleOutput}
              />
            ))}
          </div>
        )}
      </div>
    </Modal>
  );
}

/** One provider's column: a per-repeat status strip (from the cell aggregate)
 *  and the first repeat's full output. Reports its loaded output text up so the
 *  parent can offer a two-way diff. */
function ProviderPanel({
  runId,
  provider,
  cell,
  caseKey,
  open,
  onOutput,
}: {
  runId: string;
  provider: string;
  cell: MatrixCell;
  caseKey: string;
  open: boolean;
  onOutput: (provider: string, state: PanelState) => void;
}) {
  const detail = useCaseDetail(runId, caseKey, { enabled: open });
  const text = detail.data ? outputToString(detail.data.output) : "";
  const settled = !detail.isPending;
  const isError = detail.isError;

  // Report presence up to the parent once the query settles (or errors). Done in
  // an effect — never during render — because it updates a *different* component's
  // state; `onOutput` de-dupes so this can't loop.
  useEffect(() => {
    if (!open) return;
    onOutput(provider, {
      loaded: settled || isError,
      hasOutput: !isError && text.trim() !== "",
      text,
    });
  }, [open, provider, settled, isError, text, onOutput]);

  return (
    <div className="rounded-lg border border-border bg-bg/40 p-3">
      <div className="mb-1.5 flex items-center justify-between gap-2">
        <span className="truncate font-mono text-xs font-semibold" title={provider}>
          {provider}
        </span>
        <RepeatDots cell={cell} />
      </div>
      {detail.isPending ? (
        <CenteredSpinner label="Loading…" />
      ) : detail.isError ? (
        <ErrorState error={detail.error} onRetry={() => detail.refetch()} />
      ) : text.trim() === "" ? (
        <p className="text-xs text-muted">No output (skipped).</p>
      ) : (
        <OutputViewer value={detail.data?.output} maxHeight="18rem" />
      )}
    </div>
  );
}

/** A row of `total` status dots derived from the cell's aggregate counts —
 *  a pass@k glance, one dot per repeat. */
function RepeatDots({ cell }: { cell: MatrixCell }) {
  const dots: string[] = [
    ...Array<string>(cell.passed).fill("bg-pass"),
    ...Array<string>(cell.failed).fill("bg-fail"),
    ...Array<string>(cell.errored).fill("bg-error"),
    ...Array<string>(cell.skipped).fill("bg-skip"),
  ];
  return (
    <span
      className="flex shrink-0 items-center gap-1"
      title={`${cell.passed}/${cell.total} passed`}
      aria-label={`${cell.passed} of ${cell.total} passed`}
    >
      {dots.map((c, i) => (
        <span key={i} className={cn("size-2 rounded-full", c)} aria-hidden />
      ))}
    </span>
  );
}

/** The two-provider word diff shown when exactly two providers produced output.
 *  Honours the large-output guard (forces the unified line diff when oversized). */
function TwoProviderDiff({
  base,
  head,
}: {
  base: { provider: string; text: string };
  head: { provider: string; text: string };
}) {
  const guard = resolveDiffGuard(base.text, head.text, "side");
  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2 text-xs font-medium text-muted">
        <span className="font-mono text-fg">{base.provider}</span>
        <span aria-hidden>→</span>
        <span className="font-mono text-fg">{head.provider}</span>
      </div>
      <DiffView base={base.text} head={head.text} mode={guard.effectiveMode} />
    </div>
  );
}
