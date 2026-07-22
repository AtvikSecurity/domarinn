import { useRef, useState } from "react";
import { useNavigate } from "react-router";
import type { CaseSearchHit, RunSearchHit } from "@/api";
import { useSearch } from "@/api/queries";
import { useDebouncedValue } from "@/lib/useDebouncedValue";
import { shortRunId } from "@/lib/format";
import { cn } from "@/lib/cn";
import { Snippet } from "./Snippet";
import { StatusBadge } from "./StatusBadge";

/** Top hits shown in the dropdown per group; Enter opens the full /search page. */
const DROPDOWN_LIMIT = 5;

type Hit =
  | { kind: "run"; run: RunSearchHit }
  | { kind: "case"; case: CaseSearchHit };

function hrefFor(hit: Hit): string {
  return hit.kind === "run"
    ? `/runs/${encodeURIComponent(hit.run.id)}`
    : `/runs/${encodeURIComponent(hit.case.run_id)}?case=${encodeURIComponent(hit.case.case_key)}`;
}

/**
 * Header search box: full-text over runs (project, suite, branch, commit,
 * tags) and cases (name, prompt, output, error, tags). A debounced dropdown
 * shows the top hits per group; Enter with no selection opens the full
 * `/search?q=…` page.
 */
export function SearchBar() {
  const [value, setValue] = useState("");
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const navigate = useNavigate();
  const rootRef = useRef<HTMLDivElement>(null);

  const debounced = useDebouncedValue(value, 200);
  const search = useSearch(debounced, { limit: DROPDOWN_LIMIT, enabled: open });

  const hits: Hit[] = [
    ...(search.data?.runs ?? []).map((run): Hit => ({ kind: "run", run })),
    ...(search.data?.cases ?? []).map((c): Hit => ({ kind: "case", case: c })),
  ];
  const showPanel = open && value.trim().length > 0;

  function close() {
    setOpen(false);
    setActive(-1);
  }
  function go(hit: Hit) {
    close();
    void navigate(hrefFor(hit));
  }
  function goFullPage() {
    const q = value.trim();
    if (!q) return;
    close();
    void navigate(`/search?q=${encodeURIComponent(q)}`);
  }
  function onKeyDown(e: React.KeyboardEvent) {
    if (e.key === "Escape") {
      close();
      return;
    }
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (hits.length === 0) return;
      const delta = e.key === "ArrowDown" ? 1 : -1;
      setActive((i) => (i + delta + hits.length + 1) % (hits.length + 1) - 1);
      return;
    }
    if (e.key === "Enter") {
      const hit = active >= 0 ? hits[active] : undefined;
      if (hit) go(hit);
      else goFullPage();
    }
  }

  return (
    <div
      ref={rootRef}
      className="relative hidden min-w-0 flex-1 justify-center md:flex"
      onBlur={(e) => {
        // Close only when focus leaves the bar AND its dropdown.
        if (!rootRef.current?.contains(e.relatedTarget)) close();
      }}
    >
      <div className="w-full max-w-md">
        <input
          type="search"
          role="combobox"
          aria-expanded={showPanel}
          aria-controls="global-search-results"
          aria-label="Search runs and cases"
          placeholder="Search prompts, outputs, branches…"
          value={value}
          onChange={(e) => {
            setValue(e.target.value);
            setOpen(true);
            setActive(-1);
          }}
          onFocus={() => setOpen(true)}
          onKeyDown={onKeyDown}
          className="h-8 w-full rounded-md border border-border bg-surface px-3 text-sm outline-none placeholder:text-muted/70 focus:ring-2 focus:ring-ring"
        />

        {showPanel ? (
          <div
            id="global-search-results"
            className="absolute left-1/2 top-10 z-40 w-full max-w-md -translate-x-1/2 overflow-hidden rounded-lg border border-border bg-surface shadow-xl"
          >
            {search.isPending ? (
              <p className="px-3 py-2.5 text-sm text-muted">Searching…</p>
            ) : hits.length === 0 ? (
              <p className="px-3 py-2.5 text-sm text-muted">No matches.</p>
            ) : (
              <ul className="max-h-96 overflow-y-auto py-1">
                {hits.map((hit, i) => (
                  <li key={hit.kind === "run" ? `r-${hit.run.id}` : `c-${hit.case.run_id}-${hit.case.case_key}`}>
                    {i === 0 && hit.kind === "run" ? <GroupLabel>Runs</GroupLabel> : null}
                    {hit.kind === "case" && (i === 0 || hits[i - 1]?.kind === "run") ? (
                      <GroupLabel>Cases</GroupLabel>
                    ) : null}
                    <button
                      type="button"
                      data-search-hit={hit.kind}
                      // Keep focus in the input so onBlur doesn't close the
                      // panel before the click lands.
                      onMouseDown={(e) => e.preventDefault()}
                      onClick={() => go(hit)}
                      className={cn(
                        "block w-full px-3 py-2 text-left text-sm hover:bg-surface-2",
                        i === active && "bg-surface-2",
                      )}
                    >
                      {hit.kind === "run" ? (
                        <RunHitRow hit={hit.run} />
                      ) : (
                        <CaseHitRow hit={hit.case} />
                      )}
                    </button>
                  </li>
                ))}
              </ul>
            )}
            <button
              type="button"
              onMouseDown={(e) => e.preventDefault()}
              onClick={goFullPage}
              className="block w-full border-t border-border px-3 py-2 text-left text-xs text-accent hover:bg-surface-2"
            >
              See all results ↵
            </button>
          </div>
        ) : null}
      </div>
    </div>
  );
}

function GroupLabel({ children }: { children: string }) {
  return (
    <div className="px-3 pb-1 pt-2 text-[10px] font-semibold uppercase tracking-wide text-muted">
      {children}
    </div>
  );
}

function RunHitRow({ hit }: { hit: RunSearchHit }) {
  return (
    <span className="flex min-w-0 flex-col gap-0.5">
      <span className="flex items-center gap-2">
        <span className="truncate font-mono text-xs">{shortRunId(hit.id)}</span>
        <span className="truncate text-xs text-muted">
          {hit.project ?? "—"} / {hit.suite ?? "—"}
        </span>
      </span>
      <Snippet text={hit.snippet} className="line-clamp-1 text-xs text-muted" />
    </span>
  );
}

function CaseHitRow({ hit }: { hit: CaseSearchHit }) {
  return (
    <span className="flex min-w-0 flex-col gap-0.5">
      <span className="flex items-center gap-2">
        <StatusBadge status={hit.status} size="xs" />
        <span className="truncate text-xs font-medium">{hit.name ?? hit.case_key}</span>
        <span className="truncate text-xs text-muted">
          {hit.project ?? "—"} / {hit.suite ?? "—"}
        </span>
      </span>
      <Snippet text={hit.snippet} className="line-clamp-1 text-xs text-muted" />
    </span>
  );
}
