import { useState } from "react";
import type { ReactNode } from "react";
import type { CellKey, RenderedPrompt } from "@/api";
import { JsonTree, OutputViewer, RawText, outputToString } from "@/components/output";
import { cn } from "@/lib/cn";

/**
 * Schema-v2 case-drawer sections — the rendered prompt, the provider stop
 * reason, and the raw provider metadata. Every export here is presence-gated by
 * its caller (`CaseDrawer`): a v1 `CaseResult` (no `prompt`/`stop_reason`/`raw`)
 * renders none of them, so the drawer degrades to exactly its pre-v2 shape.
 */

// Role → chip tint (from the plan): system reads as the accent, user as a plain
// surface chip, assistant as the pass tint, and any tool turn as amber. Unknown
// roles fall back to the neutral surface chip.
const ROLE_TINT: Record<string, string> = {
  system: "bg-accent/12 text-accent",
  user: "bg-surface-2 text-muted",
  assistant: "bg-pass/12 text-pass",
  tool: "bg-amber/12 text-amber",
};
const ROLE_TINT_FALLBACK = "bg-surface-2 text-muted";

// stop_reason values that indicate the model was cut off rather than finishing
// on its own; matched case-insensitively. Truncation gets an amber chip.
const TRUNCATION_STOP_REASONS = ["max_tokens", "length", "content_length"];

function isTruncationStop(reason: string): boolean {
  return TRUNCATION_STOP_REASONS.includes(reason.trim().toLowerCase());
}

/**
 * A collapsible drawer section: an uppercase header button with a rotating
 * chevron (the drawer's dominant `Section` convention). Open by default —
 * the drawer shows everything it has; the toggle is for tucking sections away.
 */
function CollapsibleSection({
  title,
  defaultOpen = true,
  children,
}: {
  title: ReactNode;
  defaultOpen?: boolean;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <section>
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted hover:text-fg"
      >
        <svg
          width="14"
          height="14"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          className={cn("shrink-0 transition-transform", open && "rotate-90")}
          aria-hidden
        >
          <path d="M9 6l6 6-6 6" />
        </svg>
        <span>{title}</span>
      </button>
      {open ? <div className="mt-2">{children}</div> : null}
    </section>
  );
}

/** A single role chip: mono, lowercase, tinted per {@link ROLE_TINT}. */
function RoleChip({ role }: { role: string }) {
  return (
    <span
      className={cn(
        "rounded px-1.5 py-0.5 font-mono text-[11px] font-medium lowercase",
        ROLE_TINT[role] ?? ROLE_TINT_FALLBACK,
      )}
    >
      {role}
    </span>
  );
}

/**
 * The "Prompt" section: what was actually sent to the model. A messages-style
 * prompt renders one card per turn (role chip + content through `OutputViewer`,
 * whose auto-detection handles JSON tool payloads and markdown alike); a
 * text-style prompt renders a single `OutputViewer`.
 */
export function PromptSection({ prompt }: { prompt: RenderedPrompt }) {
  const messages = "messages" in prompt ? prompt.messages : null;
  const text = "messages" in prompt ? null : prompt.text;
  const title = (
    <>
      Prompt
      {messages ? (
        <span className="font-normal normal-case tracking-normal text-muted/80">
          {" "}
          · {messages.length} {messages.length === 1 ? "message" : "messages"}
        </span>
      ) : null}
    </>
  );

  return (
    <CollapsibleSection title={title}>
      {messages ? (
        <div className="space-y-2">
          {messages.map((m, i) => (
            <div
              key={`${m.role}-${i}`}
              className="rounded-lg border border-border p-3"
            >
              <div className="mb-2">
                <RoleChip role={m.role} />
              </div>
              <OutputViewer value={m.content} />
            </div>
          ))}
        </div>
      ) : (
        <OutputViewer value={text} />
      )}
    </CollapsibleSection>
  );
}

/**
 * The provider stop reason as a mono chip appended to the drawer meta line.
 * Amber when it indicates truncation (see {@link TRUNCATION_STOP_REASONS}),
 * muted otherwise.
 */
export function StopReasonChip({ reason }: { reason: string }) {
  const truncated = isTruncationStop(reason);
  return (
    <span
      title="Provider stop reason"
      className={cn(
        "rounded px-1.5 py-0.5 font-mono text-[11px] font-medium",
        truncated ? "bg-amber/12 text-amber" : "bg-surface-2 text-muted",
      )}
    >
      {reason}
    </span>
  );
}

/**
 * The "Provider metadata" section: the raw, provider-specific payload. Objects
 * render through `JsonTree`; any non-object value falls back to `RawText`.
 */
export function RawMetadataSection({ raw }: { raw: unknown }) {
  return (
    <CollapsibleSection title="Provider metadata">
      {typeof raw === "object" && raw !== null ? (
        <JsonTree data={raw} />
      ) : (
        <RawText text={outputToString(raw)} wrap />
      )}
    </CollapsibleSection>
  );
}

/**
 * The "Input" section: what was fed into this matrix cell. Always shows the
 * cell identity (which provider × prompt × test this row exercised); when the
 * stored case carries rendered `vars` (schema ≥ v2.1), lists them as decomposed
 * rows. Pre-v2.1 blobs without `vars` still get the identity — the variables
 * block is presence-gated, so old runs simply gain the identity they never had.
 */
export function InputSection({
  cell,
  vars,
}: {
  cell: CellKey;
  vars?: Record<string, unknown>;
}) {
  const varEntries = vars ? Object.entries(vars) : [];
  const title = (
    <>
      Input
      {varEntries.length > 0 ? (
        <span className="font-normal normal-case tracking-normal text-muted/80">
          {" "}
          · {varEntries.length}{" "}
          {varEntries.length === 1 ? "variable" : "variables"}
        </span>
      ) : null}
    </>
  );

  return (
    <CollapsibleSection title={title}>
      <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
        <dt className="text-muted">provider</dt>
        <dd className="break-all font-mono">{cell.provider_id}</dd>
        {cell.prompt_id != null ? (
          <>
            <dt className="text-muted">prompt</dt>
            <dd className="break-all font-mono">{cell.prompt_id}</dd>
          </>
        ) : null}
        <dt className="text-muted">test</dt>
        <dd className="break-all font-mono">{cell.test_id || "—"}</dd>
        {cell.repeat > 0 ? (
          <>
            <dt className="text-muted">repeat</dt>
            <dd className="font-mono tabular-nums">{cell.repeat}</dd>
          </>
        ) : null}
      </dl>

      {varEntries.length > 0 ? (
        <div className="mt-3 space-y-2">
          <div className="text-[11px] font-semibold uppercase tracking-wide text-muted/80">
            Variables
          </div>
          {varEntries.map(([name, value]) => (
            <div key={name} className="rounded-lg border border-border p-2">
              <div className="mb-1 break-all font-mono text-[11px] font-medium text-fg">
                {name}
              </div>
              {typeof value === "object" && value !== null ? (
                <JsonTree data={value} className="text-[11px]" />
              ) : (
                <RawText
                  text={outputToString(value)}
                  wrap
                  className="text-[11px]"
                />
              )}
            </div>
          ))}
        </div>
      ) : null}
    </CollapsibleSection>
  );
}

/**
 * The authored criteria for one assertion (its stored `criteria` blob): the
 * type-specific fields of the assertion definition, minus the redundant `type`
 * (already shown as the row's kind chip). A `negate: true` entry renders as a
 * marker. A lone scalar criterion (e.g. the `contains` substring) shows inline;
 * anything richer goes through `JsonTree` (decomposed rows, long strings
 * truncated) — never a raw JSON dump. Renders nothing when there is nothing
 * beyond the kind to show (e.g. `is-json`).
 */
export function AssertCriteria({ criteria }: { criteria: unknown }) {
  if (
    typeof criteria !== "object" ||
    criteria === null ||
    Array.isArray(criteria)
  ) {
    return (
      <RawText
        text={outputToString(criteria)}
        wrap
        className="mt-1.5 text-[11px]"
      />
    );
  }

  const obj = criteria as Record<string, unknown>;
  const negated = obj.negate === true;
  const restEntries = Object.entries(obj).filter(
    ([k]) => k !== "type" && k !== "negate",
  );
  if (restEntries.length === 0 && !negated) return null;

  const only = restEntries.length === 1 ? restEntries[0] : undefined;
  const loneScalar =
    only !== undefined && (typeof only[1] !== "object" || only[1] === null);

  return (
    <div className="mt-1.5 text-[11px]">
      <div className="flex flex-wrap items-center gap-1.5">
        {restEntries.length > 0 ? <span className="text-muted">expects</span> : null}
        {negated ? (
          <span className="rounded bg-amber/12 px-1 py-0.5 font-medium text-amber">
            negated
          </span>
        ) : null}
        {loneScalar && only ? (
          <span className="break-all font-mono text-fg/90">
            {outputToString(only[1])}
          </span>
        ) : null}
      </div>
      {restEntries.length > 0 && !loneScalar ? (
        <JsonTree data={Object.fromEntries(restEntries)} className="mt-1" />
      ) : null}
    </div>
  );
}
