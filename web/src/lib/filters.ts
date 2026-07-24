// URL-as-state helpers. All pure so they can be unit-tested and reused by both
// the router-connected hooks and the api layer.

export const RUNS_FILTER_KEYS = [
  "project",
  "suite",
  "tag",
  "branch",
  "since",
  "until",
  "status",
  "cached",
] as const;
export type RunsFilterKey = (typeof RUNS_FILTER_KEYS)[number];
export type RunsFilters = Partial<Record<RunsFilterKey, string>>;

// `case` (drawer selection), `sort` (grid ordering), and `view` (list vs matrix
// display toggle) are CLIENT-ONLY keys: they live in the URL for shareability but
// are stripped before any request so they never hit the server or the
// react-query cache key (see `useRunCases`). `provider` and `prompt`, by
// contrast, are SERVER filters (the case-list endpoint accepts them since
// migration 3) — they flow through to the request and participate in the query
// key, exactly like `status`/`tag`/`q`.
export const CASE_FILTER_KEYS = [
  "status",
  "tag",
  "q",
  "provider",
  "prompt",
  "cached",
  "case",
  "sort",
  "view",
] as const;
export type CaseFilterKey = (typeof CASE_FILTER_KEYS)[number];
export type CaseFilters = Partial<Record<CaseFilterKey, string>>;

function isBlank(v: string | undefined | null): boolean {
  return v === undefined || v === null || v.trim() === "";
}

/** Read a known set of keys out of a URLSearchParams into a plain object. */
export function pickParams<K extends string>(
  sp: URLSearchParams,
  keys: readonly K[],
): Partial<Record<K, string>> {
  const out: Partial<Record<K, string>> = {};
  for (const key of keys) {
    const v = sp.get(key);
    if (!isBlank(v)) out[key] = v as string;
  }
  return out;
}

/**
 * Apply a patch to an existing URLSearchParams, returning a NEW instance.
 * Blank/undefined/null values delete the key; everything else is set. Keys not
 * mentioned in the patch are preserved.
 */
export function mergeParams(
  sp: URLSearchParams,
  patch: Record<string, string | undefined | null>,
): URLSearchParams {
  const next = new URLSearchParams(sp);
  for (const [key, value] of Object.entries(patch)) {
    if (isBlank(value)) next.delete(key);
    else next.set(key, value as string);
  }
  return next;
}

/**
 * Like mergeParams but also clears a list of keys (e.g. reset pagination
 * cursor when a filter changes) and drops the given keys if they end up blank.
 */
export function mergeParamsResetting(
  sp: URLSearchParams,
  patch: Record<string, string | undefined | null>,
  resetKeys: readonly string[] = [],
): URLSearchParams {
  const cleared: Record<string, undefined> = {};
  for (const key of resetKeys) cleared[key] = undefined;
  return mergeParams(sp, { ...cleared, ...patch });
}

export function parseRunsFilters(sp: URLSearchParams): RunsFilters {
  return pickParams(sp, RUNS_FILTER_KEYS);
}

/**
 * Map parsed runs-list URL state to the request's filter params. The URL's
 * `cached` key means "what the user asked to see"; the request's means "what
 * the server should return" — and the default (no `cached` in the URL) is to
 * HIDE fully-cached passing runs, so absence maps to `cached=exclude`. An
 * explicit `cached=all` reveal sends no param (the server's no-op default),
 * `only` passes through, and junk values fall back to the hidden default
 * instead of a server-side 400.
 */
export function runsRequestFilters(filters: RunsFilters): RunsFilters {
  const { cached, ...rest } = filters;
  if (cached === "all") return rest;
  if (cached === "only") return { ...rest, cached };
  return { ...rest, cached: "exclude" };
}

export function parseCaseFilters(sp: URLSearchParams): CaseFilters {
  return pickParams(sp, CASE_FILTER_KEYS);
}

/** Number of active filters (ignores non-filter keys like pagination/case). */
export function activeRunsFilterCount(sp: URLSearchParams): number {
  return RUNS_FILTER_KEYS.reduce(
    (n, key) => (isBlank(sp.get(key)) ? n : n + 1),
    0,
  );
}

/** Toggle a value: if the key already equals value, clear it; else set it. */
export function toggleValue(
  sp: URLSearchParams,
  key: string,
  value: string,
): URLSearchParams {
  return sp.get(key) === value
    ? mergeParams(sp, { [key]: undefined })
    : mergeParams(sp, { [key]: value });
}
