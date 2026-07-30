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
  // `origin` (ci | local) separates the canonical CI stream from developer
  // iteration; `actor` narrows to one person, matching either who ran a run or
  // who uploaded it. Both are server filters — the runs list is cursor
  // paginated, so filtering client-side would silently only apply to the pages
  // already loaded.
  "origin",
  "actor",
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
  // Also a SERVER filter (migration 10): narrows the grid to one kind of
  // failure, which is how the error breakdown's rows drill in.
  "error_class",
  "case",
  "sort",
  "view",
] as const;
export type CaseFilterKey = (typeof CASE_FILTER_KEYS)[number];
export type CaseFilters = Partial<Record<CaseFilterKey, string>>;

// Cache-browser keys. `entry` (drawer selection) is the ONLY client-only key
// here — note that `sort` is a SERVER key on this page, the opposite of
// CASE_FILTER_KEYS two declarations above. The entries list is cursor
// paginated over a store far larger than one page, so sorting client-side
// would order the loaded page and quietly claim to have ordered the cache.
// Anything that strips `sort` before the request (as `useRunCases` correctly
// does for its own grid) breaks this page in a way that still looks like it
// works.
export const CACHE_FILTER_KEYS = [
  "tier",
  "kind",
  "model",
  "q",
  "since",
  "until",
  "sort",
  "entry",
] as const;
export type CacheFilterKey = (typeof CACHE_FILTER_KEYS)[number];
export type CacheFilters = Partial<Record<CacheFilterKey, string>>;

/** Sort values the entries endpoint accepts. Anything else is a server 400. */
const CACHE_SORT_COLUMNS = ["created", "last_access", "size", "cost"] as const;
const DEFAULT_CACHE_SORT = "-created";

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

export function parseCacheFilters(sp: URLSearchParams): CacheFilters {
  return pickParams(sp, CACHE_FILTER_KEYS);
}

/** The request params for `GET /cache/entries`, given the URL's state. */
export interface CacheRequestFilters {
  tier?: string;
  kind?: string;
  model?: string;
  q?: string;
  since?: string;
  until?: string;
  sort?: string;
  order?: string;
}

/**
 * Map the cache browser's URL state to the entries request.
 *
 * Three jobs. It drops `entry`, which selects the drawer and means nothing to
 * the server. It splits the single `?sort=-size` param the grid speaks into the
 * `sort` + `order` pair the endpoint speaks, defaulting to newest-first and
 * falling back to that default for a column the server would reject — a junk
 * value in a shared URL should show the page, not a 400. And it leaves `sort`
 * IN the request, unlike the case grid's client-only sort.
 */
export function cacheRequestFilters(filters: CacheFilters): CacheRequestFilters {
  const { entry: _entry, sort, ...rest } = filters;
  const raw = isBlank(sort) ? DEFAULT_CACHE_SORT : (sort as string);
  const desc = raw.startsWith("-");
  const column = desc ? raw.slice(1) : raw;
  const known = (CACHE_SORT_COLUMNS as readonly string[]).includes(column);
  return {
    ...rest,
    sort: known ? column : "created",
    order: known ? (desc ? "desc" : "asc") : "desc",
  };
}

/**
 * Number of active filters. `tier` and `entry` are excluded: the first is a
 * view selector rather than a narrowing, and the second is the open drawer —
 * counting either would make "Clear 1 filter" appear when nothing is filtered.
 */
export function activeCacheFilterCount(sp: URLSearchParams): number {
  return CACHE_FILTER_KEYS.filter(
    (key) => key !== "tier" && key !== "entry" && key !== "sort",
  ).reduce((n, key) => (isBlank(sp.get(key)) ? n : n + 1), 0);
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
