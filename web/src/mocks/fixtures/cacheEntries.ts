// Cache entries for the browse UI.
//
// Deliberately not uniform. The states below all exist in a real store and each
// one is a rendering decision the page has to get right, so the fixture carries
// them rather than leaving them to be discovered against a live server:
//
//   * entries written before 0.5, which have a `provider_fingerprint` and no
//     `request` at all (the drawer must fall back rather than show nothing),
//   * entries the backfill has not reached (`indexed: false` — "indexing", not
//     "no model"),
//   * a body this server could not parse (`parseable: false`),
//   * a ≤0.4.x grader entry, whose kind is inferred from its verdict tag,
//   * one very large output, which the drawer must refuse to auto-expand,
//   * one deeply nested `raw`, which must not go through the JSON tree,
//   * missing usage and missing cost, which must render as "-" rather than 0,
//   * entries whose output came back empty and why (`empty_reason`), which is
//     what the reason filter and its facet exist to find — a store with none of
//     them would let an empty dropdown pass for a working one.

import type {
  CacheEntryDetail,
  CacheEntryListItem,
  CacheEntryRunsResponse,
  CacheFacetsResponse,
} from "@/api";
import { hash, rand } from "./rng";

const NOW = Date.parse("2026-07-28T12:00:00Z");
const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** Above the drawer's auto-expand threshold, so the guard is exercised. */
const HUGE_OUTPUT_BYTES = 320_000;

const MODELS = [
  "claude-opus-5",
  "claude-sonnet-5",
  "gpt-4o",
  "text-embedding-3-large",
] as const;

const KINDS = ["provider", "judge", "embedding", "exec_assert"] as const;

function toIso(ms: number): string {
  return new Date(ms).toISOString();
}

/** A stable, well-formed `sha256:` key for an index. */
function keyFor(i: number): string {
  let hex = "";
  for (let block = 0; block < 8; block++) {
    hex += hash("cache-entry", i, block).toString(16).padStart(8, "0");
  }
  return `sha256:${hex.slice(0, 64)}`;
}

interface Shape {
  /** Not yet reached by the backfill: metadata is unknown, not absent. */
  unindexed?: boolean;
  /** Examined, and the body was not something this server understands. */
  unparseable?: boolean;
  /** Written before 0.5: a fingerprint instead of a request. */
  preRequest?: boolean;
  /** A ≤0.4.x grader entry. */
  legacyVerdict?: boolean;
  huge?: boolean;
  deepRaw?: boolean;
  noUsage?: boolean;
  /** The output came back empty, and the entry records why. */
  emptyReason?: string;
}

/**
 * Which of the 180 entries deviate from the ordinary shape.
 *
 * Three distinct `empty_reason` values with different counts, deliberately: a
 * facet dropdown with one option cannot show whether it sorts by count, and a
 * single reason would let the page look right while `?empty_reason=` was
 * hard-coded to it.
 */
function shapeOf(i: number): Shape {
  if (i < 12) return { preRequest: true };
  if (i < 32) return { unindexed: true };
  if (i === 32) return { unparseable: true };
  if (i === 33) return { huge: true };
  if (i === 34) return { deepRaw: true };
  if (i < 37) return { legacyVerdict: true };
  if (i < 40) return { noUsage: true };
  // Eleven poisoned entries, weighted toward `refusal` the way a real store is.
  if (i < 46) return { emptyReason: "refusal" };
  if (i < 50) return { emptyReason: "blank" };
  if (i === 50) return { emptyReason: "truncated" };
  return {};
}

function modelFor(i: number): string {
  return MODELS[Math.floor(rand("model", i) * MODELS.length)] as string;
}

function kindFor(i: number, shape: Shape): string {
  if (shape.legacyVerdict) return "judge";
  const model = modelFor(i);
  if (model.startsWith("text-embedding")) return "embedding";
  return KINDS[Math.floor(rand("kind", i) * KINDS.length)] as string;
}

function requestSummaryFor(kind: string): string {
  if (kind === "exec_assert") return "exec ./graders/schema-check";
  if (kind === "embedding") return "POST https://api.openai.com/v1/embeddings";
  return "POST https://api.anthropic.com/v1/messages";
}

const OUTPUTS = [
  "Your refund window is thirty days from delivery.",
  "I can help with that. First, confirm the order number.",
  "The account is locked after five failed attempts.",
  "Escalating to a human agent — one moment.",
  "That plan includes unlimited seats and priority support.",
] as const;

function outputFor(i: number, shape: Shape): string {
  if (shape.huge) return "x".repeat(HUGE_OUTPUT_BYTES);
  // An `empty_reason` is only ever computed for a blank output, so a fixture
  // that carried one beside prose would describe a state the server cannot
  // produce — and would hide the "nothing to show, here is why" rendering.
  if (shape.emptyReason) return "";
  return OUTPUTS[Math.floor(rand("output", i) * OUTPUTS.length)] as string;
}

/** A `raw` payload wide enough that a naive JSON tree would stall on it. */
function deepRaw(): unknown {
  const content: Record<string, unknown> = {};
  for (let i = 0; i < 2000; i++) {
    content[`token_${i}`] = { logprob: -0.0001 * i, rank: i };
  }
  return { id: "msg_deep", model: "claude-opus-5", logprobs: content };
}

export function cacheEntryList(): CacheEntryListItem[] {
  return Array.from({ length: 180 }, (_, i) => {
    const shape = shapeOf(i);
    const created = NOW - Math.floor(rand("created", i) * 40 * DAY);
    const size = shape.huge
      ? HUGE_OUTPUT_BYTES + 512
      : 900 + Math.floor(rand("size", i) * 14_000);

    if (shape.unindexed) {
      return {
        key: keyFor(i),
        size,
        created_at: toIso(created),
        last_access_at: toIso(created + HOUR),
        entry_created_at: null,
        indexed: false,
        parseable: null,
        kind: null,
        model: null,
        cost_usd: null,
        input_tokens: null,
        output_tokens: null,
        request_summary: null,
        output_preview: null,
        empty_reason: null,
      };
    }
    if (shape.unparseable) {
      return {
        key: keyFor(i),
        size,
        created_at: toIso(created),
        last_access_at: toIso(created + HOUR),
        entry_created_at: null,
        indexed: true,
        parseable: false,
        kind: null,
        model: null,
        cost_usd: null,
        input_tokens: null,
        output_tokens: null,
        request_summary: null,
        output_preview: null,
        // Nothing was parsed, so nothing is known about why an output was
        // empty. `parseable: false` is the field that says so.
        empty_reason: null,
      };
    }

    const kind = kindFor(i, shape);
    const inputTokens = shape.noUsage ? null : 200 + Math.floor(rand("in", i) * 3000);
    const outputTokens = shape.noUsage ? null : 40 + Math.floor(rand("out", i) * 900);
    return {
      key: keyFor(i),
      size,
      created_at: toIso(created),
      last_access_at: toIso(created + Math.floor(rand("access", i) * 3 * DAY)),
      entry_created_at: toIso(created - MINUTE),
      indexed: true,
      parseable: true,
      kind,
      model: shape.preRequest ? null : modelFor(i),
      cost_usd:
        shape.noUsage || kind === "embedding"
          ? null
          : Number((rand("cost", i) * 0.09).toFixed(6)),
      input_tokens: inputTokens,
      output_tokens: outputTokens,
      request_summary: shape.preRequest ? null : requestSummaryFor(kind),
      output_preview: outputFor(i, shape).slice(0, 120),
      empty_reason: shape.emptyReason ?? null,
    } satisfies CacheEntryListItem;
  });
}

/** The detail view of one entry, keyed by its `sha256:` string. */
export function cacheEntryDetail(
  key: string,
  includeRaw: boolean,
): CacheEntryDetail | null {
  const index = cacheEntryList().findIndex((e) => e.key === key);
  if (index < 0) return null;
  const row = cacheEntryList()[index] as CacheEntryListItem;
  const shape = shapeOf(index);

  const base: CacheEntryDetail = {
    key: row.key,
    size: row.size,
    created_at: row.created_at,
    last_access_at: row.last_access_at,
    entry_created_at: row.entry_created_at,
    indexed: row.indexed,
    parseable: row.parseable,
    kind: row.kind,
    model: row.model,
    cost_usd: row.cost_usd,
    input_tokens: row.input_tokens,
    output_tokens: row.output_tokens,
    attempts: null,
    provider_latency_ms: null,
    stop_reason: null,
    empty_reason: row.empty_reason,
    domarinn_version: null,
    request: null,
    provider_fingerprint: null,
    output: null,
    reasoning: null,
    tool_calls: [],
    raw: null,
  };

  if (shape.unindexed || shape.unparseable) return base;

  const kind = row.kind ?? "provider";
  return {
    ...base,
    attempts: kind === "provider" ? 1 : null,
    provider_latency_ms: 400 + Math.floor(rand("latency", index) * 2600),
    stop_reason: kind === "provider" ? "stop" : null,
    domarinn_version: shape.preRequest ? "0.4.2" : "0.5.0",
    // Pre-0.5 entries carry one or the other, never both: the fingerprint
    // stopped being written once the request itself was recorded.
    request: shape.preRequest
      ? null
      : kind === "exec_assert"
        ? {
            transport: "exec",
            command: "./graders/schema-check",
            args: ["--strict"],
            stdin: { domarinn: { protocol: 1, kind: "assert" } },
          }
        : {
            transport: "http",
            method: "POST",
            url: "https://api.anthropic.com/v1/messages",
            body: {
              model: row.model,
              max_tokens: 1024,
              messages: [
                { role: "user", content: "How long is the refund window?" },
              ],
            },
          },
    provider_fingerprint: shape.preRequest
      ? { type: "anthropic", model: "claude-3-5-sonnet" }
      : null,
    output: outputFor(index, shape),
    reasoning:
      kind === "provider" && rand("reasoning", index) > 0.7
        ? "Checked the policy table, then the exceptions list."
        : null,
    tool_calls: [],
    raw: includeRaw
      ? shape.deepRaw
        ? deepRaw()
        : { id: `msg_${index}`, role: "assistant", stop_reason: "end_turn" }
      : null,
  };
}

/**
 * Runs that used an entry.
 *
 * Most entries link to nothing, which is the honest common case: the key is
 * only recorded by runs from a version that knew to write one, and older runs
 * cannot be backfilled.
 */
export function cacheEntryRuns(key: string): CacheEntryRunsResponse {
  const index = cacheEntryList().findIndex((e) => e.key === key);
  if (index < 0 || index % 3 !== 0) return { cases: [], next_cursor: null };
  const count = 1 + (index % 3);
  return {
    cases: Array.from({ length: count }, (_, i) => ({
      run_id: `checkout-agent-regression-${12 - i}`,
      project: "checkout-agent",
      suite: "regression",
      created_at: toIso(NOW - (i + 1) * DAY),
      case_key: `case-${index}-${i}`,
      name: `refund policy #${index}`,
      status: i === 0 ? "pass" : "fail",
      cached: i > 0,
    })),
    next_cursor: null,
  };
}

export function cacheFacets(): CacheFacetsResponse {
  const rows = cacheEntryList();
  const count = (pick: (r: CacheEntryListItem) => string | null) => {
    const tally = new Map<string, number>();
    for (const row of rows) {
      const value = pick(row);
      if (value) tally.set(value, (tally.get(value) ?? 0) + 1);
    }
    return [...tally]
      .map(([value, n]) => ({ value, count: n }))
      .sort((a, b) => b.count - a.count);
  };
  return {
    kinds: count((r) => r.kind),
    models: count((r) => r.model),
    empty_reasons: count((r) => r.empty_reason),
    total: rows.length,
    unindexed: rows.filter((r) => !r.indexed).length,
    unparseable: rows.filter((r) => r.parseable === false).length,
  };
}
