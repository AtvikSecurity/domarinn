import type { ReactNode } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { useCaseDetail } from "@/api/queries";
import type { AssertResult } from "@/api";
import { StatusBadge } from "@/components/StatusBadge";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { ErrorState } from "@/components/States";
import { formatCost, formatLatency, formatTokens } from "@/lib/format";
import { cn } from "@/lib/cn";

function stringify(v: unknown): string {
  if (v === undefined) return "";
  return typeof v === "string" ? v : JSON.stringify(v, null, 2);
}

export function CaseDrawer({
  runId,
  caseKey,
  onClose,
}: {
  runId: string;
  caseKey: string | undefined;
  onClose: () => void;
}) {
  const open = !!caseKey;
  const detail = useCaseDetail(runId, caseKey);

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
            <Dialog.Close className="rounded-md p-1.5 text-muted hover:bg-surface-2 hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
              <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
                <path d="M6 6l12 12M18 6L6 18" />
              </svg>
            </Dialog.Close>
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
                  {detail.data.tags.map((t) => (
                    <span
                      key={t}
                      className="rounded bg-surface-2 px-1.5 py-0.5 text-[11px] text-muted"
                    >
                      {t}
                    </span>
                  ))}
                  <span className="ml-auto text-xs text-muted">
                    {formatTokens(
                      (detail.data.usage?.input_tokens ?? 0) +
                        (detail.data.usage?.output_tokens ?? 0),
                    )}{" "}
                    tok · {formatCost(detail.data.cost_usd)} ·{" "}
                    {formatLatency(detail.data.latency_ms)}
                  </span>
                </div>

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
                  <CodeBlock text={stringify(detail.data.output)} />
                </Section>
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

function CodeBlock({ text }: { text: string }) {
  return (
    <pre className="max-h-80 overflow-auto rounded-lg border border-border bg-bg p-3 font-mono text-xs leading-relaxed whitespace-pre-wrap break-words">
      {text || <span className="text-muted">(empty)</span>}
    </pre>
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
      <p className="mt-1.5 text-sm text-fg/90">{assert.reason}</p>
      {assert.details !== undefined ? (
        <pre className="mt-2 overflow-auto rounded border border-border bg-bg p-2 font-mono text-[11px] text-muted">
          {JSON.stringify(assert.details, null, 2)}
        </pre>
      ) : null}
    </div>
  );
}
