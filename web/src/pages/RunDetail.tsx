import { useCallback, useEffect, useMemo, useState } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router";
import type { OnChangeFn, SortingState } from "@tanstack/react-table";
import {
  useMatrix,
  useRun,
  useRunCases,
  useRuns,
  useSetBaseline,
  useDeleteRun,
} from "@/api/queries";
import { mergeParams, parseCaseFilters } from "@/lib/filters";
import { parseSort, serializeSort } from "@/lib/sort";
import { distinctProviders, distinctPrompts } from "@/lib/matrix";
import { previousRun } from "@/lib/compare";
import { listNeighbors } from "@/lib/listNeighbors";
import {
  formatCost,
  formatDate,
  formatDuration,
  formatInt,
  formatTokens,
  shortRunId,
} from "@/lib/format";
import { setsPath, suitePath } from "@/lib/routes";
import { PassRateBadge } from "@/components/PassRateBadge";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { ErrorState, EmptyState } from "@/components/States";
import { Button } from "@/components/ui/Button";
import { Card } from "@/components/ui/Card";
import { Modal, ModalActions } from "@/components/ui/Modal";
import { StatBlock } from "@/components/ui/StatBlock";
import { Breadcrumb } from "@/components/ui/Breadcrumb";
import { useAuthView } from "@/auth/AuthProvider";
import { SegmentedControl } from "@/components/ui/SegmentedControl";
import { Tooltip } from "@/components/ui/Tooltip";
import {
  TAB_ITEM_BASE,
  TAB_ITEM_IDLE,
  TAB_ITEM_SELECTED,
} from "@/components/ui/chrome";
import { cn } from "@/lib/cn";
import { useFillViewport } from "@/components/AppShell";
import { CaseGrid } from "./run-detail/CaseGrid";
import { CaseDrawer } from "./run-detail/CaseDrawer";
import { ErrorBreakdown } from "./run-detail/ErrorBreakdown";
import { MatrixView } from "./run-detail/MatrixView";

const STATUS_CHIPS: { value: string; label: string }[] = [
  { value: "", label: "All" },
  { value: "pass", label: "Pass" },
  { value: "fail", label: "Fail" },
  { value: "error", label: "Error" },
  { value: "skip", label: "Skip" },
  { value: "xfail", label: "XFail" },
  { value: "xpass", label: "XPass" },
];

/** Header stat-row widths, keyed by how many stats the run actually has.
 *
 * Enumerated rather than interpolated because Tailwind scans for literal class
 * names — a computed `lg:grid-cols-${n}` produces no CSS at all, which fails as
 * a silent fallback to the two-column base rather than as a build error.
 */
const STAT_COLUMNS: Record<number, string> = {
  6: "lg:grid-cols-6",
  7: "lg:grid-cols-7",
  8: "lg:grid-cols-8",
  9: "lg:grid-cols-9",
};

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
  const auth = useAuthView();
  const deleteRun = useDeleteRun();
  // Its own query, unfiltered by whatever the grid is showing: the breakdown
  // describes the *run*, so it must not change when someone narrows the grid.
  const erroredCases = useRunCases(id, { status: "error" });
  const [confirmDelete, setConfirmDelete] = useState(false);
  const navigate = useNavigate();
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
  //
  // `cached: "all"` explicitly, and deliberately not the user's preference:
  // this is a navigation-target resolver, not a list. Hiding cached runs here
  // would not remove noise from anything a reader can see — it would make
  // "Compare with previous" land on a run that is not the previous one, which
  // is a wrong answer rather than a quieter one. This used to inherit the
  // hidden default by passing no `cached` at all.
  const suiteRuns = useRuns({
    project: run.data?.project ?? undefined,
    suite: run.data?.suite ?? undefined,
    cached: "all",
  });

  const cases = useMemo(
    () => casesQ.data?.pages.flatMap((p) => p.cases) ?? [],
    [casesQ.data],
  );

  // The rows either side of the open case, so the drawer can step without
  // closing. Scoped to loaded rows — see `lib/listNeighbors`.
  const {
    prevKey: prevCaseKey,
    nextKey: nextCaseKey,
    position: casePosition,
  } = useMemo(
    () => listNeighbors(cases, filters.case, (c) => c.case_key),
    [cases, filters.case],
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
  // Cases whose output had nothing gradeable in it, tallied by reason. Not a
  // verdict bucket: an empty case still lands in pass/fail/error, so this
  // rides on Cases rather than beside them — a run can honestly report
  // "300 passed" and "4 empty" at once when only metric assertions applied.
  //
  // Omitted, never `{}` or `0`, when there is nothing to report — and absent
  // covers both "no empty cases" and "this run predates the column", which is
  // why zero renders as no sub-line at all instead of a reassuring "0 empty".
  const emptyByReason = Object.entries(r.empty_counts ?? {}).sort(([a], [b]) =>
    a.localeCompare(b),
  );
  const emptyCount = emptyByReason.reduce((sum, [, n]) => sum + n, 0);
  // The fresh/cached case filter only means something when the run actually
  // mixes both; on a fully-fresh or fully-cached run it would be a no-op chip.
  const partiallyCached = (r.cache_hits ?? 0) > 0 && (r.cache_misses ?? 0) > 0;
  // Provider-side prompt caching. Null for runs stored before the columns
  // existed, which is not the same as zero — either way there is nothing to
  // show, so both collapse to "hide the stat".
  const promptCacheTokens = (r.cache_read_tokens ?? 0) + (r.cache_write_tokens ?? 0);
  const showGraderCost = r.grader_cost_usd != null;
  const showPromptCache = promptCacheTokens > 0;
  // Six unconditional stats plus whichever of the three optional ones this run
  // has data for. Derived rather than hardcoded: the column count used to be a
  // literal that mirrored the JSX below, and adding a stat silently wrapped the
  // row — costing the cases grid a whole row of height, which only the app-shell
  // scroll e2e caught.
  const statColumns =
    STAT_COLUMNS[
      6 + Number(showGraderCost) + Number(cacheKnown) + Number(showPromptCache)
    ];
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
      <Card className="shrink-0">
        <div className="flex flex-wrap items-start gap-4">
          <div className="min-w-0">
            {/* The run id is the trail's last crumb, and that is a fix rather
                than a flourish: `Breadcrumb` marks its final item
                `aria-current="page"`, so ending at the suite told a screen
                reader you were standing on the suite while you were in fact
                standing on a run. Terminating at the run makes the claim true
                and frees the suite to be a link.

                Both crumbs need `project`, since a set is addressed by the
                pair; `||` rather than `??` so an empty name is rejected too,
                and a run that declared neither renders exactly as before —
                `Breadcrumb` already draws a crumb without a `to` as inert
                muted text. */}
            <Breadcrumb
              items={[
                { label: "Runs", to: "/runs" },
                {
                  label: r.project || "—",
                  to: r.project ? setsPath(r.project) : undefined,
                },
                {
                  label: r.suite || "—",
                  to:
                    r.project && r.suite
                      ? suitePath(r.project, r.suite)
                      : undefined,
                },
                { label: shortRunId(r.id) },
              ]}
            />
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
              {/* Who produced it, on the same line as when and from where.
                  `actor` is who ran it, `uploaded_by` who pushed it; a shared
                  CI token names nobody, so both are shown when they differ. */}
              {r.actor ? <span>{r.actor}</span> : null}
              {r.host ? <span className="font-mono">@{r.host}</span> : null}
              {r.uploaded_by && r.uploaded_by !== r.actor ? (
                <span>uploaded by {r.uploaded_by}</span>
              ) : null}
              {r.domarinn_version ? (
                <span className="font-mono">v{r.domarinn_version}</span>
              ) : null}
              {r.note ? <span className="italic">“{r.note}”</span> : null}
            </div>
          </div>

          <div className="ml-auto flex items-center gap-2">
            {/* Admin-only, and behind a confirm: retention is off by default,
                so without this a run pushed by mistake was permanent. */}
            {auth.canAdmin ? (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setConfirmDelete(true)}
                disabled={deleteRun.isPending}
              >
                Delete run
              </Button>
            ) : null}
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

        <div className={`mt-4 grid grid-cols-2 gap-3 sm:grid-cols-3 ${statColumns}`}>
          <StatBlock variant="bare" label="Pass rate">
            <PassRateBadge
              pass={r.pass_count}
              fail={r.fail_count}
              error={r.error_count}
              xpass={r.xpass_count}
              className="text-sm"
            />
          </StatBlock>
          <StatBlock
            variant="bare"
            label="Cases"
            // The breakdown rides in a title rather than the sub-line: a run
            // with four reasons would wrap the stat box, and the count is the
            // part you scan for.
            sub={
              emptyCount > 0 ? (
                <span
                  title={emptyByReason
                    .map(([reason, n]) => `${reason} × ${n}`)
                    .join(", ")}
                >
                  {formatInt(emptyCount)} empty
                </span>
              ) : undefined
            }
          >
            {r.case_count}
          </StatBlock>
          <StatBlock
            variant="bare"
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
          </StatBlock>
          <StatBlock
            variant="bare"
            label="Tokens"
            // Prompt-heavy is a cost problem; output-heavy is a truncation
            // risk. Summed, they look identical.
            sub={`${formatTokens(r.prompt_tokens)} in · ${formatTokens(r.completion_tokens)} out`}
          >
            {formatTokens(r.prompt_tokens + r.completion_tokens)}
          </StatBlock>
          <StatBlock
            variant="bare"
            label="Cost"
            // What the run *avoided* only means something beside the number it
            // is a fraction of, so it rides on this stat rather than getting
            // its own. Actual spend is the difference.
            sub={
              r.cache_savings_usd != null && r.cache_savings_usd > 0
                ? `${formatCost(r.cache_savings_usd)} saved by cache`
                : undefined
            }
          >
            {formatCost(r.cost_usd)}
          </StatBlock>
          {/* Its own stat, never added to Cost above. Cost is what the systems
              under test cost — the number a `cost:` assertion budgets. This is
              what measuring them cost, and on a suite judged by a bigger model
              than it tests it is the larger of the two. Presence-gated: a
              suite with no model-graded assertions has nothing to report, and
              a run stored before the column existed reports null, which is not
              the same as zero. */}
          {showGraderCost ? (
            <StatBlock variant="bare" label="Grading" sub="not included in cost">
              {formatCost(r.grader_cost_usd)}
            </StatBlock>
          ) : null}
          <StatBlock variant="bare" label="Duration">{formatDuration(r.duration_ms)}</StatBlock>
          {cacheKnown ? (
            <StatBlock
            variant="bare"
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
            </StatBlock>
          ) : null}
          {/* Provider-side prompt caching, which is a different mechanism from
              domarinn's own response cache above and dominates spend on a
              cache-heavy suite. Shown only when the run reported any, so it
              stays absent for suites that never enabled it. */}
          {showPromptCache ? (
            <StatBlock
            variant="bare"
              label="Prompt cache"
              sub={`${formatTokens(r.cache_write_tokens)} written`}
            >
              {formatTokens(r.cache_read_tokens)} read
            </StatBlock>
          ) : null}
        </div>
      </Card>

      {/* Only when there is something to break down, and fetched separately
          from the grid: the grid is filtered and paginated, so counting its
          loaded rows would report whatever happens to be on screen. */}
      {r.error_count > 0 ? (
        <div className="shrink-0">
          <ErrorBreakdown
            cases={erroredCases.data?.pages.flatMap((p) => p.cases) ?? []}
            totalCases={r.case_count}
          />
        </div>
      ) : null}

      {/* Filters */}
      {/* `gap-6` between groups against `gap-1` inside one: the filter groups
          used to be told apart by their own borders, and once those went the
          only thing separating "Skip" from the next group's "All" was 4px more
          than separates it from "Error". Spacing is what carries the grouping
          now, so it has to be unambiguous. */}
      <div className="flex shrink-0 flex-wrap items-center gap-6">
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
        onPrev={prevCaseKey === undefined ? undefined : () => selectCase(prevCaseKey)}
        onNext={nextCaseKey === undefined ? undefined : () => selectCase(nextCaseKey)}
        position={casePosition}
      />

      <Modal
        open={confirmDelete}
        onOpenChange={setConfirmDelete}
        title="Delete this run?"
        description={
          <>
            <span className="font-mono">{r.id}</span> and all of its cases will
            be removed permanently. Eval results cannot be recreated — a rerun
            produces different outputs.
          </>
        }
      >
        {deleteRun.isError ? (
          <p className="text-sm text-fail">Could not delete the run. Try again.</p>
        ) : null}
        <ModalActions
          onCancel={() => setConfirmDelete(false)}
          onConfirm={() =>
            deleteRun.mutate(r.id, {
              // Only navigate once it is actually gone; leaving the page on a
              // failed delete would imply it worked.
              onSuccess: () => {
                setConfirmDelete(false);
                void navigate("/");
              },
            })
          }
          confirmLabel="Delete run"
          confirmVariant="danger"
          pending={deleteRun.isPending}
        />
      </Modal>
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
    <div role="group" aria-label={label} className="flex items-center gap-1">
      {chips.map((chip) => {
        const selected = active === chip.value;
        return (
          <button
            key={chip.value}
            // Selection was conveyed by background colour alone: nothing in the
            // accessibility tree, and nothing that survives grayscale or
            // forced-colors. `role="group"` stays — several specs scope to it,
            // and these are independent filters rather than a single choice, so
            // the radiogroup `SegmentedControl` claims would be wrong here even
            // though the two now look alike.
            aria-pressed={selected}
            onClick={() => onSelect(chip.value)}
            className={cn(
              TAB_ITEM_BASE,
              "px-2.5 pb-1 text-sm",
              selected ? TAB_ITEM_SELECTED : TAB_ITEM_IDLE,
            )}
          >
            {chip.label}
          </button>
        );
      })}
    </div>
  );
}
