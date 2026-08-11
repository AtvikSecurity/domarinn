import { Link } from "react-router";
import type { CaseResult, FallbackAttempt } from "@/api";
import { StatusBadge } from "@/components/StatusBadge";
import { Chip } from "@/components/ui/Chip";
import { StatBlock } from "@/components/ui/StatBlock";
import { formatCost, formatLatency, formatTokens } from "@/lib/format";
import { StopReasonChip } from "./CaseDrawerSections";
import { CaseHistoryRail } from "./CaseHistoryRail";

const STATUS_TONE: Record<string, string> = {
  pass: "text-pass",
  fail: "text-fail",
  error: "text-error",
  skip: "text-skip",
  xfail: "text-xfail",
  xpass: "text-xpass",
};

/**
 * The drawer's fixed verdict band: what happened, how much it cost, and whether
 * it has always been this way.
 *
 * This replaces a single line of 12px muted text that read
 * `score 0.42 · 500 tok · $0.0012 · 1.24s`. The score is what the whole case
 * reduces to, so it is the largest thing here after the case name; the three
 * numbers become real labelled stats whose sub-lines carry the decomposition the
 * summed headline hides (a prompt-heavy case is a cost problem, an output-heavy
 * one is a truncation problem — indistinguishable when the tokens are summed).
 *
 * It sits outside the scrolling body on purpose: these are the facts you want
 * while reading the output, not facts you scroll past to reach it.
 */
export function CaseVerdictStrip({
  detail,
  project,
  suite,
  runId,
  caseKey,
}: {
  detail: CaseResult;
  project: string;
  suite: string;
  runId: string;
  caseKey: string;
}) {
  const input = detail.usage?.input_tokens ?? 0;
  const output = detail.usage?.output_tokens ?? 0;
  const cacheRead = detail.usage?.cache_read_tokens;

  return (
    <div className="shrink-0 space-y-3 border-b border-border px-5 py-3">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <StatusBadge status={detail.status} />
        <span
          className={`text-2xl font-semibold tabular-nums ${STATUS_TONE[detail.status] ?? ""}`}
        >
          {detail.score.toFixed(2)}
        </span>
        <div className="ml-auto flex flex-wrap items-center justify-end gap-1.5">
          {(detail.tags ?? []).map((t) => (
            <Chip key={t}>{t}</Chip>
          ))}
          {/* The model the provider *reported* serving, which is not always the
              one configured: an alias silently repointing to a new snapshot is
              exactly the drift this exists to make visible. Absent on runs
              stored before it was recorded. */}
          {detail.model ? (
            <Chip mono title="The model the provider reported using">
              {detail.model}
            </Chip>
          ) : null}
          {/* The other half of the runs <-> cache link. The cache browser can
              already name the runs that used an entry; this is how you get from
              a case to the entry that answered it.

              The key is recorded whether the call hit or missed — it is a
              property of the request — so the link is offered on both, and the
              chip still says which happened. A case with no key (--no-cache, a
              provider that declines caching, or any run recorded before the
              field existed) keeps the plain chip: absence is not "no cache was
              used", it is "this run never wrote it down". */}
          {detail.cache_key ? (
            <Link
              to={`/cache/entries?entry=${encodeURIComponent(detail.cache_key)}`}
              title="Open the cache entry this case was addressed by"
              className="rounded-full focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              <Chip tone="accent">{detail.cached ? "cached" : "cache entry"} →</Chip>
            </Link>
          ) : detail.cached ? (
            <Chip>cached</Chip>
          ) : null}
          {detail.attempts > 1 ? (
            <Chip tone="amber">{detail.attempts} attempts</Chip>
          ) : null}
          {/* The only field that explains a blank output. An empty answer is a
              *successful* call — nothing errored, no assertion says why — so
              without this chip the drawer shows an empty output panel and no
              account of it. Amber because it is a fault to chase, not a
              verdict; the reason set is open, so the value is shown verbatim
              rather than mapped to a friendlier phrase this build may not
              know. Absent means "not empty, or recorded before the field
              existed" — never rendered as a reassuring "none". */}
          {detail.empty_reason ? (
            <Chip
              tone="amber"
              mono
              title="Why this output had nothing gradeable in it"
            >
              empty: {detail.empty_reason}
            </Chip>
          ) : null}
          {detail.stop_reason ? (
            <StopReasonChip reason={detail.stop_reason} />
          ) : null}
        </div>
      </div>

      {/* The `expect_fail` annotation, shown whatever the outcome: it is why
          an XFail badge is calm and an XPass one is alarming. The reason is
          the author's own words; a reasonless marker still gets the sentence,
          because the annotation itself is the explanation. */}
      {detail.status === "xfail" ||
      detail.status === "xpass" ||
      detail.expect_fail_reason ? (
        <p className="text-xs text-muted">
          expected to fail
          {detail.expect_fail_reason ? `: ${detail.expect_fail_reason}` : ""}
        </p>
      ) : null}

      {/* Who really answered. Sits in the verdict band rather than among the
          chips above because the sentence is the point: a chip reading
          `reserve-mini` beside a drawer titled with the configured provider is
          exactly the ambiguity this exists to remove. */}
      {detail.answered_by_provider_id ? (
        <FallbackNotice
          answeredBy={detail.answered_by_provider_id}
          configured={detail.cell.provider_id}
          attempts={detail.fallback_attempts}
        />
      ) : null}

      <div className="grid grid-cols-3 gap-3">
        <StatBlock
          label="Tokens"
          variant="bare"
          sub={
            detail.usage ? (
              <>
                {formatTokens(input)} in · {formatTokens(output)} out
                {cacheRead != null && cacheRead > 0
                  ? ` · ${formatTokens(cacheRead)} cached`
                  : ""}
              </>
            ) : undefined
          }
        >
          {formatTokens(input + output)}
        </StatBlock>
        <StatBlock label="Cost" variant="bare">
          {formatCost(detail.cost_usd)}
        </StatBlock>
        <StatBlock label="Latency" variant="bare">
          {formatLatency(detail.latency_ms)}
        </StatBlock>
      </div>

      <CaseHistoryRail
        project={project}
        suite={suite}
        runId={runId}
        caseKey={caseKey}
      />
    </div>
  );
}

/**
 * The handoff: a `fallback:` chain answered this case, and the model that
 * produced the output below is not the one the suite configured.
 *
 * `cell.provider_id` stays the *configured* provider so `case_key` is stable
 * and an `--against` baseline still joins the same row, which means every other
 * surface in this drawer names the primary. Without this the output, the cost
 * and the verdict are all silently attributed to a provider that never ran.
 *
 * Amber, in the same border/tint treatment as the reasoning notice and the
 * `empty_reason` chip: a walked chain is the configuration working as asked,
 * not a failure — but it is not the thing you asked to measure either.
 *
 * `attempts` are the links tried and passed over *before* the answerer, in
 * configured order, and the first of them is the configured provider itself.
 * Absent (not empty) on the overwhelming majority of cases, so its own presence
 * is the render condition — never a length compared against a chain we cannot
 * see.
 */
function FallbackNotice({
  answeredBy,
  configured,
  attempts,
}: {
  answeredBy: string;
  configured: string;
  attempts: FallbackAttempt[] | undefined;
}) {
  return (
    <div className="rounded-lg border border-amber/25 bg-amber/5 p-2.5">
      <p className="text-xs font-medium text-amber">
        Answered by {answeredBy} — fallback for {configured}
      </p>
      {attempts && attempts.length > 0 ? (
        <ul className="mt-1.5 space-y-0.5">
          {attempts.map((a, i) => (
            <li
              key={`${a.provider_id}-${i}`}
              className="font-mono text-[11px] text-amber/80"
            >
              {/* Exactly one of the two is set per link (see `FallbackAttempt`);
                  the third branch is unreachable by contract and says so
                  plainly rather than rendering an empty reason. */}
              {a.provider_id}: {a.empty_reason ?? a.error_class ?? "did not answer"}
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
