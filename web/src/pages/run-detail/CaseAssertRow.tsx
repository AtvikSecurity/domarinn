import { Fragment, type ReactNode } from "react";
import type { AssertResult } from "@/api";
import { JsonTree, RawText, outputToString } from "@/components/output";
import { StatusBadge } from "@/components/StatusBadge";
import { Chip } from "@/components/ui/Chip";
import { cn } from "@/lib/cn";
import {
  criteriaView,
  type CriteriaBody,
  formatScore,
  formatThreshold,
  hasProseCriteria,
  verdictSource,
} from "@/lib/assertView";

/**
 * One assertion, as three attributed blocks instead of three anonymous ones.
 *
 * The row previously stacked the authored criteria, the evaluation's reason, and
 * an optional details payload with a single 11px `expects` label between them.
 * Two paragraphs of similar-looking text with no attribution invited exactly the
 * wrong reading: for an `llm-rubric`, the first is the rubric *you wrote* and the
 * second is *another model's argument* for the score it gave. Which is which
 * changes what you do next — edit the rubric, or distrust the grader.
 *
 * So every block is labelled with what it is and where it came from, the criteria
 * sit in an inset panel to mark them as verbatim configuration, and the details
 * payload collapses because it is rarely what you opened the row for.
 */
export function CaseAssertRow({
  assert,
  showWeight,
}: {
  assert: AssertResult;
  showWeight: boolean;
}) {
  const criteria = criteriaView(assert.criteria);
  const source = verdictSource(assert.kind);
  const threshold = formatThreshold(criteria?.threshold ?? null);
  const reason = assert.reason?.trim();
  const prose = hasProseCriteria(assert.kind);

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
      <div className="flex flex-wrap items-center gap-2">
        <StatusBadge status={assert.status} size="xs" />
        <Chip mono className="text-fg">
          {assert.kind}
        </Chip>
        {criteria?.negated ? (
          <Chip tone="amber" title="This assertion passes when the condition does NOT hold">
            negated
          </Chip>
        ) : null}
        {/* A cached grader verdict means the assertion did NOT re-run. Editing
            an llm-rubric and re-running otherwise looks like it took effect
            when it didn't — this is a correctness signal, not a nicety. */}
        {assert.cached ? (
          <Chip
            tone="neutral"
            size="xs"
            title="This verdict was served from cache — the assertion did not re-run"
          >
            cached
          </Chip>
        ) : null}

        {/* The score, and the bar it had to clear. `score 0.95` on its own never
            said whether that was a pass. */}
        <span className="ml-auto flex items-baseline gap-1.5 text-xs tabular-nums">
          <span className="text-muted">score</span>
          <span className="font-semibold text-fg">{formatScore(assert.score)}</span>
          {threshold ? <span className="text-muted/80">{threshold}</span> : null}
          {showWeight ? (
            <span className="text-muted/80">· weight {assert.weight.toFixed(2)}</span>
          ) : null}
        </span>
      </div>

      {criteria?.body ? (
        <LabeledBlock label="Criteria" hint="as authored in your suite">
          <div className="rounded-md border border-border/60 bg-bg/50 p-2.5">
            <CriteriaBodyView body={criteria.body} prose={prose} />
          </div>
        </LabeledBlock>
      ) : null}

      {reason ? (
        <LabeledBlock label={source.label} hint={source.hint}>
          <p className="break-words text-sm text-fg/90">{reason}</p>
        </LabeledBlock>
      ) : null}

      {assert.details !== undefined && assert.details !== null ? (
        // A native disclosure rather than the app's CollapsibleSection: this is
        // a payload nested inside an already-open section, not a peer heading,
        // and it should not add another "…" heading to the drawer's outline.
        <details className="mt-2.5 group">
          <summary className="cursor-pointer list-none text-[11px] font-semibold uppercase tracking-wide text-muted/80 hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring rounded">
            <span className="inline-flex items-center gap-1">
              <svg
                width="12"
                height="12"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2.5"
                className="transition-transform group-open:rotate-90"
                aria-hidden
              >
                <path d="M9 6l6 6-6 6" />
              </svg>
              Details
              <span className="font-normal normal-case tracking-normal text-muted/70">
                raw evaluation payload
              </span>
            </span>
          </summary>
          <div className="mt-1.5">
            {typeof assert.details === "object" ? (
              <JsonTree data={assert.details} className="text-[11px]/relaxed" />
            ) : (
              <RawText
                text={outputToString(assert.details)}
                wrap
                className="text-[11px]/relaxed"
              />
            )}
          </div>
        </details>
      ) : null}
    </div>
  );
}

/** The criteria body, rendered per its resolved kind. */
function CriteriaBodyView({
  body,
  prose,
}: {
  body: CriteriaBody;
  prose: boolean;
}) {
  switch (body.kind) {
    case "scalar":
      return prose ? (
        // Authored line breaks are structure in a rubric, so they are preserved
        // rather than collapsed into one paragraph.
        <p className="whitespace-pre-wrap break-words text-[13px]/relaxed text-fg/90">
          {body.text}
        </p>
      ) : (
        // Character-exact criteria: a substring, regex or expression, where
        // whitespace and punctuation are part of the assertion.
        <code className="block whitespace-pre-wrap break-words font-mono text-[11px]/relaxed text-fg/90">
          {body.text}
        </code>
      );
    case "pairs":
      // The field names are the point here: a `tokens` criterion showing a bare
      // "4000" states a number and withholds what it means.
      return (
        <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 font-mono text-[11px]/relaxed">
          {body.pairs.map(([key, value]) => (
            <Fragment key={key}>
              <dt className="text-muted">{key}</dt>
              <dd className="break-words text-fg/90">{value}</dd>
            </Fragment>
          ))}
        </dl>
      );
    case "json":
      return <JsonTree data={body.data} className="text-[11px]/relaxed" />;
  }
}

/**
 * A block with a name and, where the source could be mistaken, a note saying
 * who produced it.
 */
function LabeledBlock({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <div className="mt-2.5">
      <div className="mb-1 text-[11px] font-semibold uppercase tracking-wide text-muted/80">
        {/* The label keeps its own element so an exact-text query still matches
            it once the hint is appended — same reason CollapsibleSection wraps
            its `title` separately from `meta`. */}
        <span>{label}</span>
        {hint ? (
          <span className="ml-1.5 font-normal normal-case tracking-normal text-muted/70">
            {hint}
          </span>
        ) : null}
      </div>
      {children}
    </div>
  );
}
