import type { ReactNode } from "react";
import { useSearchParams } from "react-router";
import { useProjects, useSuites } from "@/api/queries";
import {
  activeRunsFilterCount,
  mergeParams,
  parseRunsFilters,
} from "@/lib/filters";
import { Button } from "./ui/Button";

const controlCls =
  "h-8 rounded-md border border-border bg-surface px-2 text-sm text-fg outline-none focus:ring-2 focus:ring-ring";

function epochToDate(v: string | undefined): string {
  if (!v) return "";
  const d = new Date(Number(v));
  if (Number.isNaN(d.getTime())) return "";
  return d.toISOString().slice(0, 10);
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
  const filters = parseRunsFilters(params);
  const projects = useProjects();
  const suites = useSuites(filters.project);

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

  return (
    <div className="flex flex-wrap items-end gap-3 rounded-xl border border-border bg-surface/60 p-3">
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

      {activeCount > 0 ? (
        <Button
          variant="ghost"
          size="sm"
          className="mb-0.5"
          onClick={() =>
            setParams(
              mergeParams(params, {
                project: undefined,
                suite: undefined,
                tag: undefined,
                branch: undefined,
                since: undefined,
                until: undefined,
                status: undefined,
              }),
              { replace: true },
            )
          }
        >
          Clear {activeCount} filter{activeCount > 1 ? "s" : ""}
        </Button>
      ) : null}
    </div>
  );
}
