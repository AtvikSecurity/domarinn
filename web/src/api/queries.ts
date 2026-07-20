import {
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { apiRequest } from "./client";
import type {
  AdminUser,
  ApiKey,
  ApiKeyCreated,
  AuthScope,
  CacheStats,
  CaseDetail,
  CasesResponse,
  CompareResult,
  Meta,
  MeResponse,
  ProjectsResponse,
  Role,
  RunDetailResult,
  RunsResponse,
  SuitesResponse,
} from "./types";
import type { CaseFilters, RunsFilters } from "@/lib/filters";

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
  compare: (id: string, other?: string) => ["compare", id, other ?? null] as const,
  projects: ["projects"] as const,
  suites: (project: string) => ["suites", project] as const,
  cacheStats: ["cache", "stats"] as const,
};

export function useMeta() {
  return useQuery({
    queryKey: qk.meta,
    queryFn: () => apiRequest<Meta>("/meta"),
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
  return useInfiniteQuery({
    queryKey: qk.runs(filters),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam, signal }) =>
      apiRequest<RunsResponse>("/runs", {
        params: { ...filters, limit: 50, cursor: pageParam },
        signal,
      }),
    getNextPageParam: (last) => last.next_cursor,
  });
}

export function useRun(id: string) {
  return useQuery({
    queryKey: qk.run(id),
    queryFn: ({ signal }) =>
      apiRequest<RunDetailResult>(`/runs/${encodeURIComponent(id)}`, { signal }),
  });
}

export function useRunCases(id: string, filters: CaseFilters) {
  // Only the server-side filters go to the API; `case` (the drawer selection)
  // is client-only.
  const { case: _case, ...serverFilters } = filters;
  return useInfiniteQuery({
    queryKey: qk.cases(id, serverFilters),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam, signal }) =>
      apiRequest<CasesResponse>(`/runs/${encodeURIComponent(id)}/cases`, {
        params: { ...serverFilters, limit: 250, cursor: pageParam },
        signal,
      }),
    getNextPageParam: (last) => last.next_cursor,
  });
}

export function useCaseDetail(id: string, caseKey: string | undefined) {
  return useQuery({
    queryKey: qk.caseDetail(id, caseKey ?? ""),
    queryFn: ({ signal }) =>
      apiRequest<CaseDetail>(
        `/runs/${encodeURIComponent(id)}/cases/${encodeURIComponent(caseKey!)}`,
        { signal },
      ),
    enabled: !!caseKey,
  });
}

export function useCompare(id: string, other?: string) {
  const suffix = other ? `/${encodeURIComponent(other)}` : "";
  return useQuery({
    queryKey: qk.compare(id, other),
    queryFn: ({ signal }) =>
      apiRequest<CompareResult>(
        `/runs/${encodeURIComponent(id)}/compare${suffix}`,
        { signal },
      ),
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
      client.invalidateQueries({ queryKey: qk.suites(project) });
      client.invalidateQueries({ queryKey: ["compare"] });
    },
  });
}

export function useCacheStats() {
  return useQuery({
    queryKey: qk.cacheStats,
    queryFn: ({ signal }) => apiRequest<CacheStats>("/cache/stats", { signal }),
  });
}

export function usePruneCache() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: () => apiRequest("/cache/prune", { method: "POST" }),
    onSuccess: () => client.invalidateQueries({ queryKey: qk.cacheStats }),
  });
}

// --- API keys --------------------------------------------------------------

export function useApiKeys(enabled = true) {
  return useQuery({
    queryKey: qk.apiKeys,
    queryFn: ({ signal }) => apiRequest<ApiKey[]>("/apikeys", { signal }),
    enabled,
  });
}

export function useCreateApiKey() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: { name: string; scope?: AuthScope }) =>
      apiRequest<ApiKeyCreated>("/apikeys", { method: "POST", body: input }),
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
    queryFn: ({ signal }) => apiRequest<AdminUser[]>("/users", { signal }),
    enabled,
  });
}

export function useCreateUser() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: { username: string; password: string; role: Role }) =>
      apiRequest<AdminUser>("/users", { method: "POST", body: input }),
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
      apiRequest<AdminUser>(`/users/${encodeURIComponent(input.id)}`, {
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
