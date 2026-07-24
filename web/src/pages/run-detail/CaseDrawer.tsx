import * as Dialog from "@radix-ui/react-dialog";
import { useCaseDetail } from "@/api/queries";
import type { AssertResult, CaseResult } from "@/api";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { CopyButton } from "@/components/ui/CopyButton";
import { CollapsibleSection } from "@/components/ui/CollapsibleSection";
import { ErrorState } from "@/components/States";
import { OutputViewer } from "@/components/output";
import { BaselineDiffSection } from "./BaselineDiffSection";
import { CaseAssertRow } from "./CaseAssertRow";
import { CaseInputSection } from "./CaseInputSection";
import { CaseVerdictStrip } from "./CaseVerdictStrip";
import { RawMetadataSection, reasoningNotice } from "./CaseDrawerSections";

export function CaseDrawer({
  runId,
  project,
  suite,
  caseKey,
  onClose,
}: {
  runId: string;
  project: string;
  suite: string;
  caseKey: string | undefined;
  onClose: () => void;
}) {
  const open = !!caseKey;
  const detail = useCaseDetail(runId, caseKey);

  // Shareable deep link to this exact case (the drawer re-opens from `?case=`).
  const permalink = caseKey
    ? `${window.location.origin}/runs/${encodeURIComponent(runId)}?case=${encodeURIComponent(caseKey)}`
    : "";

  return (
    <Dialog.Root open={open} onOpenChange={(o) => !o && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/40 backdrop-blur-[1px] data-[state=open]:animate-[overlay-in_120ms_ease-out]" />
        <Dialog.Content
          className="fixed inset-y-0 right-0 z-50 flex w-[min(44rem,100vw)] flex-col border-l border-border bg-surface shadow-2xl outline-none data-[state=open]:animate-[drawer-in_160ms_ease-out]"
          aria-describedby={undefined}
        >
          <div className="flex items-start justify-between gap-3 border-b border-border px-5 py-3">
            <div className="min-w-0">
              <Dialog.Title className="truncate text-sm font-semibold">
                {detail.data?.name ?? caseKey}
              </Dialog.Title>
              <div className="truncate font-mono text-xs text-muted">{caseKey}</div>
            </div>
            <div className="flex shrink-0 items-center gap-1">
              {caseKey ? (
                <CopyButton value={permalink} label="Copy link" />
              ) : null}
              <Dialog.Close className="rounded-md p-1.5 text-muted hover:bg-surface-2 hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
                <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
                  <path d="M6 6l12 12M18 6L6 18" />
                </svg>
              </Dialog.Close>
            </div>
          </div>

          {detail.isPending ? (
            <div className="flex-1 overflow-y-auto px-5 py-4">
              <CenteredSpinner label="Loading case…" />
            </div>
          ) : detail.isError ? (
            <div className="flex-1 overflow-y-auto px-5 py-4">
              <ErrorState error={detail.error} onRetry={() => detail.refetch()} />
            </div>
          ) : detail.data && caseKey ? (
            <>
              <CaseVerdictStrip
                detail={detail.data}
                project={project}
                suite={suite}
                runId={runId}
                caseKey={caseKey}
              />

              {/* Body order runs verdict-first: what went wrong, why, and what
                  the model actually said — then the provenance. Prompt and
                  Input are re-derivable from the suite config; the output is
                  the only thing this run alone knows, so it leads. */}
              <div className="flex-1 overflow-y-auto px-5 py-4">
                <div className="space-y-5">
                  {detail.data.error ? (
                    <div className="rounded-lg border border-fail/25 bg-fail/5 p-3">
                      <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-fail">
                        Error
                      </div>
                      <p className="text-sm text-fail">{detail.data.error}</p>
                    </div>
                  ) : null}

                  <CollapsibleSection
                    title="Assertions"
                    meta={assertMeta(detail.data.asserts)}
                  >
                    <div className="space-y-2">
                      {detail.data.asserts.map((a, i) => (
                        <CaseAssertRow
                          key={`${a.kind}-${i}`}
                          assert={a}
                          showWeight={hasVaryingWeights(detail.data.asserts)}
                        />
                      ))}
                      {detail.data.asserts.length === 0 ? (
                        <p className="text-sm text-muted">
                          No assertions were evaluated.
                        </p>
                      ) : null}
                    </div>
                  </CollapsibleSection>

                  {/* The one capped viewer in the drawer: the prompt messages
                      and assert details scroll with the body, so this is the
                      single inner scrollbar rather than one per block. */}
                  <OutputSection
                    output={detail.data.output}
                    raw={detail.data.raw}
                    stopReason={detail.data.stop_reason}
                  />

                  <BaselineDiffSection
                    project={project}
                    suite={suite}
                    runId={runId}
                    caseKey={caseKey}
                    currentOutput={detail.data.output}
                  />

                  <CaseInputSection
                    cell={detail.data.cell}
                    vars={detail.data.vars}
                    prompt={detail.data.prompt}
                    request={detail.data.request}
                  />

                  {detail.data.raw !== undefined ? (
                    <RawMetadataSection raw={detail.data.raw} />
                  ) : null}
                </div>
              </div>
            </>
          ) : null}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/**
 * The model's output — and, when what we scored is really its exposed
 * reasoning, a plain statement of that. Without it the drawer shows a wall of
 * "Thinking Process: …" as if it were the answer, or an "(empty)" box with the
 * real text buried in the raw provider payload below.
 *
 * This is the one capped viewer in the drawer: the prompt messages and assert
 * details scroll with the body, so there is a single inner scrollbar rather
 * than one per block.
 */
function OutputSection({
  output,
  raw,
  stopReason,
}: {
  output: CaseResult["output"];
  raw: unknown;
  stopReason: string | null | undefined;
}) {
  const notice = reasoningNotice(raw, stopReason);
  return (
    <CollapsibleSection
      title="Output"
      meta={notice ? "· reasoning, not a final answer" : undefined}
    >
      {notice ? (
        <p className="mb-2 rounded-lg border border-amber/25 bg-amber/5 p-2.5 text-xs text-amber">
          {notice}
        </p>
      ) : null}
      <OutputViewer value={output} maxHeight="36rem" />
    </CollapsibleSection>
  );
}

/** `· 2 of 5 failed`, or nothing when everything passed. */
function assertMeta(asserts: readonly AssertResult[]): string | undefined {
  if (asserts.length === 0) return undefined;
  const failed = asserts.filter((a) => a.status !== "pass").length;
  return failed === 0
    ? `· all ${asserts.length} passed`
    : `· ${failed} of ${asserts.length} failed`;
}

/**
 * Weight is only meaningful when the case actually uses weighting; printing
 * `weight 1.00` on every row (the overwhelming default) is noise.
 */
function hasVaryingWeights(asserts: readonly AssertResult[]): boolean {
  return asserts.some((a) => a.weight !== 1);
}
