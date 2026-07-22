import type { CaseResult, RenderedPrompt } from "@/api";
import { hash, pick, rand, round2 } from "./rng";
import { NOUNS, VERBS } from "./suites";
import { RUN_META_BY_ID, type RunMeta } from "./runMeta";
import { detailAsserts, fullOutput, generateCases, type MockCaseRow } from "./cases";

// The one fixture suite whose cases carry schema-v2 case-detail fields
// (rendered prompt, provider stop reason, raw provider metadata). Every other
// suite stays v1-shaped, so its drawer degrades to the pre-v2 form — and the
// prompt-drawer E2E has both a v2 target (a run of this suite) and a clean v1
// target (the money run). Not referenced by any drawer-opening E2E constant.
const V2_SUITE_KEY = "support-bot/tone-and-safety";

/**
 * Schema-v2 case-detail fields for the {@link V2_SUITE_KEY} suite; `{}` (fully
 * v1-shaped) for every other suite. Skipped cases were never executed, so they
 * also stay v1 — nothing was sent to or returned by a provider. Deterministic
 * per case seed: ~1 in 5 cases carry a flattened text prompt, the rest a
 * role-tagged system + user pair; ~1 in 4 non-error generations truncate
 * (`max_tokens`) rather than finishing cleanly (`end_turn`); errored ones carry
 * no clean stop reason.
 */
function v2Fields(
  meta: RunMeta,
  row: MockCaseRow,
): Pick<CaseResult, "prompt" | "stop_reason" | "raw"> {
  if (meta.suiteKey !== V2_SUITE_KEY || row.status === "skip") return {};

  const seed = row.seed;
  const noun = pick(NOUNS, "noun", seed);
  const verb = pick(VERBS, "verb", seed);

  const systemText =
    "You are a customer-support assistant. Follow the tone-and-safety policy: " +
    "stay empathetic, never disclose PII, and refuse toxic or unsafe requests.";
  const userText =
    `A customer needs help with a ${noun}. Respond per policy; the assistant ` +
    `${verb} the request and replies with clear next steps.`;

  const asText = rand(meta.suiteKey, seed, "promptstyle") < 0.2;
  const prompt: RenderedPrompt = asText
    ? { text: `[system]\n${systemText}\n\n[user]\n${userText}` }
    : {
        messages: [
          { role: "system", content: systemText },
          { role: "user", content: userText },
        ],
      };

  const stop_reason =
    row.status === "error"
      ? undefined
      : rand(meta.suiteKey, seed, "stop") < 0.25
        ? "max_tokens"
        : "end_turn";

  const raw = {
    id: `msg_${hash(meta.suiteKey, seed, "rawid").toString(16)}`,
    model: "gpt-4o-mini",
    provider: row.provider_id,
    finish_reason: stop_reason ?? "error",
    usage: {
      input_tokens: row.prompt_tokens,
      output_tokens: row.completion_tokens,
    },
    system_fingerprint: `fp_${hash(meta.suiteKey, seed, "fp").toString(16).slice(0, 8)}`,
  };

  return { prompt, stop_reason, raw };
}

export function caseDetail(runId: string, caseKey: string): CaseResult | undefined {
  const meta = RUN_META_BY_ID.get(runId);
  if (!meta) return undefined;
  const row = generateCases(runId).find((c) => c.case_key === caseKey);
  if (!row) return undefined;
  const asserts = detailAsserts(meta, row.seed, row.status, row.asserts);
  const score =
    asserts.length === 0
      ? row.status === "pass"
        ? 1
        : 0
      : round2(asserts.reduce((sum, a) => sum + a.score, 0) / asserts.length);
  return {
    cell: {
      provider_id: row.provider_id,
      // `CellKey.prompt_id` is optional (omitted, not null) — send it only when
      // the case actually carries a prompt dimension.
      ...(row.prompt_id != null ? { prompt_id: row.prompt_id } : {}),
      test_id: row.test_id,
      repeat: row.repeat,
    },
    case_key: row.case_key,
    name: row.name,
    tags: row.tags,
    status: row.status,
    score,
    output: fullOutput(meta, row.seed, row.status),
    ...v2Fields(meta, row),
    asserts,
    usage: { input_tokens: row.prompt_tokens, output_tokens: row.completion_tokens },
    cost_usd: row.cost_usd,
    latency_ms: row.latency_ms,
    cached: false,
    attempts: row.status === "error" ? 3 : 1,
    error:
      row.status === "error"
        ? "upstream provider returned HTTP 502 (Bad Gateway) after 3 retries."
        : undefined,
  };
}
