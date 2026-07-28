import { useSearchParams } from "react-router";
import type { CaseListItem } from "@/api";
import {
  aggregateErrorClasses,
  errorClassLabel,
  errorClassTone,
  ERROR_NOISE_THRESHOLD,
} from "@/lib/errors";
import { mergeParams } from "@/lib/filters";
import { CollapsibleSection } from "@/components/ui/CollapsibleSection";
import { Chip } from "@/components/ui/Chip";

/** Bar segment tints, matching the chip tones. */
const FILL: Record<string, string> = {
  amber: "bg-amber/70",
  error: "bg-error/70",
};

/**
 * What a run's errors actually were.
 *
 * "14 errors" is not a finding — it is usually several different problems with
 * different owners, and reading it as one number is what makes people open
 * fourteen cases to discover none of them were about the model. Grouping by
 * class answers that in one line.
 *
 * The proportion bar is a CSS flex of tinted divs, not SVG and not a chart
 * library: it is a proportion, not a plot.
 */
export function ErrorBreakdown({
  cases,
  totalCases,
}: {
  cases: CaseListItem[];
  totalCases: number;
}) {
  const [params, setParams] = useSearchParams();
  const tallies = aggregateErrorClasses(cases, totalCases);
  if (tallies.length === 0) return null;

  const errored = tallies.reduce((n, t) => n + t.count, 0);
  const share = totalCases === 0 ? 0 : errored / totalCases;
  // A handful of errors in a large run is background; a tenth of the run is
  // the story. Only the latter opens itself.
  const defaultOpen = share > ERROR_NOISE_THRESHOLD;
  const active = params.get("error_class");

  return (
    <CollapsibleSection
      title={`Errors — ${errored} of ${totalCases} cases (${(share * 100).toFixed(1)}%)`}
      defaultOpen={defaultOpen}
    >
      <div className="flex h-2 w-full overflow-hidden rounded-full bg-surface-2">
        {tallies.map((t) => (
          <div
            key={t.class}
            className={FILL[errorClassTone(t.class)] ?? "bg-error/70"}
            style={{ width: `${(t.count / errored) * 100}%` }}
            title={`${errorClassLabel(t.class)}: ${t.count}`}
          />
        ))}
      </div>
      <ul className="mt-3 space-y-1.5">
        {tallies.map((t) => (
          <li key={t.class} className="flex items-center gap-2 text-sm">
            <Chip tone={errorClassTone(t.class)} size="xs">
              {errorClassLabel(t.class)}
            </Chip>
            <span className="tabular-nums text-muted">
              {t.count} · {((t.count / errored) * 100).toFixed(0)}%
            </span>
            <button
              type="button"
              className="ml-auto text-xs text-accent hover:underline"
              onClick={() =>
                setParams(
                  mergeParams(params, {
                    // Clicking the active class clears it, so the control is a
                    // toggle rather than a one-way trip.
                    error_class: active === t.class ? undefined : t.class,
                  }),
                  { replace: true },
                )
              }
            >
              {active === t.class ? "clear filter" : "filter"}
            </button>
          </li>
        ))}
      </ul>
    </CollapsibleSection>
  );
}
