import { useRef, useState } from "react";
import { useNavigate } from "react-router";
import type { CaseSearchHit, ProjectSetView, RunSearchHit } from "@/api";
import { useSearch, useSetSearch } from "@/api/queries";
import { useDebouncedValue } from "@/lib/useDebouncedValue";
import { formatInt, formatRelative, isoFromEpoch, shortRunId } from "@/lib/format";
import { runPath, setsPath } from "@/lib/routes";
import { cn } from "@/lib/cn";
import { Chip } from "./ui/Chip";
import { Snippet } from "./Snippet";
import { StatusBadge } from "./StatusBadge";

/** Top hits shown in the dropdown per group; Enter opens the full /search page. */
const DROPDOWN_LIMIT = 5;

/**
 * Fewer than the server groups get. A set row is one line where run and case
 * rows are two, and the panel is height-capped — five of them would push the
 * full-text results below the fold in exactly the case where a query matched
 * both a project name and its contents.
 */
const SETS_DROPDOWN_LIMIT = 3;

type Hit =
  | { kind: "set"; project: ProjectSetView }
  | { kind: "run"; run: RunSearchHit }
  | { kind: "case"; case: CaseSearchHit };

function hrefFor(hit: Hit): string {
  switch (hit.kind) {
    case "set":
      return setsPath(hit.project.project);
    case "run":
      return runPath(hit.run.id);
    case "case":
      return `${runPath(hit.case.run_id)}?case=${encodeURIComponent(hit.case.case_key)}`;
  }
}

function keyFor(hit: Hit): string {
  switch (hit.kind) {
    case "set":
      return `s-${hit.project.project}`;
    case "run":
      return `r-${hit.run.id}`;
    case "case":
      return `c-${hit.case.run_id}-${hit.case.case_key}`;
  }
}

/**
 * Header search box: sets by name, plus full-text over runs (project, suite,
 * branch, commit, tags) and cases (name, prompt, output, error, tags). A
 * debounced dropdown shows the top hits per group; Enter with no selection
 * opens the full `/search?q=…` page.
 */
export function SearchBar({
  variant = "header",
  onNavigate,
}: {
  /**
   * `header` is the desktop bar: hidden below `md`, panel floating over the
   * page. `sheet` is the same component inside the phone menu, where it is
   * full width and the panel sits in flow so it scrolls with the sheet.
   *
   * A prop rather than a headless hook plus two shells: the two differ only in
   * three class strings, while the debounce, two queries, group assembly and
   * keyboard handling are exactly what there must not be two copies of.
   */
  variant?: "header" | "sheet";
  /** Called after a hit is opened, so a containing sheet can close itself. */
  onNavigate?: () => void;
} = {}) {
  const [value, setValue] = useState("");
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const navigate = useNavigate();
  const rootRef = useRef<HTMLDivElement>(null);

  const debounced = useDebouncedValue(value, 200);
  const search = useSearch(debounced, { limit: DROPDOWN_LIMIT, enabled: open });
  // Debounced too, not raw: the client filter could answer per keystroke, but
  // then the panel would show sets for "checkou" beside runs for "check" and
  // as a whole would be answering no single question.
  const setSearch = useSetSearch(debounced, {
    enabled: open,
    limit: SETS_DROPDOWN_LIMIT,
  });

  // Sets first. Partly because a name match is a stronger statement of intent
  // than a full-text hit — but mainly because sets resolve synchronously from
  // cache while server hits arrive later, so putting them first means a late
  // response only ever *appends*, and the indices the arrow keys are already
  // sitting on never shift underneath.
  const groups = [
    {
      key: "set" as const,
      label: "Sets",
      hits: setSearch.matches.map((project): Hit => ({ kind: "set", project })),
    },
    {
      key: "run" as const,
      label: "Runs",
      hits: (search.data?.runs ?? []).map((run): Hit => ({ kind: "run", run })),
    },
    {
      key: "case" as const,
      label: "Cases",
      hits: (search.data?.cases ?? []).map(
        (c): Hit => ({ kind: "case", case: c }),
      ),
    },
  ].filter((g) => g.hits.length > 0);

  // Derived from the groups rather than assembled alongside them, so the flat
  // list the keyboard indexes cannot drift out of step with what is rendered.
  const hits: Hit[] = groups.flatMap((g) => g.hits);
  const showPanel = open && value.trim().length > 0;

  // `useSearch` keeps previous data, so a narrowing query can shrink the list
  // out from under a selection. Clamp on read instead of resetting in an
  // effect: that keeps a still-valid selection through a refetch, and stops
  // Enter ever opening a row that is no longer there.
  const activeIndex = active < hits.length ? active : -1;

  // A cold `/sets` cache would otherwise render "No matches." and then flip to
  // a list — retracting a definite negative answer, which is worse than a
  // moment of "Searching…".
  const pending = search.isPending || setSearch.isPending;

  function close() {
    setOpen(false);
    setActive(-1);
  }
  function go(hit: Hit) {
    close();
    onNavigate?.();
    void navigate(hrefFor(hit));
  }
  function goFullPage() {
    const q = value.trim();
    if (!q) return;
    close();
    onNavigate?.();
    void navigate(`/search?q=${encodeURIComponent(q)}`);
  }
  function onKeyDown(e: React.KeyboardEvent) {
    if (e.key === "Escape") {
      // Just closes the panel. Deliberately no `stopPropagation`: inside the
      // phone menu the competing listener is Radix's, which is registered on
      // `document` in the capture phase and has therefore already run by the
      // time this fires. Keeping one Escape to one dismissal is the sheet's
      // job, via `onEscapeKeyDown` — see MobileNavSheet.
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
      const hit = activeIndex >= 0 ? hits[activeIndex] : undefined;
      if (hit) go(hit);
      else goFullPage();
    }
  }

  return (
    <div
      ref={rootRef}
      className={cn(
        "relative min-w-0",
        variant === "header"
          ? "hidden flex-1 justify-center md:flex"
          : "flex w-full flex-col",
      )}
      onBlur={(e) => {
        // Close only when focus leaves the bar AND its dropdown.
        if (!rootRef.current?.contains(e.relatedTarget)) close();
      }}
    >
      <div className={variant === "header" ? "w-full max-w-md" : "w-full"}>
        <input
          type="search"
          role="combobox"
          aria-expanded={showPanel}
          aria-controls="global-search-results"
          aria-label="Search sets, runs and cases"
          placeholder="Search projects, prompts, outputs…"
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
            className={cn(
              "overflow-hidden rounded-lg border border-border bg-surface shadow-xl",
              variant === "header"
                ? "absolute left-1/2 top-10 z-40 w-full max-w-md -translate-x-1/2"
                : // In flow inside the sheet, so it scrolls with the menu
                  // instead of floating over it off-screen.
                  "relative mt-2 w-full",
            )}
          >
            {pending ? (
              <p className="px-3 py-2.5 text-sm text-muted">Searching…</p>
            ) : hits.length === 0 ? (
              <p className="px-3 py-2.5 text-sm text-muted">No matches.</p>
            ) : (
              <div className="max-h-96 overflow-y-auto py-1">
                {groups.map((group, gi) => {
                  // Where this group starts in the flat list the arrow keys
                  // walk. Counted from the groups themselves rather than
                  // peeked from the previous row's kind, which is what the
                  // old two-group rendering did and would not survive here.
                  const offset = groups
                    .slice(0, gi)
                    .reduce((n, g) => n + g.hits.length, 0);
                  return (
                    <ul key={group.key}>
                      <li>
                        <GroupLabel>{group.label}</GroupLabel>
                      </li>
                      {group.hits.map((hit, i) => {
                        const index = offset + i;
                        return (
                          <li key={keyFor(hit)}>
                            <button
                              type="button"
                              data-search-hit={hit.kind}
                              // Keep focus in the input so onBlur doesn't
                              // close the panel before the click lands.
                              onMouseDown={(e) => e.preventDefault()}
                              onClick={() => go(hit)}
                              className={cn(
                                "block w-full px-3 py-2 text-left text-sm hover:bg-surface-2",
                                index === activeIndex && "bg-surface-2",
                              )}
                            >
                              {hit.kind === "set" ? (
                                <SetHitRow project={hit.project} />
                              ) : hit.kind === "run" ? (
                                <RunHitRow hit={hit.run} />
                              ) : (
                                <CaseHitRow hit={hit.case} />
                              )}
                            </button>
                          </li>
                        );
                      })}
                    </ul>
                  );
                })}
              </div>
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

function SetHitRow({ project }: { project: ProjectSetView }) {
  // `last_run_at` is epoch-ms here, unlike the RFC3339 strings the older
  // /projects DTOs carry — `formatRelative` on the raw number renders "-".
  const last = isoFromEpoch(project.last_run_at);
  return (
    <span className="flex min-w-0 items-center gap-2">
      {/* Project names are words, not ids — no font-mono. */}
      <span className="truncate text-xs font-medium">{project.project}</span>
      <span className="shrink-0 text-xs text-muted">
        {formatInt(project.suite_count)} suite
        {project.suite_count === 1 ? "" : "s"} · {formatInt(project.run_count)}{" "}
        run{project.run_count === 1 ? "" : "s"}
        {last ? ` · ${formatRelative(last)}` : ""}
      </span>
      {project.restricted ? (
        <Chip tone="amber" size="xs">
          restricted
        </Chip>
      ) : null}
    </span>
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
