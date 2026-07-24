import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { Link, useParams, useSearchParams } from "react-router";
import type { OnChangeFn, SortingState } from "@tanstack/react-table";
import {
  useMatrix,
  useRun,
  useRunCases,
  useRuns,
  useSetBaseline,
} from "@/api/queries";
import { mergeParams, parseCaseFilters } from "@/lib/filters";
import { parseSort, serializeSort } from "@/lib/sort";
import { distinctProviders, distinctPrompts } from "@/lib/matrix";
import { previousRun } from "@/lib/compare";
import {
  formatCost,
  formatDate,
  formatDuration,
  formatInt,
  formatTokens,
} from "@/lib/format";
import { PassRateBadge } from "@/components/PassRateBadge";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { ErrorState, EmptyState } from "@/components/States";
import { Button } from "@/components/ui/Button";
import { SegmentedControl } from "@/components/ui/SegmentedControl";
import { Tooltip } from "@/components/ui/Tooltip";
import { cn } from "@/lib/cn";
import { useFillViewport } from "@/components/AppShell";
import { CaseGrid } from "./run-detail/CaseGrid";
import { CaseDrawer } from "./run-detail/CaseDrawer";
import { MatrixView } from "./run-detail/MatrixView";

const STATUS_CHIPS: { value: string; label: string }[] = [
  { value: "", label: "All" },
  { value: "pass", label: "Pass" },
  { value: "fail", label: "Fail" },
  { value: "error", label: "Error" },
  { value: "skip", label: "Skip" },
];

export function RunDetail() {
  const { id = "" } = useParams();
  const [params, setParams] = useSearchParams();
  const filters = parseCaseFilters(params);

  const run = useRun(id);
  const casesQ = useRunCases(id, filters);
  // Provider/prompt axes come from the matrix endpoint's columns (the complete,
  // authoritative set). Chips + grid columns appear only for a run with >1
  // distinct provider (resp. prompt); while the matrix loads, both stay hidden.
  const matrix = useMatrix(id);
  const providerValues = useMemo(
    () => distinctProviders(matrix.data),
    [matrix.data],
  );
  const promptValues = useMemo(() => distinctPrompts(matrix.data), [matrix.data]);
  const showProvider = providerValues.length > 1;
  const showPrompt = promptValues.length > 1;
  // A run is "matrix-shaped" when it spans more than one provider or prompt —
  // only then is the List | Matrix toggle offered. `?view=matrix` deep-loaded on
  // a single-provider run silently falls back to the list.
  const matrixShaped = showProvider || showPrompt;
  const viewMode: "list" | "matrix" =
    matrixShaped && filters.view === "matrix" ? "matrix" : "list";
  // The case grid is unavoidably its own scroller (horizontal overflow forces
  // a vertical scrollport), so the shell must not scroll behind it.
  useFillViewport();
  const baseline = useSetBaseline(run.data?.project ?? "", run.data?.suite ?? "");
  // Scoped to the run that was actually pinned: `isSuccess` alone never resets,
  // so the button kept reading "Baseline set ✓" after navigating to a sibling
  // run whose baseline had not been set at all.
  const justSetBaseline =
    baseline.isSuccess && baseline.variables === run.data?.id;
  useEffect(() => {
    if (!baseline.isSuccess) return;
    const t = setTimeout(() => baseline.reset(), 2500);
    return () => clearTimeout(t);
  }, [baseline]);
  // Sibling runs in the same project/suite, used only to resolve a default
  // compare target (the immediately older run) for the header's Compare
  // button — see `previousRun`.
  const suiteRuns = useRuns({
    project: run.data?.project ?? undefined,
    suite: run.data?.suite ?? undefined,
  });

  const cases = useMemo(
    () => casesQ.data?.pages.flatMap((p) => p.cases) ?? [],
    [casesQ.data],
  );

  // Debounced output search -> ?q=
  const [search, setSearch] = useState(filters.q ?? "");
  // Reset the search box when navigating to a different run. Done during
  // render (the "adjusting state when props change" pattern) instead of in an
  // effect, so no stale-search frame is committed.
  const [prevId, setPrevId] = useState(id);
  if (prevId !== id) {
    setPrevId(id);
    setSearch(filters.q ?? "");
  }
  useEffect(() => {
    const handle = setTimeout(() => {
      if ((filters.q ?? "") !== search) {
        setParams(mergeParams(params, { q: search || undefined }), {
          replace: true,
        });
      }
    }, 300);
    return () => clearTimeout(handle);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [search]);

  // Client-side sort of the loaded cases, encoded in `?sort=`.
  const sorting = useMemo(() => parseSort(params.get("sort")), [params]);
  const onSortingChange = useCallback<OnChangeFn<SortingState>>(
    (updater) => {
      const next = typeof updater === "function" ? updater(sorting) : updater;
      setParams(mergeParams(params, { sort: serializeSort(next) }), {
        replace: true,
      });
    },
    [params, setParams, sorting],
  );

  function setStatus(value: string) {
    setParams(mergeParams(params, { status: value || undefined }), {
      replace: true,
    });
  }
  function setCached(value: string) {
    setParams(mergeParams(params, { cached: value || undefined }), {
      replace: true,
    });
  }
  function setProvider(value: string) {
    setParams(mergeParams(params, { provider: value || undefined }), {
      replace: true,
    });
  }
  function setPrompt(value: string) {
    setParams(mergeParams(params, { prompt: value || undefined }), {
      replace: true,
    });
  }
  function setView(value: "list" | "matrix") {
    setParams(
      mergeParams(params, { view: value === "matrix" ? "matrix" : undefined }),
      { replace: true },
    );
  }
  // `replace`, like every other param mutation above: clicking through ten
  // cases otherwise buries the runs list under twenty Back presses. The drawer
  // has its own explicit exits (Esc, ✕, overlay), and the history rail's squares
  // are real cross-run links that still push.
  function selectCase(caseKey: string) {
    setParams(mergeParams(params, { case: caseKey }), { replace: true });
  }
  function closeCase() {
    setParams(mergeParams(params, { case: undefined }), { replace: true });
  }

  if (run.isPending) return <CenteredSpinner label="Loading run…" />;
  if (run.isError) return <ErrorState error={run.error} onRetry={() => run.refetch()} />;

  const r = run.data;
  const cacheKnown = r.cache_hits != null && r.cache_misses != null;
  const cacheTotal = (r.cache_hits ?? 0) + (r.cache_misses ?? 0);
  // Skips are not a field on the run response; they are the remainder. Clamped
  // because a v1 blob could in principle disagree with itself.
  const skipCount = Math.max(
    0,
    r.case_count - r.pass_count - r.fail_count - r.error_count,
  );
  // The fresh/cached case filter only means something when the run actually
  // mixes both; on a fully-fresh or fully-cached run it would be a no-op chip.
  const partiallyCached = (r.cache_hits ?? 0) > 0 && (r.cache_misses ?? 0) > 0;
  const siblingRuns = suiteRuns.data?.pages.flatMap((p) => p.runs) ?? [];
  // Older run in the suite = the default compare base for this run.
  // Undefined when `r` is the oldest loaded run in its suite — the real
  // server has no target-less compare route, so the button is hidden.
  const compareTarget = previousRun(siblingRuns, r.id);

  return (
    // A flex column inside the filled shell: the header and filters take their
    // natural height and the grid gets whatever is left, so it is sized to the
    // viewport rather than to a guessed `vh` fraction.
    <div className="flex flex-col gap-5 lg:min-h-0 lg:flex-1">
      {/* Summary header */}
      <div className="shrink-0 rounded-xl border border-border bg-surface p-4">
        <div className="flex flex-wrap items-start gap-4">
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <Link to="/" className="text-sm text-muted hover:text-fg">
                Runs
              </Link>
              <span className="text-muted">/</span>
              <span className="text-sm text-muted">{r.project}</span>
              <span className="text-muted">/</span>
              <span className="text-sm text-muted">{r.suite}</span>
            </div>
            <h1 className="mt-0.5 font-mono text-lg font-semibold tracking-tight">
              {r.id}
            </h1>
            <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted">
              <span>{formatDate(r.created_at)}</span>
              {r.git_branch ? (
                <span className="font-mono">
                  {r.git_branch}
                  {r.git_commit ? `@${r.git_commit}` : ""}
                </span>
              ) : null}
              {r.ci_run_url ? (
                <a
                  href={r.ci_run_url}
                  target="_blank"
                  rel="noreferrer"
                  className="text-accent hover:underline"
                >
                  CI run ↗
                </a>
              ) : null}
            </div>
          </div>

          <div className="ml-auto flex items-center gap-2">
            <Button
              variant="secondary"
              size="sm"
              onClick={() => baseline.mutate(r.id)}
              disabled={baseline.isPending}
            >
              {justSetBaseline
                ? "Baseline set ✓"
                : baseline.isError
                  ? "Could not set — retry"
                  : "Set baseline"}
            </Button>
            {compareTarget ? (
              <Link
                to={`/runs/${encodeURIComponent(compareTarget.id)}/compare/${encodeURIComponent(r.id)}`}
              >
                <Button variant="primary" size="sm">
                  Compare
                </Button>
              </Link>
            ) : (
              // A `title` on a disabled button never appears — disabled controls
              // suppress pointer events — so the only explanation for why this
              // is dead was unreachable. The wrapper receives the events.
              <Tooltip content="No earlier run in this suite to compare against">
                <span tabIndex={0} className="inline-flex rounded-md">
                  <Button variant="primary" size="sm" disabled>
                    Compare
                  </Button>
                </span>
              </Tooltip>
            )}
          </div>
        </div>

        <div
          className={`mt-4 grid grid-cols-2 gap-3 sm:grid-cols-3 ${cacheKnown ? "lg:grid-cols-7" : "lg:grid-cols-6"}`}
        >
          <Stat label="Pass rate">
            <PassRateBadge
              pass={r.pass_count}
              fail={r.fail_count}
              error={r.error_count}
              className="text-sm"
            />
          </Stat>
          <Stat label="Cases">{r.case_count}</Stat>
          <Stat
            label="Pass / Fail / Err"
            // The Skip filter chip promised a state the header never counted.
            // It is not a field on the response — it is what is left over.
            sub={skipCount > 0 ? `${formatInt(skipCount)} skipped` : undefined}
          >
            <span className="tabular-nums">
              <span className="text-pass">{r.pass_count}</span> /{" "}
              <span className="text-fail">{r.fail_count}</span> /{" "}
              <span className="text-error">{r.error_count}</span>
            </span>
          </Stat>
          <Stat
            label="Tokens"
            // Prompt-heavy is a cost problem; output-heavy is a truncation
            // risk. Summed, they look identical.
            sub={`${formatTokens(r.prompt_tokens)} in · ${formatTokens(r.completion_tokens)} out`}
          >
            {formatTokens(r.prompt_tokens + r.completion_tokens)}
          </Stat>
          <Stat label="Cost">{formatCost(r.cost_usd)}</Stat>
          <Stat label="Duration">{formatDuration(r.duration_ms)}</Stat>
          {cacheKnown ? (
            <Stat
              label="Cache"
              // The raw counts used to live only in a native `title`. They are
              // the actionable part — a percentage doesn't tell you how much
              // work the run actually did.
              sub={`${formatInt(r.cache_hits)} hit · ${formatInt(r.cache_misses)} miss`}
            >
              <span className="tabular-nums">
                {cacheTotal > 0
                  ? `${Math.round(((r.cache_hits ?? 0) / cacheTotal) * 100)}% cached`
                  : "—"}
              </span>
            </Stat>
          ) : null}
        </div>
      </div>

      {/* Filters */}
      <div className="flex shrink-0 flex-wrap items-center gap-3">
        {/* List | Matrix toggle — only for matrix-shaped runs. Stays visible in
            both modes so the user can switch back. */}
        {matrixShaped ? (
          <SegmentedControl
            ariaLabel="View"
            options={[
              { value: "list", label: "List" },
              { value: "matrix", label: "Matrix" },
            ]}
            value={viewMode}
            onChange={setView}
          />
        ) : null}

        {/* The status/provider/prompt chips and search filter the LIST only, so
            they are hidden in matrix mode (the matrix already shows every axis). */}
        {viewMode === "list" ? (
          <>
            <ChipGroup
              label="Status"
              chips={STATUS_CHIPS}
              active={filters.status ?? ""}
              onSelect={setStatus}
            />

            {/* Fresh/cached response filter — only for runs that mix both. */}
            {partiallyCached ? (
              <ChipGroup
                label="Cached"
                chips={[
                  { value: "", label: "All" },
                  { value: "false", label: "Fresh only" },
                ]}
                active={filters.cached ?? ""}
                onSelect={setCached}
              />
            ) : null}

            {/* Provider / prompt chips appear only for matrix-shaped runs (>1
                distinct value each); nothing renders while the matrix loads. */}
            {showProvider ? (
              <ChipGroup
                label="Provider"
                chips={toChips(providerValues)}
                active={filters.provider ?? ""}
                onSelect={setProvider}
              />
            ) : null}
            {showPrompt ? (
              <ChipGroup
                label="Prompt"
                chips={toChips(promptValues)}
                active={filters.prompt ?? ""}
                onSelect={setPrompt}
              />
            ) : null}

            <div className="ml-auto">
              <input
                type="search"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder="Search name / output…"
                aria-label="Search cases"
                className="h-8 w-56 rounded-md border border-border bg-surface px-3 text-sm outline-none focus:ring-2 focus:ring-ring"
              />
            </div>
          </>
        ) : (
          <span className="text-xs text-muted">
            Every provider and prompt is shown; status filters and search apply to
            the list view.
          </span>
        )}
      </div>

      {/* Body: matrix pivot or the virtualized list. Both own their scrolling
          within the height the shell hands them. */}
      <div className="flex flex-col lg:min-h-0 lg:flex-1">
      {viewMode === "matrix" ? (
        <MatrixView runId={id} onSelectCase={selectCase} />
      ) : casesQ.isPending ? (
        <CenteredSpinner label="Loading cases…" />
      ) : casesQ.isError ? (
        <ErrorState error={casesQ.error} onRetry={() => casesQ.refetch()} />
      ) : cases.length === 0 ? (
        <EmptyState title="No cases match these filters" />
      ) : (
        <CaseGrid
          cases={cases}
          assertLabels={r.assert_labels}
          showProvider={showProvider}
          showPrompt={showPrompt}
          selectedKey={filters.case}
          onSelect={selectCase}
          sorting={sorting}
          onSortingChange={onSortingChange}
          // While placeholder pages from the previous run are showing, their
          // cursor is meaningless for this run — don't let the grid page on it.
          hasNextPage={casesQ.hasNextPage && !casesQ.isPlaceholderData}
          fetchNextPage={casesQ.fetchNextPage}
          isFetchingNextPage={casesQ.isFetchingNextPage}
          // Only meaningful as a total while unfiltered; the grid decides how
          // to phrase the count from this plus `hasNextPage`.
          totalCount={r.case_count}
          // `isPending` is true only on the very first load, so without this a
          // filter change leaves the previous run's rows fully opaque and
          // clickable until the new page lands.
          busy={casesQ.isPlaceholderData}
        />
      )}
      </div>

      <CaseDrawer
        runId={id}
        project={r.project ?? ""}
        suite={r.suite ?? ""}
        caseKey={filters.case}
        onClose={closeCase}
      />
    </div>
  );
}

/** Build an "All" reset chip plus one chip per value. */
function toChips(values: string[]): { value: string; label: string }[] {
  return [{ value: "", label: "All" }, ...values.map((v) => ({ value: v, label: v }))];
}

/** A single-select segmented chip group backed by a URL param. Mirrors the
 *  status chips' look; the `label` names the group for assistive tech (and the
 *  E2E, which scopes to `role="group"` by name). */
function ChipGroup({
  label,
  chips,
  active,
  onSelect,
}: {
  label: string;
  chips: { value: string; label: string }[];
  active: string;
  onSelect: (value: string) => void;
}) {
  return (
    <div
      role="group"
      aria-label={label}
      className="flex items-center gap-1 rounded-lg border border-border bg-surface p-0.5"
    >
      {chips.map((chip) => {
        const selected = active === chip.value;
        return (
          <button
            key={chip.value}
            // Selection was conveyed by background colour alone: nothing in the
            // accessibility tree, and nothing that survives grayscale or
            // forced-colors. `role="group"` stays — several specs scope to it.
            aria-pressed={selected}
            onClick={() => onSelect(chip.value)}
            className={cn(
              "rounded-md px-2.5 py-1 text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
              selected
                ? "bg-surface-2 text-fg ring-1 ring-inset ring-border-strong"
                : "text-muted hover:text-fg",
            )}
          >
            {chip.label}
          </button>
        );
      })}
    </div>
  );
}

function Stat({
  label,
  children,
  sub,
}: {
  label: string;
  children: ReactNode;
  sub?: ReactNode;
}) {
  return (
    <div className="rounded-lg border border-border bg-bg/40 px-3 py-2">
      <div className="text-[11px] font-medium uppercase tracking-wide text-muted">
        {label}
      </div>
      <div className="mt-0.5 text-sm font-semibold tabular-nums">{children}</div>
      {sub ? (
        <div className="mt-0.5 text-[11px] tabular-nums text-muted">{sub}</div>
      ) : null}
    </div>
  );
}
