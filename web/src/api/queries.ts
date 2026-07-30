import {
  keepPreviousData,
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { apiRequest } from "./client";
import type {
  ApiKeyCreatedResponse,
  ApiKeyListResponse,
  AuthScope,
  CacheEntryDetail,
  CacheEntryListResponse,
  CacheFacetsResponse,
  CacheStatsResponse,
  CaseHistoryResponse,
  CaseListResponse,
  CaseResult,
  CompareResponse,
  MatrixResponse,
  MetaResponse,
  MeResponse,
  ProjectsResponse,
  PruneResponse,
  Role,
  RunConfigResponse,
  RunDetailResponse,
  RunListResponse,
  SearchResponse,
  SuitesResponse,
  UserListResponse,
  UserView,
} from "@/api";
import {
  cacheRequestFilters,
  runsRequestFilters,
  type CacheFilters,
  type CacheRequestFilters,
  type CaseFilters,
  type RunsFilters,
} from "@/lib/filters";

export const qk = {
  meta: ["meta"] as const,
  me: ["auth", "me"] as const,
  apiKeys: ["apikeys"] as const,
  users: ["users"] as const,
  runs: (filters: RunsFilters) => ["runs", filters] as const,
  run: (id: string) => ["run", id] as const,
  cases: (id: string, filters: CaseFilters) => ["cases", id, filters] as const,
  caseDetail: (id: string, caseKey: string) =>
    ["case", id, caseKey] as const,
  caseHistory: (project: string, suite: string, caseKey: string, limit: number) =>
    ["caseHistory", project, suite, caseKey, limit] as const,
  matrix: (id: string) => ["matrix", id] as const,
  matrixAll: (id: string) => ["matrix", id, "all"] as const,
  compare: (id: string, other?: string) => ["compare", id, other ?? null] as const,
  runConfig: (id: string) => ["runConfig", id] as const,
  search: (q: string, limit: number) => ["search", q, limit] as const,
  projects: ["projects"] as const,
  suites: (project: string) => ["suites", project] as const,
  cacheStats: ["cache", "stats"] as const,
  // Everything cache-related nests under ["cache"], so pruning can invalidate
  // the whole subtree in one call — see `usePruneCache`.
  cacheEntries: (filters: CacheRequestFilters) =>
    ["cache", "entries", filters] as const,
  cacheEntry: (key: string, raw: boolean) =>
    ["cache", "entry", key, raw] as const,
  cacheFacets: ["cache", "facets"] as const,
};

export function useMeta() {
  return useQuery({
    queryKey: qk.meta,
    queryFn: () => apiRequest<MetaResponse>("/meta"),
    staleTime: Infinity,
  });
}

export function useMe() {
  return useQuery({
    queryKey: qk.me,
    queryFn: ({ signal }) =>
      apiRequest<MeResponse>("/auth/me", { signal, skipAuthRedirect: true }),
    staleTime: 15_000,
  });
}

export function useRuns(filters: RunsFilters) {
  // URL state → request params: the hidden-by-default `cached` mapping lives
  // in `runsRequestFilters`; keying the query on the mapped value keeps the
  // cache identity equal to the request identity.
  const request = runsRequestFilters(filters);
  return useInfiniteQuery({
    queryKey: qk.runs(request),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam, signal }) =>
      apiRequest<RunListResponse>("/runs", {
        params: { ...request, limit: 50, cursor: pageParam },
        signal,
      }),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
}

export function useRun(id: string, opts: { enabled?: boolean } = {}) {
  const enabled = (opts.enabled ?? true) && !!id;
  return useQuery({
    queryKey: qk.run(id),
    queryFn: ({ signal }) =>
      apiRequest<RunDetailResponse>(`/runs/${encodeURIComponent(id)}`, { signal }),
    enabled,
    // Run-to-run navigation (history squares, compare links) re-renders
    // RunDetail with a new id; without a placeholder the page — and the open
    // case drawer inside it — unmounts into a full-page spinner and remounts.
    // Keep the previous run on screen and swap in place.
    placeholderData: keepPreviousData,
  });
}

export function useRunCases(id: string, filters: CaseFilters) {
  // Only the server-side filters go to the API; `case` (the drawer selection),
  // `sort` (client-side grid ordering), and `view` (list/matrix toggle) are
  // client-only — stripping them here keeps them out of both the request and the
  // query key, so sorting, opening the drawer, or switching views never triggers
  // a refetch.
  const { case: _case, sort: _sort, view: _view, ...serverFilters } = filters;
  return useInfiniteQuery({
    queryKey: qk.cases(id, serverFilters),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam, signal }) =>
      apiRequest<CaseListResponse>(`/runs/${encodeURIComponent(id)}/cases`, {
        params: { ...serverFilters, limit: 250, cursor: pageParam },
        signal,
      }),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    // See useRun: keeps the grid mounted while switching runs. Consumers must
    // not fetchNextPage off placeholder pages (stale cursor for the new run) —
    // RunDetail gates hasNextPage on !isPlaceholderData.
    placeholderData: keepPreviousData,
  });
}

/**
 * The run's provider × prompt × test matrix (`GET /runs/{id}/matrix`). The
 * columns are the complete, first-seen `(provider, prompt)` set, so this is the
 * authoritative source for which provider/prompt filter chips + grid columns a
 * run should show (a run is "matrix-shaped" when it has >1 distinct provider or
 * prompt). Small and stable, so a single default page is fetched; Task 12's
 * matrix view reuses this hook (and `qk.matrix`).
 */
export function useMatrix(id: string, opts: { enabled?: boolean } = {}) {
  const enabled = (opts.enabled ?? true) && !!id;
  return useQuery({
    queryKey: qk.matrix(id),
    queryFn: ({ signal }) =>
      apiRequest<MatrixResponse>(`/runs/${encodeURIComponent(id)}/matrix`, { signal }),
    enabled,
    staleTime: 60_000,
    // See useRun: keeps the provider/prompt chips stable while switching runs.
    placeholderData: keepPreviousData,
  });
}

/**
 * Every test row of a run's matrix, draining the endpoint's row pagination
 * (`next_cursor`) via an infinite query. Distinct from {@link useMatrix} (a
 * single default page, used only to derive the axes/chips) by its `qk.matrixAll`
 * key: Task 12's matrix view needs the complete grid, so it fetches the large
 * `limit` and follows the cursor to the end. Columns are identical across pages
 * (the endpoint only paginates rows), so callers read them off the first page.
 */
export function useMatrixAll(id: string, opts: { enabled?: boolean } = {}) {
  const enabled = (opts.enabled ?? true) && !!id;
  return useInfiniteQuery({
    queryKey: qk.matrixAll(id),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam, signal }) =>
      apiRequest<MatrixResponse>(`/runs/${encodeURIComponent(id)}/matrix`, {
        // Server clamps to 1..=500; ask for the max so most runs are one page.
        params: { limit: 500, cursor: pageParam },
        signal,
      }),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    enabled,
    staleTime: 60_000,
  });
}

export function useCaseDetail(
  id: string,
  caseKey: string | undefined,
  opts: { enabled?: boolean } = {},
) {
  // The query key is unchanged (id + caseKey); `enabled` only gates when the
  // fetch fires — the drawer's baseline diff uses it to defer fetching the
  // baseline case until its section is expanded.
  const enabled = (opts.enabled ?? true) && !!caseKey;
  return useQuery({
    queryKey: qk.caseDetail(id, caseKey ?? ""),
    queryFn: ({ signal }) =>
      apiRequest<CaseResult>(
        `/runs/${encodeURIComponent(id)}/cases/${encodeURIComponent(caseKey!)}`,
        { signal },
      ),
    enabled,
  });
}

/**
 * Grouped full-text hits for `q` (`GET /search`). Disabled for blank queries;
 * previous hits stay visible while a keystroke's refetch is in flight so the
 * search dropdown/page never flashes empty mid-typing.
 */
export function useSearch(
  q: string,
  opts: { enabled?: boolean; limit?: number } = {},
) {
  const query = q.trim();
  const limit = opts.limit ?? 20;
  const enabled = (opts.enabled ?? true) && query.length > 0;
  return useQuery({
    queryKey: qk.search(query, limit),
    queryFn: ({ signal }) =>
      apiRequest<SearchResponse>("/search", { params: { q: query, limit }, signal }),
    enabled,
    staleTime: 30_000,
    placeholderData: keepPreviousData,
  });
}

/** Server default/clamp for the case-history window (see
 *  `HISTORY_DEFAULT_LIMIT`/`HISTORY_MAX_LIMIT` in the routes crate). */
const CASE_HISTORY_DEFAULT_LIMIT = 20;

/**
 * A single case's evolution across the recent runs of its suite (`GET
 * /projects/{project}/suites/{suite}/cases/{case_key}/history`). `points` come
 * back newest-first; the timeline section reverses them for oldest→newest
 * display. `enabled` gates the fetch so the drawer's collapsed History section
 * only asks for the window once it is expanded.
 */
export function useCaseHistory(
  project: string,
  suite: string,
  caseKey: string | undefined,
  opts: { enabled?: boolean; limit?: number } = {},
) {
  const limit = opts.limit ?? CASE_HISTORY_DEFAULT_LIMIT;
  const enabled = (opts.enabled ?? true) && !!project && !!suite && !!caseKey;
  return useQuery({
    queryKey: qk.caseHistory(project, suite, caseKey ?? "", limit),
    queryFn: ({ signal }) =>
      apiRequest<CaseHistoryResponse>(
        `/projects/${encodeURIComponent(project)}/suites/${encodeURIComponent(
          suite,
        )}/cases/${encodeURIComponent(caseKey!)}/history`,
        { params: { limit }, signal },
      ),
    enabled,
    staleTime: 60_000,
  });
}

export function useCompare(id: string, other?: string) {
  const suffix = other ? `/${encodeURIComponent(other)}` : "";
  return useQuery({
    queryKey: qk.compare(id, other),
    queryFn: ({ signal }) =>
      apiRequest<CompareResponse>(
        `/runs/${encodeURIComponent(id)}/compare${suffix}`,
        { signal },
      ),
  });
}

/**
 * The run's config digest + snapshot (`GET /runs/{id}/config`). Cheap
 * config-drift fetch (extracted from the stored blob, no full re-download).
 * `enabled` gates the fetch so the compare page's ConfigDrift panel only asks
 * for both runs' configs once it is opened (`?config=1`).
 */
export function useRunConfig(id: string, opts: { enabled?: boolean } = {}) {
  const enabled = (opts.enabled ?? true) && !!id;
  return useQuery({
    queryKey: qk.runConfig(id),
    queryFn: ({ signal }) =>
      apiRequest<RunConfigResponse>(`/runs/${encodeURIComponent(id)}/config`, { signal }),
    enabled,
    staleTime: 60_000,
  });
}

export function useProjects() {
  return useQuery({
    queryKey: qk.projects,
    queryFn: ({ signal }) => apiRequest<ProjectsResponse>("/projects", { signal }),
  });
}

export function useSuites(project: string | undefined) {
  return useQuery({
    queryKey: qk.suites(project ?? ""),
    queryFn: ({ signal }) =>
      apiRequest<SuitesResponse>(
        `/projects/${encodeURIComponent(project!)}/suites`,
        { signal },
      ),
    enabled: !!project,
  });
}

export function useSetBaseline(project: string, suite: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (runId: string) =>
      apiRequest(
        `/projects/${encodeURIComponent(project)}/suites/${encodeURIComponent(
          suite,
        )}/baseline`,
        { method: "PUT", body: { run_id: runId } },
      ),
    onSuccess: () => {
      // Fire-and-forget refetches; react-query owns the resulting promises.
      void client.invalidateQueries({ queryKey: qk.suites(project) });
      void client.invalidateQueries({ queryKey: ["compare"] });
    },
  });
}

/**
 * Delete a run (admin scope).
 *
 * The endpoint has existed since the server did; nothing could reach it. It is
 * the manual counterpart to `DOMARINN_RUN_MAX_AGE_DAYS`, which is off by
 * default — so without this, a run pushed by mistake was permanent.
 */
export function useDeleteRun() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (runId: string) =>
      apiRequest(`/runs/${encodeURIComponent(runId)}`, { method: "DELETE" }),
    onSuccess: () => {
      // The run is gone from every list, every suite series, and any compare
      // that referenced it.
      void client.invalidateQueries({ queryKey: ["runs"] });
      void client.invalidateQueries({ queryKey: ["suites"] });
      void client.invalidateQueries({ queryKey: ["projects"] });
      void client.invalidateQueries({ queryKey: ["compare"] });
    },
  });
}

export function useCacheStats() {
  return useQuery({
    queryKey: qk.cacheStats,
    queryFn: ({ signal }) => apiRequest<CacheStatsResponse>("/cache/stats", { signal }),
  });
}

export function usePruneCache() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: () => apiRequest<PruneResponse>("/cache/prune", { method: "POST" }),
    // The whole ["cache"] subtree, not just the stats. A prune deletes entries,
    // so leaving the entries list cached would keep rendering rows the server
    // has just evicted.
    onSuccess: () => client.invalidateQueries({ queryKey: ["cache"] }),
  });
}

/**
 * The entries list. Keyed on the MAPPED filters so cache identity equals
 * request identity — `?sort` absent and `?sort=-created` are the same request
 * and must not be two entries.
 */
export function useCacheEntries(filters: CacheFilters) {
  const request = cacheRequestFilters(filters);
  return useInfiniteQuery({
    queryKey: qk.cacheEntries(request),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam, signal }) =>
      apiRequest<CacheEntryListResponse>("/cache/entries", {
        params: { ...request, limit: 100, cursor: pageParam },
        signal,
      }),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    // Dim the current rows while a filter change is in flight rather than
    // unmounting the grid. Consumers MUST gate `fetchNextPage` on
    // `!isPlaceholderData`, or the previous filter's cursor appends its next
    // page into the new result set.
    placeholderData: keepPreviousData,
  });
}

/**
 * One entry.
 *
 * `staleTime: Infinity` is honest here where it rarely is: entries are
 * immutable by construction (the store is first-write-wins), so a fetched
 * entry can never go stale — only be deleted.
 */
export function useCacheEntry(
  key: string | undefined,
  opts: { enabled?: boolean; raw?: boolean } = {},
) {
  const raw = opts.raw ?? false;
  return useQuery({
    queryKey: qk.cacheEntry(key ?? "", raw),
    queryFn: ({ signal }) =>
      apiRequest<CacheEntryDetail>(`/cache/entries/${encodeURIComponent(key!)}`, {
        params: { raw: raw ? "true" : undefined },
        signal,
      }),
    enabled: (opts.enabled ?? true) && !!key,
    staleTime: Infinity,
  });
}

export function useCacheFacets(opts: { enabled?: boolean } = {}) {
  return useQuery({
    queryKey: qk.cacheFacets,
    queryFn: ({ signal }) =>
      apiRequest<CacheFacetsResponse>("/cache/facets", { signal }),
    enabled: opts.enabled ?? true,
  });
}

// --- API keys --------------------------------------------------------------

export function useApiKeys(enabled = true) {
  return useQuery({
    queryKey: qk.apiKeys,
    queryFn: ({ signal }) =>
      apiRequest<ApiKeyListResponse>("/apikeys", { signal }).then((r) => r.keys),
    enabled,
  });
}

export function useCreateApiKey() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: { name: string; scope?: AuthScope }) =>
      apiRequest<ApiKeyCreatedResponse>("/apikeys", { method: "POST", body: input }),
    onSuccess: () => client.invalidateQueries({ queryKey: qk.apiKeys }),
  });
}

export function useRevokeApiKey() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (id: string) =>
      apiRequest(`/apikeys/${encodeURIComponent(id)}`, { method: "DELETE" }),
    onSuccess: () => client.invalidateQueries({ queryKey: qk.apiKeys }),
  });
}

// --- Users (admin) ---------------------------------------------------------

export function useUsers(enabled = true) {
  return useQuery({
    queryKey: qk.users,
    queryFn: ({ signal }) =>
      apiRequest<UserListResponse>("/users", { signal }).then((r) => r.users),
    enabled,
  });
}

export function useCreateUser() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: { username: string; password: string; role: Role }) =>
      apiRequest<UserView>("/users", { method: "POST", body: input }),
    onSuccess: () => client.invalidateQueries({ queryKey: qk.users }),
  });
}

export function useUpdateUser() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: {
      id: string;
      patch: { role?: Role; disabled?: boolean; password?: string };
    }) =>
      apiRequest<UserView>(`/users/${encodeURIComponent(input.id)}`, {
        method: "PATCH",
        body: input.patch,
      }),
    onSuccess: () => client.invalidateQueries({ queryKey: qk.users }),
  });
}

export function useDeleteUser() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (id: string) =>
      apiRequest(`/users/${encodeURIComponent(id)}`, { method: "DELETE" }),
    onSuccess: () => client.invalidateQueries({ queryKey: qk.users }),
  });
}
