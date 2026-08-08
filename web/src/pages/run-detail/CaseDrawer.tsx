import { useCaseDetail } from "@/api/queries";
import type { AssertResult, CaseResult } from "@/api";
import { CopyButton } from "@/components/ui/CopyButton";
import { CollapsibleSection } from "@/components/ui/CollapsibleSection";
import { DetailDrawer } from "@/components/ui/DetailDrawer";
import { JsonTree, OutputViewer, RawText, outputToString } from "@/components/output";
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
  onPrev,
  onNext,
  position,
}: {
  runId: string;
  project: string;
  suite: string;
  caseKey: string | undefined;
  onClose: () => void;
  /** Step to the neighbouring case. Omitted at the ends of the loaded rows. */
  onPrev?: () => void;
  onNext?: () => void;
  position?: { index: number; total: number };
}) {
  const detail = useCaseDetail(runId, caseKey);

  // Shareable deep link to this exact case (the drawer re-opens from `?case=`).
  const permalinkFor = (key: string) =>
    `${window.location.origin}/runs/${encodeURIComponent(runId)}?case=${encodeURIComponent(key)}`;

  return (
    <DetailDrawer
      open={!!caseKey}
      item={detail.data}
      error={detail.isError ? detail.error : undefined}
      onRetry={() => detail.refetch()}
      onClose={onClose}
      onPrev={onPrev}
      onNext={onNext}
      position={position}
      navItemLabel="case"
      // Both fall back to the selection, so a cold deep link and a failed load
      // still name the case in the URL instead of an anonymous bar. Once the
      // case lands they follow it, so the copied link is always the one on
      // screen rather than a row still loading behind it.
      renderHeaderActions={(c) => {
        const key = c?.case_key ?? caseKey;
        return key ? <CopyButton value={permalinkFor(key)} label="Copy link" /> : null;
      }}
      renderEyebrow={(c) => c?.case_key ?? caseKey ?? null}
      renderTitle={(c) => c.name ?? c.case_key}
      renderSubheader={(c) => (
        <CaseVerdictStrip
          detail={c}
          project={project}
          suite={suite}
          runId={runId}
          caseKey={c.case_key}
        />
      )}
      // Body order runs verdict-first: what went wrong, why, and what the model
      // actually said — then the provenance. Prompt and Input are re-derivable
      // from the suite config; the output is the only thing this run alone
      // knows, so it leads.
      // Keyed by case: this element is the drawer's scroller, and the sections
      // inside it own their expanded state. Reused across a step, the next case
      // opens part-way down the previous one's output with whichever sections
      // that case happened to leave collapsed.
      renderBody={(c) => (
        <div key={c.case_key} className="flex-1 overflow-y-auto px-5 py-4">
          <div className="space-y-5">
            {c.error ? (
              <div className="rounded-lg border border-fail/25 bg-fail/5 p-3">
                <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-fail">
                  Error
                </div>
                <p className="text-sm text-fail">{c.error}</p>
                {/* The structured diagnostic a provider sent alongside the
                    message — a rate-limit window, a validation payload, a
                    child's own error object. An errored case has no output and
                    no raw metadata, so this is the only thing it carries; it is
                    deliberately exempt from `--no-raw` for that reason.
                    Rendered inline rather than collapsed: you opened an errored
                    case to read exactly this. */}
                {c.error_details != null ? (
                  <div className="mt-2.5">
                    <div className="mb-1 text-[11px] font-semibold uppercase tracking-wide text-fail/70">
                      Details
                    </div>
                    {typeof c.error_details === "object" ? (
                      <JsonTree data={c.error_details} className="text-[11px]/relaxed" />
                    ) : (
                      <RawText
                        text={outputToString(c.error_details)}
                        wrap
                        className="text-[11px]/relaxed"
                      />
                    )}
                  </div>
                ) : null}
              </div>
            ) : null}

            <CollapsibleSection title="Assertions" meta={assertMeta(c.asserts)}>
              <div className="space-y-2">
                {c.asserts.map((a, i) => (
                  <CaseAssertRow
                    key={`${a.kind}-${i}`}
                    assert={a}
                    showWeight={hasVaryingWeights(c.asserts)}
                  />
                ))}
                {c.asserts.length === 0 ? (
                  <p className="text-sm text-muted">No assertions were evaluated.</p>
                ) : null}
              </div>
            </CollapsibleSection>

            {/* The one capped viewer in the drawer: the prompt messages and
                assert details scroll with the body, so this is the single inner
                scrollbar rather than one per block. */}
            <OutputSection output={c.output} raw={c.raw} stopReason={c.stop_reason} />

            <BaselineDiffSection
              project={project}
              suite={suite}
              runId={runId}
              caseKey={c.case_key}
              currentOutput={c.output}
            />

            <CaseInputSection
              cell={c.cell}
              vars={c.vars}
              prompt={c.prompt}
              request={c.request}
            />

            {c.raw !== undefined ? <RawMetadataSection raw={c.raw} /> : null}
          </div>
        </div>
      )}
    />
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
