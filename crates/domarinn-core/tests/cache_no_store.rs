//! `cache.store_empty_outputs`: which empty provider outputs reach the store.
//!
//! The bug this pins (issue #79) is subtler than "refusals are cached". An
//! empty output is a *successful* call, so it takes the `Ok` arm of the write
//! path — which is why the structural "errors are never cached" guard never
//! covered it. Against an immutable, first-write-wins store one transient empty
//! reply is then replayed on every later run, for everyone sharing the cache.
//!
//! The fixtures matter as much as the assertions. `empty_reason` is only
//! computed when the output text is *blank* (`anthropic.rs`, `openai.rs`, both
//! gate on `if text.trim().is_empty()`), and `refusal` only appears there when
//! the vendor said so. A stub that returns prose, or one that returns an empty
//! body with an ordinary finish reason, classifies as something else entirely —
//! which is exactly why a denylist naming only `refusal` would not have fixed
//! the reported symptom. Every fixture here is written to that grain.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use domarinn_core::cache::{
    CacheBackend, CacheEntry, CacheError, CacheKey, CacheStats, PurgeFilter,
};
use domarinn_core::config::StoreEmptyOutputs;
use domarinn_core::runner::{run, RunOptions};
use domarinn_core::RunResult;

/// First-write-wins in-memory store that counts what it was asked to do.
///
/// The `puts` counter is the whole point: a plain map shows the end state, but
/// "was this ever offered to the store" is the property under test.
#[derive(Default)]
struct CountingCache {
    map: Mutex<HashMap<String, CacheEntry>>,
    puts: AtomicUsize,
    gets: AtomicUsize,
}

impl CountingCache {
    fn puts(&self) -> usize {
        self.puts.load(Ordering::SeqCst)
    }
    fn gets(&self) -> usize {
        self.gets.load(Ordering::SeqCst)
    }
    fn entries(&self) -> usize {
        self.map.lock().unwrap().len()
    }
}

#[async_trait]
impl CacheBackend for CountingCache {
    async fn get(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        Ok(self.map.lock().unwrap().get(&key.0).cloned())
    }
    async fn put(&self, key: &CacheKey, entry: &CacheEntry) -> Result<(), CacheError> {
        self.puts.fetch_add(1, Ordering::SeqCst);
        self.map
            .lock()
            .unwrap()
            .entry(key.0.clone())
            .or_insert_with(|| entry.clone());
        Ok(())
    }
    async fn stats(&self) -> Result<CacheStats, CacheError> {
        Ok(CacheStats {
            entries: self.entries() as u64,
            ..Default::default()
        })
    }
    async fn purge(&self, _filter: &PurgeFilter) -> Result<u64, CacheError> {
        Ok(0)
    }
}

/// A suite whose `exec` provider emits `body` verbatim as its protocol response.
///
/// `exec` rather than a wiremock vendor because the exec protocol lets a test
/// state an `empty_reason` directly, which is the cleanest way to cover a reason
/// this build has never heard of — and because it needs no network at all.
fn suite_emitting(body: &str, cache_cfg: &str) -> String {
    format!(
        r#"
version: 1
project: test
suite: no-store
{cache_cfg}providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '%s' '{body}'"]
    cache_salt: "v1"
tests:
  - id: t
    assert:
      - type: contains
        value: anything
"#
    )
}

async fn run_suite(yaml: &str, cache: &dyn CacheBackend, opts: RunOptions) -> RunResult {
    let suite = domarinn_core::load_str(yaml).unwrap();
    run(&suite, Path::new("."), cache, None, &opts)
        .await
        .unwrap()
}

/// The reported symptom: a gateway returns an empty body with an ordinary
/// finish reason. It classifies as `blank`, never as `refusal` — so a fix
/// written as a denylist of `[refusal, content_filter]` would have left this
/// cached and shipped a fix for #79 that did not fix #79.
#[tokio::test]
async fn an_empty_output_with_no_vendor_reason_is_not_stored_by_default() {
    let cache = CountingCache::default();
    let result = run_suite(
        &suite_emitting(r#"{\"output\":\"\"}"#, ""),
        &cache,
        RunOptions::default(),
    )
    .await;

    assert_eq!(
        cache.puts(),
        0,
        "an empty output must not reach the store under the default policy"
    );
    assert_eq!(cache.entries(), 0);
    // Still graded, and still reported: not storing is not the same as hiding.
    assert_eq!(result.summary.total, 1);
    assert!(result.cases[0].empty_reason.is_some());
}

#[tokio::test]
async fn a_declared_refusal_is_not_stored_by_default() {
    let cache = CountingCache::default();
    run_suite(
        &suite_emitting(r#"{\"output\":\"\",\"empty_reason\":\"refusal\"}"#, ""),
        &cache,
        RunOptions::default(),
    )
    .await;
    assert_eq!(cache.puts(), 0);
}

/// The guard against the gate over-matching. This is the case that must keep
/// working exactly as before, and it is the overwhelming majority of calls.
#[tokio::test]
async fn a_real_answer_is_still_stored() {
    let cache = CountingCache::default();
    run_suite(
        &suite_emitting(r#"{\"output\":\"anything at all\"}"#, ""),
        &cache,
        RunOptions::default(),
    )
    .await;
    assert_eq!(cache.puts(), 1);
    assert_eq!(cache.entries(), 1);
}

/// `truncated` is a property of the request — raise `max_tokens` and it changes
/// — so it recurs, and caching it is right. This is the line `reproducible`
/// draws, and drawing it in the wrong place is how the policy becomes either
/// useless or a permanent cache miss.
#[tokio::test]
async fn a_reproducible_empty_reason_is_still_stored() {
    let cache = CountingCache::default();
    run_suite(
        &suite_emitting(r#"{\"output\":\"\",\"empty_reason\":\"truncated\"}"#, ""),
        &cache,
        RunOptions::default(),
    )
    .await;
    assert_eq!(
        cache.puts(),
        1,
        "`truncated` recurs for the same request, so it is worth keeping"
    );
}

/// `EmptyReason` is open, so the policy has to answer for reasons that did not
/// exist when it was written. A named policy answers "not stored"; a denylist
/// would have answered "stored", silently, until someone noticed.
#[tokio::test]
async fn a_reason_this_build_has_never_heard_of_is_not_stored() {
    let cache = CountingCache::default();
    run_suite(
        &suite_emitting(
            r#"{\"output\":\"\",\"empty_reason\":\"some_future_vendor_reason\"}"#,
            "",
        ),
        &cache,
        RunOptions::default(),
    )
    .await;
    assert_eq!(cache.puts(), 0);
}

#[tokio::test]
async fn always_restores_the_previous_behaviour() {
    let cache = CountingCache::default();
    run_suite(
        &suite_emitting(
            r#"{\"output\":\"\",\"empty_reason\":\"refusal\"}"#,
            "cache:\n  store_empty_outputs: always\n",
        ),
        &cache,
        RunOptions::default(),
    )
    .await;
    assert_eq!(cache.puts(), 1);
}

#[tokio::test]
async fn never_drops_even_a_reproducible_reason() {
    let cache = CountingCache::default();
    run_suite(
        &suite_emitting(
            r#"{\"output\":\"\",\"empty_reason\":\"truncated\"}"#,
            "cache:\n  store_empty_outputs: never\n",
        ),
        &cache,
        RunOptions::default(),
    )
    .await;
    assert_eq!(cache.puts(), 0);
}

/// The run option behind `--store-empty-outputs` / the env var, which must beat
/// the suite rather than merge with it.
#[tokio::test]
async fn the_run_option_overrides_the_suite() {
    let cache = CountingCache::default();
    run_suite(
        &suite_emitting(
            r#"{\"output\":\"\",\"empty_reason\":\"refusal\"}"#,
            "cache:\n  store_empty_outputs: never\n",
        ),
        &cache,
        RunOptions {
            store_empty_outputs: Some(StoreEmptyOutputs::Always),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(cache.puts(), 1);
}

/// The locked read-path decision, stated as a test so it is not "fixed" later.
///
/// The gate is write-side only. An entry already in an immutable store keeps
/// replaying, which is precisely why targeted eviction (issue #80) exists — and
/// why the runner warns once per provider when it happens.
#[tokio::test]
async fn an_already_cached_empty_still_replays() {
    let cache = CountingCache::default();
    let yaml = suite_emitting(
        r#"{\"output\":\"\",\"empty_reason\":\"refusal\"}"#,
        "cache:\n  store_empty_outputs: always\n",
    );
    // Seed it under the policy that stores.
    run_suite(&yaml, &cache, RunOptions::default()).await;
    assert_eq!(cache.entries(), 1, "the seeding run must have stored it");

    // Now run under today's default, which would not have written it.
    let strict = suite_emitting(r#"{\"output\":\"\",\"empty_reason\":\"refusal\"}"#, "");
    let result = run_suite(&strict, &cache, RunOptions::default()).await;
    assert!(
        result.cases[0].cached,
        "the entry is immutable and still on disk, so it is still served"
    );
    assert!(cache.gets() > 0);
}

/// Refusal patterns catch what the classification gate cannot see: a model that
/// declines in prose, where the output is non-empty and `empty_reason` is
/// therefore `None`.
#[tokio::test]
async fn a_prose_refusal_matching_a_pattern_is_not_stored() {
    let cache = CountingCache::default();
    let yaml = format!(
        r#"
version: 1
project: test
suite: no-store
runner:
  refusal_patterns:
    - "(?i)^i cannot help with"
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '%s' '{body}'"]
    cache_salt: "v1"
tests:
  - id: t
    assert:
      - type: contains
        value: anything
"#,
        // No apostrophe: the exec command wraps this in shell single quotes,
        // and one here would close them and leave the provider erroring — which
        // looks exactly like the pattern not matching.
        body = r#"{\"output\":\"I cannot help with that request.\"}"#
    );
    let result = run_suite(&yaml, &cache, RunOptions::default()).await;

    assert_eq!(
        cache.puts(),
        0,
        "a matched prose refusal must not be stored"
    );
    assert_eq!(
        result.cases[0].empty_reason.as_ref().map(|r| r.as_str()),
        Some("refusal"),
        "the case reports the pattern's diagnosis, so the two classifiers agree"
    );
}

/// The same output, with no pattern configured, is an ordinary answer. Proves
/// the feature is genuinely opt-in — a false positive here would silently stop
/// caching real answers.
#[tokio::test]
async fn the_same_prose_is_an_ordinary_answer_without_a_pattern() {
    let cache = CountingCache::default();
    let result = run_suite(
        &suite_emitting(r#"{\"output\":\"I cannot help with that request.\"}"#, ""),
        &cache,
        RunOptions::default(),
    )
    .await;
    assert_eq!(cache.puts(), 1);
    assert!(result.cases[0].empty_reason.is_none());
}
