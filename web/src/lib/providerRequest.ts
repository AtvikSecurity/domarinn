import type { RenderedPrompt } from "@/api";

/**
 * Reading `CaseResult.request` — the provider request captured server-side.
 *
 * The field is `unknown` on the wire (a provider-authored JSON envelope), so
 * every access is narrowed here rather than in JSX. Keeping it pure also keeps
 * the "what did we actually send" logic testable, which matters because getting
 * it subtly wrong would mislabel a payload as authoritative.
 */

/** A captured request, narrowed by transport. */
export type ProviderRequestView =
  | {
      transport: "http";
      method: string;
      url: string;
      /** The request body — the part you can paste into `curl`. */
      payload: unknown;
    }
  | {
      transport: "exec";
      command: string;
      args: readonly string[];
      /** The protocol document written to the child's stdin. */
      payload: unknown;
    }
  | {
      /** A transport this UI predates. Shown verbatim rather than guessed at. */
      transport: "other";
      payload: unknown;
    };

function asRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function asString(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 ? value : null;
}

/**
 * Narrow a stored `CaseResult.request` into something renderable, or `null` when
 * the case carries no captured request at all (a run from before capture
 * existed, or the `http` provider, which withholds it because its templates are
 * rendered against `env` and would leak credentials).
 */
export function parseProviderRequest(raw: unknown): ProviderRequestView | null {
  const obj = asRecord(raw);
  if (!obj) return null;

  const transport = obj.transport;
  if (transport === "http") {
    const url = asString(obj.url);
    if (!url) return { transport: "other", payload: raw };
    return {
      transport: "http",
      method: asString(obj.method) ?? "POST",
      url,
      payload: obj.body,
    };
  }
  if (transport === "exec") {
    const command = asString(obj.command);
    if (!command) return { transport: "other", payload: raw };
    return {
      transport: "exec",
      command,
      args: Array.isArray(obj.args) ? obj.args.filter(isString) : [],
      payload: obj.stdin,
    };
  }
  return { transport: "other", payload: raw };
}

function isString(value: unknown): value is string {
  return typeof value === "string";
}

/**
 * The one-line "where this went" label: `POST https://…/chat/completions`, or
 * the command line for an exec provider. Empty for an unrecognized transport,
 * whose envelope is shown whole instead.
 */
export function requestTarget(view: ProviderRequestView): string {
  switch (view.transport) {
    case "http":
      return `${view.method} ${view.url}`;
    case "exec":
      return [view.command, ...view.args].join(" ");
    case "other":
      return "";
  }
}

/**
 * The model the request actually names, when the payload carries one.
 *
 * Worth surfacing separately: a provider id in the suite config is an alias, so
 * `provider: "fast"` tells you nothing about which model answered. This is the
 * only place in the drawer that states it without reading raw metadata.
 */
export function requestModel(view: ProviderRequestView): string | null {
  const payload = asRecord(view.payload);
  return payload ? asString(payload.model) : null;
}

/**
 * The rendered prompt as a plain JSON value, for the fallback raw view shown
 * when no provider request was captured.
 *
 * Deliberately *not* dressed up as a request envelope: it is the prompt
 * domarinn rendered, not the payload a provider assembled from it, and the two
 * differ (Anthropic lifts `system` out of the message list, the OpenAI shape
 * folds a text prompt into one user message and merges sampling params). The
 * caller labels it as such.
 */
export function promptAsJson(prompt: RenderedPrompt | undefined): unknown {
  return prompt ?? null;
}

/** Pretty-printed JSON for the copy button and the raw text block. */
export function formatJson(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2) ?? String(value);
  } catch {
    // Cyclic or otherwise unserializable: better to show something than to
    // blank the panel.
    return String(value);
  }
}
