import type { ReactNode } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { useCaseDetail } from "@/api/queries";
import type { AssertResult } from "@/api";
import { StatusBadge } from "@/components/StatusBadge";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { CopyButton } from "@/components/ui/CopyButton";
import { ErrorState } from "@/components/States";
import { JsonTree, OutputViewer, RawText, outputToString } from "@/components/output";
import { formatCost, formatLatency, formatTokens } from "@/lib/format";
import { cn } from "@/lib/cn";
import { BaselineDiffSection } from "./BaselineDiffSection";
import { CaseHistorySection } from "./CaseHistorySection";
import {
  AssertCriteria,
  InputSection,
  PromptSection,
  RawMetadataSection,
  StopReasonChip,
} from "./CaseDrawerSections";

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

          <div className="flex-1 overflow-y-auto px-5 py-4">
            {detail.isPending ? (
              <CenteredSpinner label="Loading case…" />
            ) : detail.isError ? (
              <ErrorState error={detail.error} onRetry={() => detail.refetch()} />
            ) : detail.data ? (
              <div className="space-y-5">
                <div className="flex flex-wrap items-center gap-2">
                  <StatusBadge status={detail.data.status} />
                  {(detail.data.tags ?? []).map((t) => (
                    <span
                      key={t}
                      className="rounded bg-surface-2 px-1.5 py-0.5 text-[11px] text-muted"
                    >
                      {t}
                    </span>
                  ))}
                  {detail.data.cached ? (
                    <span className="rounded bg-surface-2 px-1.5 py-0.5 text-[11px] font-medium text-muted">
                      cached
                    </span>
                  ) : null}
                  {detail.data.attempts > 1 ? (
                    <span className="rounded bg-amber/12 px-1.5 py-0.5 text-[11px] font-medium text-amber">
                      {detail.data.attempts} attempts
                    </span>
                  ) : null}
                  {detail.data.stop_reason ? (
                    <StopReasonChip reason={detail.data.stop_reason} />
                  ) : null}
                  <span className="ml-auto text-xs tabular-nums text-muted">
                    score {detail.data.score.toFixed(2)} ·{" "}
                    {formatTokens(
                      (detail.data.usage?.input_tokens ?? 0) +
                        (detail.data.usage?.output_tokens ?? 0),
                    )}{" "}
                    tok · {formatCost(detail.data.cost_usd)} ·{" "}
                    {formatLatency(detail.data.latency_ms)}
                  </span>
                </div>

                {detail.data.error ? (
                  <div className="rounded-lg border border-fail/25 bg-fail/5 p-3">
                    <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-fail">
                      Error
                    </div>
                    <p className="text-sm text-fail/90">{detail.data.error}</p>
                  </div>
                ) : null}

                <InputSection cell={detail.data.cell} vars={detail.data.vars} />

                {detail.data.prompt ? (
                  <PromptSection prompt={detail.data.prompt} />
                ) : null}

                <Section title="Assertions">
                  <div className="space-y-2">
                    {detail.data.asserts.map((a, i) => (
                      <AssertRow key={`${a.kind}-${i}`} assert={a} />
                    ))}
                    {detail.data.asserts.length === 0 ? (
                      <p className="text-sm text-muted">No assertions were evaluated.</p>
                    ) : null}
                  </div>
                </Section>

                <Section title="Output">
                  <OutputViewer value={detail.data.output} />
                </Section>

                {caseKey ? (
                  <BaselineDiffSection
                    project={project}
                    suite={suite}
                    runId={runId}
                    caseKey={caseKey}
                    currentOutput={detail.data.output}
                  />
                ) : null}

                {caseKey ? (
                  <CaseHistorySection
                    project={project}
                    suite={suite}
                    runId={runId}
                    caseKey={caseKey}
                  />
                ) : null}

                {detail.data.raw !== undefined ? (
                  <RawMetadataSection raw={detail.data.raw} />
                ) : null}
              </div>
            ) : null}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section>
      <h3 className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted">
        {title}
      </h3>
      {children}
    </section>
  );
}

function AssertRow({ assert }: { assert: AssertResult }) {
  return (
    <div
      className={cn(
        "rounded-lg border p-3",
        assert.status === "pass"
          ? "border-pass/25 bg-pass/5"
          : assert.status === "error"
            ? "border-error/25 bg-error/5"
            : "border-fail/25 bg-fail/5",
      )}
    >
      <div className="flex items-center gap-2">
        <StatusBadge status={assert.status} size="xs" />
        <span className="rounded bg-surface-2 px-1.5 py-0.5 font-mono text-[11px] font-medium text-fg">
          {assert.kind}
        </span>
        <span className="ml-auto text-xs tabular-nums text-muted">
          score {assert.score.toFixed(2)} · weight {assert.weight.toFixed(2)}
        </span>
      </div>
      {assert.criteria != null ? (
        <AssertCriteria criteria={assert.criteria} />
      ) : null}
      <p className="mt-1.5 text-sm text-fg/90">{assert.reason}</p>
      {assert.details !== undefined ? (
        typeof assert.details === "object" && assert.details !== null ? (
          <JsonTree data={assert.details} className="mt-2 text-[11px]" />
        ) : (
          <RawText
            text={outputToString(assert.details)}
            wrap
            className="mt-2 text-[11px]"
          />
        )
      ) : null}
    </div>
  );
}
