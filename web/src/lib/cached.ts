/**
 * The client-side mirror of the server's cached-run rules.
 *
 * These predicates existed in four places — the runs list, the suite page, and
 * twice inside the mock handlers — which is three chances for the UI to
 * disagree with itself about which runs are noise. They live here so the
 * dimming, the chips, the hidden counts and the mock server all answer the
 * question the same way. Parity with the *server* is guarded separately, by
 * the Rust tests over `FULLY_CACHED` in `storage/runs.rs`.
 *
 * Both predicates take structural shapes rather than `RunListItem`, so fixtures
 * and search hits can use them without first being widened into a full run.
 *
 * Everything here is pure. The stored preference that `resolveCached` reads
 * against lives in `cachedPref.ts`, keeping storage effects out of the rules —
 * the same split `drawerWidth.ts` uses.
 */

import type { CachedFilter } from "@/api";

/** The two migration-6 counters, as they arrive on the wire. */
export interface CacheCounters {
  cache_hits: number | null;
  cache_misses: number | null;
}

/** Counters plus the verdict tallies the exclude rule also reads. */
export interface CachedRunVerdict extends CacheCounters {
  fail_count: number;
  error_count: number;
}

/**
 * Every provider call in the run was served from cache.
 *
 * Mirrors `FULLY_CACHED` in `storage/runs.rs`: `cache_misses = 0 AND
 * cache_hits > 0`. The `> 0` is what excludes a run that made no provider
 * calls at all, which has zero misses without being cached in any useful
 * sense.
 *
 * A row we cannot classify is never "fully cached". Legacy pre-backfill rows
 * arrive as `null`, and so does the `-1` undecodable-blob sentinel, which the
 * server maps to `null` in `clean_cache_count` before it reaches us. Both fall
 * out of the comparisons below as `false`, which is the safe direction: an
 * unclassifiable run stays visible.
 */
export function isFullyCached(r: CacheCounters): boolean {
  return r.cache_misses === 0 && (r.cache_hits ?? 0) > 0;
}

/**
 * What `cached=exclude` suppresses: every fully cached run, whatever its
 * verdict.
 *
 * This used to also require the run to have passed, reasoning that grader
 * verdicts are not cached — only provider responses are — so a replay could
 * still surface a new failure. That reasoning was already false: since 0.5.0
 * graders, embeddings and `exec` assertions share one cache and one key space
 * with provider calls, so a fully cached run replayed its grading too and its
 * verdict carried nothing new. Meanwhile any suite whose failures are not rare
 * tripped the guard on every run, and the filter suppressed nothing at all.
 *
 * The server's `cached_hidden_sql` carries the full account, including the two
 * narrow paths that really can move a verdict inside a fully cached run —
 * neither of which is "the run failed".
 *
 * Mirrors `cached_hidden_sql` in the server's `storage/mod.rs`; the two must
 * agree, or the client dims a different set of rows than the server withholds.
 */
export function hiddenByCachedExclude(r: CachedRunVerdict): boolean {
  return isFullyCached(r);
}

/** The three tokens `GET /runs?cached=` accepts. */
export const CACHED_FILTERS: readonly CachedFilter[] = ["exclude", "only", "all"];

/** What an untouched install does: keep the CI re-run noise out of the list. */
export const DEFAULT_CACHED: CachedFilter = "exclude";

export function isCachedFilter(v: unknown): v is CachedFilter {
  return (
    typeof v === "string" && (CACHED_FILTERS as readonly string[]).includes(v)
  );
}

/**
 * Which cached-run view a surface should render, given its URL and the user's
 * stored preference.
 *
 * The URL wins whenever it names a real filter. That is what makes a shared
 * link mean the same thing to whoever opens it — if the preference could
 * override it, two people reading the same URL would see different runs, and
 * the difference would be invisible to both.
 *
 * Absence falls through to the preference rather than to a hard-coded default,
 * which is what lets one setting reach every surface. It also keeps every link
 * written before this existed working: the shipped default preference is
 * `exclude`, so a bare `/runs` still means "hidden" for anyone who has not
 * deliberately chosen otherwise.
 *
 * Junk resolves to the preference rather than being passed through, so a typo
 * in a pasted URL shows the page instead of a 400 from the server.
 */
export function resolveCached(
  urlValue: string | null | undefined,
  pref: CachedFilter,
): CachedFilter {
  return isCachedFilter(urlValue) ? urlValue : pref;
}
