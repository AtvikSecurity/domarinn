import type { CellKey, RenderedPrompt } from "@/api";
import { JsonTree, OutputViewer, RawText, outputToString } from "@/components/output";
import { Chip, type ChipTone } from "@/components/ui/Chip";
import { CollapsibleSection } from "@/components/ui/CollapsibleSection";

/**
 * Schema-v2 case-drawer sections — the rendered prompt, the provider stop
 * reason, and the raw provider metadata. Every export here is presence-gated by
 * its caller (`CaseDrawer`): a v1 `CaseResult` (no `prompt`/`stop_reason`/`raw`)
 * renders none of them, so the drawer degrades to exactly its pre-v2 shape.
 */

// Role → chip tone: system reads as the accent, user as a plain surface chip,
// assistant as the pass tint, and any tool turn as amber. Unknown roles fall
// back to the neutral surface chip.
const ROLE_TONE: Record<string, ChipTone> = {
  system: "accent",
  user: "neutral",
  assistant: "pass",
  tool: "amber",
};

// stop_reason values that indicate the model was cut off rather than finishing
// on its own; matched case-insensitively. Truncation gets an amber chip.
const TRUNCATION_STOP_REASONS = ["max_tokens", "length", "content_length"];

function isTruncationStop(reason: string): boolean {
  return TRUNCATION_STOP_REASONS.includes(reason.trim().toLowerCase());
}

/**
 * A single chat-turn role chip: mono, lowercase, tinted per {@link ROLE_TONE}.
 * Named for the chat role specifically — `Layout` has its own `AuthRoleChip`
 * for the very different concept of a user's permission role.
 */
function ChatRoleChip({ role }: { role: string }) {
  return (
    <Chip tone={ROLE_TONE[role] ?? "neutral"} mono className="lowercase">
      {role}
    </Chip>
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

  return (
    <CollapsibleSection
      title="Prompt"
      meta={
        messages
          ? `· ${messages.length} ${messages.length === 1 ? "message" : "messages"}`
          : undefined
      }
    >
      {messages ? (
        <div className="space-y-2">
          {messages.map((m, i) => (
            <div
              key={`${m.role}-${i}`}
              className="rounded-lg border border-border p-3"
            >
              <div className="mb-2">
                <ChatRoleChip role={m.role} />
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
  return (
    <Chip
      // Kept as a native title deliberately: several specs assert on it, and it
      // labels a chip whose text ("max_tokens") is otherwise unexplained.
      title="Provider stop reason"
      tone={isTruncationStop(reason) ? "amber" : "neutral"}
      mono
    >
      {reason}
    </Chip>
  );
}

/**
 * Detects that the scored text is really the model's exposed reasoning.
 *
 * Reasoning models return their answer in `message.reasoning` (ollama) or
 * `message.reasoning_content` (DeepSeek/vLLM) and leave `content` empty. The
 * provider now falls back to that text so the case is scored on something real
 * — but the reader must be told, because "the model reasoned about the answer"
 * and "the model answered" are very different things, and a `length` stop
 * reason usually means it never got to the answer at all.
 */
export function reasoningNotice(
  raw: unknown,
  stopReason: string | null | undefined,
): string | null {
  if (typeof raw !== "object" || raw === null) return null;
  const source = (raw as Record<string, unknown>).domarinn_output_source;
  if (source !== "reasoning") return null;
  return stopReason && isTruncationStop(stopReason)
    ? "This model returned no final message — it was cut off mid-reasoning, so the text below is its reasoning, not an answer. Raise max_tokens."
    : "This model returned its reasoning instead of a final message, so the text below is what it was thinking, not an answer.";
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

  return (
    <CollapsibleSection
      title="Input"
      meta={
        varEntries.length > 0
          ? `· ${varEntries.length} ${varEntries.length === 1 ? "variable" : "variables"}`
          : undefined
      }
    >
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
                <JsonTree data={value} className="text-[11px]/relaxed" />
              ) : (
                <RawText
                  text={outputToString(value)}
                  wrap
                  className="text-[11px]/relaxed"
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
        className="mt-1.5 text-[11px]/relaxed"
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
