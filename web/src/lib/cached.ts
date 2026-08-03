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
 * Both functions take structural shapes rather than `RunListItem`, so fixtures
 * and search hits can use them without first being widened into a full run.
 */

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
 * What `cached=exclude` suppresses: a fully cached run that also passed.
 *
 * The `&& passing` half is load-bearing rather than incidental. Grader
 * verdicts are not cached — only provider responses are — so re-running an
 * unchanged config can still surface a new failure. Hiding a fully-cached
 * *failing* run would hide exactly the regression the re-run was for.
 */
export function hiddenByCachedExclude(r: CachedRunVerdict): boolean {
  return isFullyCached(r) && r.fail_count === 0 && r.error_count === 0;
}
