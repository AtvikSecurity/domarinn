import { useState } from "react";
import type { CellKey, ChatMessage, RenderedPrompt } from "@/api";
import { JsonTree, OutputViewer, RawText, outputToString } from "@/components/output";
import { Chip, type ChipTone } from "@/components/ui/Chip";
import { CollapsibleSection } from "@/components/ui/CollapsibleSection";
import { CopyButton } from "@/components/ui/CopyButton";
import { SegmentedControl } from "@/components/ui/SegmentedControl";
import {
  formatJson,
  parseProviderRequest,
  promptAsJson,
  requestModel,
  requestTarget,
  type ProviderRequestView,
} from "@/lib/providerRequest";

/**
 * The "Input" section: everything that went *in*, in one place.
 *
 * This replaces two sibling sections — "Prompt" (the rendered messages) and
 * "Input" (variables and cell identity) — that between them answered a single
 * question in two halves, under names that gave no hint which half was which.
 * A reader asking "what exactly did the model receive?" had to open both and
 * assemble the answer.
 *
 * Two views, because the honest answer has two forms:
 *
 * - **Rendered** — readable: which model, the message turns, the variables that
 *   substituted into them, the cell identity.
 * - **Raw** — the verbatim JSON the provider sent. Not reconstructed here: it is
 *   captured server-side by the provider that built it, because each provider
 *   assembles a different body (Anthropic lifts `system` out of the message
 *   list; the OpenAI shape folds a text prompt into one user message and merges
 *   sampling params) and a client-side guess would look authoritative while
 *   being wrong.
 *
 * The `Rendered`/`Raw` wording matches `OutputViewer`'s own toggle deliberately
 * — same concept, same words. They are told apart by their section, and for
 * assistive tech and tests by this control's `aria-label`.
 */
export function CaseInputSection({
  cell,
  vars,
  prompt,
  request,
}: {
  cell: CellKey;
  vars?: Record<string, unknown>;
  prompt?: RenderedPrompt;
  /** The captured provider request (`CaseResult.request`), `unknown` on the wire. */
  request: unknown;
}) {
  // Component state, not persisted: the readable view is the right default every
  // time the drawer opens, and a stuck "Raw" would bury the messages.
  const [view, setView] = useState<"rendered" | "raw">("rendered");

  const captured = parseProviderRequest(request);
  const varEntries = vars ? Object.entries(vars) : [];
  const messages = prompt && "messages" in prompt ? prompt.messages : null;

  // A v1 case has neither a captured request nor a rendered prompt, so "Raw"
  // would lead to a panel explaining that there is nothing to show. Offer the
  // toggle only when it goes somewhere.
  const hasRawView = captured !== null || prompt !== undefined;
  const effectiveView = hasRawView ? view : "rendered";

  return (
    <CollapsibleSection
      title="Input"
      meta={inputMeta(varEntries.length, messages?.length)}
      actions={
        hasRawView ? (
          <SegmentedControl
            options={[
              { value: "rendered", label: "Rendered" },
              { value: "raw", label: "Raw" },
            ]}
            value={effectiveView}
            onChange={setView}
            ariaLabel="Input view"
            size="xs"
          />
        ) : undefined
      }
    >
      {effectiveView === "rendered" ? (
        <RenderedInput
          cell={cell}
          varEntries={varEntries}
          prompt={prompt}
          captured={captured}
        />
      ) : (
        <RawInput captured={captured} prompt={prompt} />
      )}
    </CollapsibleSection>
  );
}

/** `· 2 variables · 2 messages`, omitting either half when it is empty. */
function inputMeta(varCount: number, messageCount: number | undefined): string | undefined {
  const parts: string[] = [];
  if (varCount > 0) {
    parts.push(`${varCount} ${varCount === 1 ? "variable" : "variables"}`);
  }
  if (messageCount !== undefined && messageCount > 0) {
    parts.push(`${messageCount} ${messageCount === 1 ? "message" : "messages"}`);
  }
  return parts.length > 0 ? `· ${parts.join(" · ")}` : undefined;
}

/**
 * A small uppercase label above a block, so no block is left unattributed.
 *
 * The label keeps its own element so an exact-text query still matches it once
 * `hint` is appended — same reason `CollapsibleSection` wraps its `title`
 * separately from its `meta`.
 */
function BlockLabel({ label, hint }: { label: string; hint?: string }) {
  return (
    <div className="mb-1.5 text-[11px] font-semibold uppercase tracking-wide text-muted/80">
      <span>{label}</span>
      {hint ? (
        <span className="ml-1.5 font-normal normal-case tracking-normal text-muted/70">
          {hint}
        </span>
      ) : null}
    </div>
  );
}

// Role → chip tone: system reads as the accent, user as a plain surface chip,
// assistant as the pass tint, and any tool turn as amber. Unknown roles fall
// back to the neutral surface chip.
const ROLE_TONE: Record<string, ChipTone> = {
  system: "accent",
  user: "neutral",
  assistant: "pass",
  tool: "amber",
};

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
 * One turn's body: the reasoning that preceded it, what it said, and the calls
 * it made.
 *
 * `content` is a bare string for almost every turn — the block form only shows
 * up in a transcript replaying a model's reasoning. Thinking is dimmed and
 * labelled rather than shown inline, so it never reads as something the model
 * said aloud.
 */
function MessageBody({ message }: { message: ChatMessage }) {
  const blocks = Array.isArray(message.content) ? message.content : null;
  return (
    <>
      {blocks ? (
        <div className="space-y-2">
          {blocks.map((block, i) =>
            block.type === "thinking" ? (
              <div
                key={i}
                className="rounded border border-dashed border-border p-2"
              >
                <div className="mb-1 font-mono text-[10px] uppercase tracking-wide text-muted/80">
                  thinking
                </div>
                <RawText text={block.thinking} wrap className="text-muted" />
              </div>
            ) : (
              <OutputViewer key={i} value={block.text} />
            ),
          )}
        </div>
      ) : (
        <OutputViewer value={message.content} />
      )}
      {message.tool_calls?.length ? (
        <div className="mt-2 space-y-1.5">
          {message.tool_calls.map((call, i) => (
            <div
              key={call.id ?? i}
              className="rounded border border-border p-2"
            >
              <div className="mb-1 break-all font-mono text-[11px] font-medium text-fg">
                {call.name}
              </div>
              <JsonTree
                data={call.arguments}
                className="text-[11px]/relaxed"
              />
            </div>
          ))}
        </div>
      ) : null}
    </>
  );
}

/**
 * The readable view, ordered by what a reader reaches for first: the model that
 * answered, then the turns it saw, then the variables that produced them, then
 * the cell coordinates.
 *
 * The model line matters more than it looks: a provider id is a config alias, so
 * `provider: fast` says nothing about which model ran. Before this, the only
 * place that named it was the raw metadata dump at the very bottom.
 */
function RenderedInput({
  cell,
  varEntries,
  prompt,
  captured,
}: {
  cell: CellKey;
  varEntries: [string, unknown][];
  prompt: RenderedPrompt | undefined;
  captured: ProviderRequestView | null;
}) {
  const model = captured ? requestModel(captured) : null;
  const target = captured ? requestTarget(captured) : "";
  const messages = prompt && "messages" in prompt ? prompt.messages : null;
  const text = prompt && !("messages" in prompt) ? prompt.text : null;

  return (
    <div className="space-y-4">
      {model || target ? (
        <div>
          <BlockLabel label="Sent to" />
          <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
            {model ? (
              <>
                <dt className="text-muted">model</dt>
                <dd className="break-words font-mono">{model}</dd>
              </>
            ) : null}
            {target ? (
              <>
                <dt className="text-muted">endpoint</dt>
                {/* `break-words`, not `break-all`: the target reads
                    "POST https://…", and break-all would split the method too.
                    overflow-wrap still breaks the URL when it cannot fit. */}
                <dd className="break-words font-mono text-[11px]/relaxed text-fg/80">
                  {target}
                </dd>
              </>
            ) : null}
          </dl>
        </div>
      ) : null}

      {prompt ? (
        <div>
          <BlockLabel
            label={messages ? "Messages" : "Prompt"}
            hint="after template rendering"
          />
          {messages ? (
            <div className="space-y-2">
              {messages.map((m, i) => (
                <div
                  key={`${m.role}-${i}`}
                  className="rounded-lg border border-border p-3"
                >
                  <div className="mb-2 flex items-center gap-1.5">
                    <ChatRoleChip role={m.role} />
                    {m.tool_call_id ? (
                      <span className="font-mono text-[10px] text-muted/80">
                        answering {m.tool_call_id}
                      </span>
                    ) : null}
                  </div>
                  <MessageBody message={m} />
                </div>
              ))}
            </div>
          ) : (
            <OutputViewer value={text} />
          )}
        </div>
      ) : null}

      {varEntries.length > 0 ? (
        <div>
          <BlockLabel label="Variables" hint="substituted into the prompt" />
          <div className="space-y-2">
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
        </div>
      ) : null}

      <div>
        <BlockLabel label="Cell" />
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
      </div>
    </div>
  );
}

/**
 * The verbatim view: the payload as sent, with the destination above it.
 *
 * When nothing was captured this falls back to the rendered prompt and says so.
 * The distinction is not pedantic — the fallback is what domarinn rendered, not
 * what a provider assembled from it, so presenting it as "the request" would be
 * wrong for every provider that reshapes the message list.
 */
function RawInput({
  captured,
  prompt,
}: {
  captured: ProviderRequestView | null;
  prompt: RenderedPrompt | undefined;
}) {
  const payload = captured ? captured.payload : promptAsJson(prompt);
  const target = captured ? requestTarget(captured) : "";
  const json = formatJson(payload);

  return (
    <div className="space-y-2">
      {captured ? null : (
        <p className="rounded-lg border border-border bg-surface-2 p-2.5 text-xs text-muted">
          No provider payload was recorded for this case, so this is the prompt
          domarinn rendered — not the body a provider built from it. Runs from
          before request capture have none, and the <code>http</code> provider
          withholds its request because its templates are rendered against{" "}
          <code>env</code> and could carry credentials.
        </p>
      )}

      <div className="flex items-start justify-between gap-2">
        {target ? (
          <div className="min-w-0">
            <BlockLabel label="Destination" />
            <div className="break-words font-mono text-[11px]/relaxed text-fg/80">
              {target}
            </div>
          </div>
        ) : (
          <BlockLabel label={captured ? "Payload" : "Rendered prompt"} />
        )}
        <CopyButton
          value={json}
          label={captured ? "Copy payload" : "Copy prompt"}
          className="shrink-0"
        />
      </div>

      {target ? <BlockLabel label="Payload" /> : null}
      {payload === null || payload === undefined ? (
        <p className="text-sm text-muted">This case recorded no payload.</p>
      ) : typeof payload === "object" ? (
        <JsonTree data={payload} />
      ) : (
        <RawText text={outputToString(payload)} wrap />
      )}
    </div>
  );
}
