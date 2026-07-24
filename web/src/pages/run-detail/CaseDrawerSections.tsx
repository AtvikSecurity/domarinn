import { JsonTree, RawText, outputToString } from "@/components/output";
import { Chip } from "@/components/ui/Chip";
import { CollapsibleSection } from "@/components/ui/CollapsibleSection";

/**
 * Schema-v2 case-drawer pieces that are not sections of their own: the provider
 * stop reason chip, the reasoning-source notice, and the raw provider metadata
 * dump. Each is presence-gated by its caller (`CaseDrawer`), so a v1
 * `CaseResult` (no `stop_reason`/`raw`) renders none of them.
 *
 * The rendered prompt and the test input live in `CaseInputSection`, which owns
 * the whole "what went in" story including the captured provider request.
 */

// stop_reason values that indicate the model was cut off rather than finishing
// on its own; matched case-insensitively. Truncation gets an amber chip.
const TRUNCATION_STOP_REASONS = ["max_tokens", "length", "content_length"];

function isTruncationStop(reason: string): boolean {
  return TRUNCATION_STOP_REASONS.includes(reason.trim().toLowerCase());
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
