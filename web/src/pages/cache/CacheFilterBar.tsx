import type { ReactNode } from "react";
import type { CacheFacetsResponse } from "@/api";
import { Button } from "@/components/ui/Button";
import { CACHE_FILTER_KEYS, type CacheFilters } from "@/lib/filters";

const controlCls =
  "h-8 rounded-md border border-border bg-surface px-2 text-sm text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring";

/** Keys the "Clear filters" button resets — derived, so it cannot drift from
 *  the count beside it. */
const CLEARABLE = CACHE_FILTER_KEYS.filter(
  (key) => key !== "tier" && key !== "entry" && key !== "sort",
);

export interface CacheFilterBarProps {
  filters: CacheFilters;
  facets?: CacheFacetsResponse;
  activeCount: number;
  onPatch: (patch: Record<string, string | undefined>) => void;
  /** Debounced search box value, owned by the page. */
  search: string;
  onSearch: (value: string) => void;
  /** True on a tier with no full-text index — say so rather than let the box
   *  imply a capability it does not have. */
  substringOnly?: boolean;
}

export function CacheFilterBar({
  filters,
  facets,
  activeCount,
  onPatch,
  search,
  onSearch,
  substringOnly = false,
}: CacheFilterBarProps) {
  const clearAll = () =>
    onPatch(Object.fromEntries(CLEARABLE.map((k) => [k, undefined])));

  return (
    <div className="flex shrink-0 flex-wrap items-end gap-3 rounded-xl border border-border bg-surface/60 p-3">
      <Field label="Search">
        <input
          type="search"
          className={`${controlCls} w-56`}
          placeholder={
            substringOnly ? "substring match" : "request or output text"
          }
          value={search}
          onChange={(e) => onSearch(e.target.value)}
        />
      </Field>

      <Field label="Kind">
        <select
          className={controlCls}
          value={filters.kind ?? ""}
          onChange={(e) => onPatch({ kind: e.target.value || undefined })}
        >
          <option value="">All</option>
          {/* Only the kinds this store actually holds — an empty facet is a
              filter that can only ever return nothing. */}
          {facets?.kinds.map((k) => (
            <option key={k.value} value={k.value}>
              {k.value} ({k.count})
            </option>
          ))}
          {/* Two states rather than kinds. Without them these rows are
              unreachable exactly while the backfill is running, which is when
              someone is most likely to be looking for them. */}
          {facets && facets.unindexed > 0 ? (
            <option value="unindexed">not indexed yet ({facets.unindexed})</option>
          ) : null}
          {facets && facets.unparseable > 0 ? (
            <option value="unparseable">unreadable ({facets.unparseable})</option>
          ) : null}
        </select>
      </Field>

      <Field label="Model">
        <select
          className={controlCls}
          value={filters.model ?? ""}
          onChange={(e) => onPatch({ model: e.target.value || undefined })}
        >
          <option value="">All</option>
          {facets?.models.map((m) => (
            <option key={m.value} value={m.value}>
              {m.value} ({m.count})
            </option>
          ))}
        </select>
      </Field>

      {/* Only when the store holds one. An empty-output reason is rare by
          construction, and an always-present dropdown reading "All" over
          nothing would suggest a dimension this cache does not have. */}
      {facets && facets.empty_reasons.length > 0 ? (
        <Field label="Empty output">
          <select
            className={controlCls}
            value={filters.empty_reason ?? ""}
            onChange={(e) =>
              onPatch({ empty_reason: e.target.value || undefined })
            }
          >
            <option value="">All</option>
            {facets.empty_reasons.map((r) => (
              <option key={r.value} value={r.value}>
                {r.value} ({r.count})
              </option>
            ))}
          </select>
        </Field>
      ) : null}

      <Field label="From">
        <input
          type="date"
          className={controlCls}
          value={toDateInput(filters.since)}
          onChange={(e) => onPatch({ since: fromDateInput(e.target.value) })}
        />
      </Field>

      <Field label="To">
        <input
          type="date"
          className={controlCls}
          value={toDateInput(filters.until)}
          onChange={(e) => onPatch({ until: fromDateInput(e.target.value) })}
        />
      </Field>

      {activeCount > 0 ? (
        <Button variant="ghost" size="sm" onClick={clearAll}>
          Clear {activeCount} filter{activeCount === 1 ? "" : "s"}
        </Button>
      ) : null}
    </div>
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

/** URL carries epoch-ms (what the server accepts); the input wants `yyyy-mm-dd`. */
function toDateInput(ms: string | undefined): string {
  if (!ms) return "";
  const n = Number(ms);
  if (!Number.isFinite(n)) return "";
  return new Date(n).toISOString().slice(0, 10);
}

function fromDateInput(value: string): string | undefined {
  if (!value) return undefined;
  const ms = Date.parse(`${value}T00:00:00Z`);
  return Number.isFinite(ms) ? String(ms) : undefined;
}
