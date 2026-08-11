import { useState, type ReactNode } from "react";
import { useSearchParams } from "react-router";
import { useProjects, useSuites } from "@/api/queries";
import {
  activeRunsFilterCount,
  mergeParams,
  parseRunsFilters,
  RUNS_FILTER_KEYS,
} from "@/lib/filters";
import { CHROME_FRAME } from "@/components/ui/chrome";
import { cn } from "@/lib/cn";
import { resolveCached } from "@/lib/cached";
import { setCachedPref, useCachedPref } from "@/lib/cachedPref";
import type { CachedFilter } from "@/api";
import { Button } from "./ui/Button";
import { SegmentedControl } from "./ui/SegmentedControl";

const controlCls =
  "h-8 rounded-md border border-border bg-surface px-2 text-sm text-fg outline-none focus:ring-2 focus:ring-ring";

function epochToDate(v: string | undefined): string {
  if (!v) return "";
  const d = new Date(Number(v));
  if (Number.isNaN(d.getTime())) return "";
  return d.toISOString().slice(0, 10);
}

function Chevron({ open }: { open: boolean }) {
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      className={cn("text-muted transition-transform", open && "rotate-180")}
      aria-hidden
    >
      <path d="M6 9l6 6 6-6" />
    </svg>
  );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-[11px] font-medium uppercase tracking-wide text-muted">
        {label}
      </span>
      {children}
    </label>
  );
}

export function RunsFilterBar() {
  const [params, setParams] = useSearchParams();
  const [mobileOpen, setMobileOpen] = useState(false);
  const filters = parseRunsFilters(params);
  const projects = useProjects();
  const suites = useSuites(filters.project);
  const cachedPref = useCachedPref();

  function patch(next: Record<string, string | undefined>) {
    // Changing a filter resets pagination and, when project changes, suite.
    setParams(mergeParams(params, next), { replace: true });
  }

  const activeCount = activeRunsFilterCount(params);

  const statusOptions = [
    { value: "", label: "Any status" },
    { value: "pass", label: "All passing" },
    { value: "fail", label: "Has failures" },
    { value: "error", label: "Has errors" },
  ];

  const cachedOptions: readonly { value: CachedFilter; label: string }[] = [
    { value: "exclude", label: "Hidden" },
    { value: "all", label: "Shown" },
    { value: "only", label: "Only" },
  ];

  return (
    <div className={cn(CHROME_FRAME, "p-3")}>
      {/* Nine controls wrap into two tidy rows on a desktop and into nine
          stacked ones on a phone, where they filled the entire first screen
          and pushed every run below the fold. Collapsed there by default, and
          only there — at `md` and up the fields are always shown and this
          toggle does not exist. */}
      <button
        type="button"
        aria-expanded={mobileOpen}
        aria-controls="runs-filters"
        onClick={() => setMobileOpen((v) => !v)}
        className="flex w-full items-center justify-between rounded-md text-sm font-medium text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring md:hidden"
      >
        <span>
          Filters
          {activeCount > 0 ? (
            // Named in the toggle so a narrowed list is never unexplained
            // while the controls that narrowed it are out of sight.
            <span className="ml-1.5 text-muted">· {activeCount} active</span>
          ) : null}
        </span>
        <Chevron open={mobileOpen} />
      </button>

      <div
        id="runs-filters"
        className={cn(
          "flex-wrap items-end gap-3 md:flex",
          mobileOpen ? "mt-3 flex" : "hidden",
        )}
      >
        <Field label="Project">
          <select
            className={controlCls}
            value={filters.project ?? ""}
            onChange={(e) => patch({ project: e.target.value || undefined, suite: undefined })}
          >
            <option value="">All projects</option>
            {projects.data?.projects.map((p) => (
              <option key={p.project} value={p.project}>
                {p.project} ({p.run_count})
              </option>
            ))}
          </select>
        </Field>

        <Field label="Suite">
          <select
            className={controlCls}
            value={filters.suite ?? ""}
            onChange={(e) => patch({ suite: e.target.value || undefined })}
            disabled={!filters.project}
          >
            <option value="">All suites</option>
            {suites.data?.suites.map((s) => (
              <option key={s.suite} value={s.suite}>
                {s.suite}
              </option>
            ))}
          </select>
        </Field>

        <Field label="Tag">
          <input
            className={controlCls}
            placeholder="e.g. nightly"
            defaultValue={filters.tag ?? ""}
            key={`tag-${filters.tag ?? ""}`}
            onKeyDown={(e) => {
              if (e.key === "Enter")
                patch({ tag: (e.target as HTMLInputElement).value || undefined });
            }}
            onBlur={(e) => patch({ tag: e.target.value || undefined })}
          />
        </Field>

        <Field label="Branch">
          <input
            className={controlCls}
            placeholder="e.g. main"
            defaultValue={filters.branch ?? ""}
            key={`branch-${filters.branch ?? ""}`}
            onKeyDown={(e) => {
              if (e.key === "Enter")
                patch({ branch: (e.target as HTMLInputElement).value || undefined });
            }}
            onBlur={(e) => patch({ branch: e.target.value || undefined })}
          />
        </Field>

        {/* The facet that separates the canonical CI stream from developer
            iteration. A segmented control rather than a select: three mutually
            exclusive states, all worth seeing at once. */}
        <Field label="Origin">
          <SegmentedControl
            ariaLabel="Origin"
            value={filters.origin ?? "all"}
            onChange={(v) => patch({ origin: v === "all" ? undefined : v })}
            options={[
              { value: "all", label: "All" },
              { value: "ci", label: "CI" },
              { value: "local", label: "Local" },
            ]}
          />
        </Field>

        <Field label="Actor">
          <input
            className={controlCls}
            placeholder="e.g. alice"
            defaultValue={filters.actor ?? ""}
            key={`actor-${filters.actor ?? ""}`}
            onKeyDown={(e) => {
              if (e.key === "Enter")
                patch({ actor: (e.target as HTMLInputElement).value || undefined });
            }}
            onBlur={(e) => patch({ actor: e.target.value || undefined })}
          />
        </Field>

        <Field label="Since">
          <input
            type="date"
            className={controlCls}
            value={epochToDate(filters.since)}
            onChange={(e) =>
              patch({
                since: e.target.value
                  ? String(Date.parse(`${e.target.value}T00:00:00Z`))
                  : undefined,
              })
            }
          />
        </Field>

        <Field label="Until">
          <input
            type="date"
            className={controlCls}
            value={epochToDate(filters.until)}
            onChange={(e) =>
              patch({
                until: e.target.value
                  ? String(Date.parse(`${e.target.value}T23:59:59Z`))
                  : undefined,
              })
            }
          />
        </Field>

        <Field label="Status">
          <select
            className={controlCls}
            value={filters.status ?? ""}
            onChange={(e) => patch({ status: e.target.value || undefined })}
          >
            {statusOptions.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </Field>

        {/* Three mutually exclusive states, worth seeing at once — the same
            reasoning as Origin above, which this used to disagree with by
            being a select.

            Unlike every other control here, this one is not only a filter:
            it also stores the choice as the standing preference, so the suite
            pages, search and the case drawer stop hiding (or start hiding)
            cached runs too. It writes the URL as well, so the view it produces
            stays shareable and beats whatever preference the recipient holds. */}
        <Field label="Cached runs">
          <SegmentedControl<CachedFilter>
            ariaLabel="Cached runs"
            value={resolveCached(filters.cached, cachedPref)}
            onChange={(v) => {
              setCachedPref(v);
              patch({ cached: v });
            }}
            options={cachedOptions}
          />
        </Field>

        {activeCount > 0 ? (
          <Button
            variant="ghost"
            size="sm"
            className="mb-0.5"
            // Derived from RUNS_FILTER_KEYS rather than listed by hand: the count
            // above already iterates that array, so a hand-maintained list here
            // drifts into counting filters it cannot clear.
            onClick={() =>
              setParams(
                mergeParams(
                  params,
                  Object.fromEntries(RUNS_FILTER_KEYS.map((k) => [k, undefined])),
                ),
                { replace: true },
              )
            }
          >
            Clear {activeCount} filter{activeCount > 1 ? "s" : ""}
          </Button>
        ) : null}
      </div>
    </div>
  );
}
