//! Crossing eras: what a cache entry records, and how a 0.4 store keeps serving.
//!
//! 0.5.0 moved every provider cache key onto a hash of the canonical outgoing
//! request. That is a flag day for every warm store in existence, and the two
//! halves of surviving it are tested here: an entry now carries the request its
//! key was derived from, and an entry written the ≤0.4.x way is found by probing,
//! served, and re-filed under the new key.
//!
//! Separate from `cache_integration.rs` — which asks what shares and what busts
//! *within* one era — because these tests need a backend that reports which keys
//! were asked for, and because the whole file is deletable on the same schedule
//! as `cache_migrate.rs` (see its "Deleting this" section).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use domarinn_core::cache::{
    CacheBackend, CacheEntry, CacheError, CacheKey, CacheStats, PurgeFilter,
};
use domarinn_core::runner::{run, RunOptions};
use domarinn_core::RunResult;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// In-memory, first-write-wins cache mirroring the real backends' semantics.
#[derive(Default)]
struct MemCache {
    map: Mutex<HashMap<String, CacheEntry>>,
}

impl MemCache {
    fn entries(&self) -> usize {
        self.map.lock().unwrap().len()
    }
}

#[async_trait]
impl CacheBackend for MemCache {
    async fn get(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        Ok(self.map.lock().unwrap().get(&key.0).cloned())
    }
    async fn put(&self, key: &CacheKey, entry: &CacheEntry) -> Result<(), CacheError> {
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

/// A [`MemCache`] that also records every key it was asked for, in order.
///
/// The probe list is otherwise unobservable: a backend never sees the request,
/// only the hash. Recording the runner's own lookups is how a test can talk
/// about "the key an older domarinn would have written under" without asserting
/// its own reconstruction of the request against itself.
#[derive(Default)]
struct SpyCache {
    inner: MemCache,
    asked: Mutex<Vec<CacheKey>>,
}

impl SpyCache {
    fn asked(&self) -> Vec<CacheKey> {
        self.asked.lock().unwrap().clone()
    }
    fn forget_lookups(&self) {
        self.asked.lock().unwrap().clear();
    }
    fn entry_at(&self, key: &CacheKey) -> Option<CacheEntry> {
        self.inner.map.lock().unwrap().get(&key.0).cloned()
    }
}

#[async_trait]
impl CacheBackend for SpyCache {
    async fn get(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        self.asked.lock().unwrap().push(key.clone());
        self.inner.get(key).await
    }
    async fn put(&self, key: &CacheKey, entry: &CacheEntry) -> Result<(), CacheError> {
        self.inner.put(key, entry).await
    }
    async fn stats(&self) -> Result<CacheStats, CacheError> {
        self.inner.stats().await
    }
    async fn purge(&self, filter: &PurgeFilter) -> Result<u64, CacheError> {
        self.inner.purge(filter).await
    }
}

async fn run_suite(yaml: &str, opts: RunOptions, cache: &dyn CacheBackend) -> RunResult {
    let suite = domarinn_core::load_str(yaml).unwrap();
    run(&suite, Path::new("."), cache, None, &opts)
        .await
        .unwrap()
}

async fn run_default(yaml: &str, cache: &dyn CacheBackend) -> RunResult {
    run_suite(yaml, RunOptions::default(), cache).await
}

/// The entry stores the request its key was derived from, resolved.
///
/// This is what makes a store debuggable — and re-keyable — offline: an entry is
/// a hash plus an opaque answer unless it also carries the question. The URL is
/// asserted against the stub's own address, so a provider that stopped resolving
/// `base_url` into the request would fail here rather than store a template.
#[tokio::test]
async fn a_warm_entry_records_the_resolved_request_it_answers() {
    const KEY: &str = "DOMARINN_CACHE_E2E_REQUEST_KEY";
    std::env::set_var(KEY, "sk-test");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{"type": "text", "text": "hello"}],
            "usage": {"input_tokens": 10, "output_tokens": 5},
        })))
        .mount(&server)
        .await;

    let yaml = format!(
        r#"
version: 1
project: test
suite: cache
providers:
  - id: p
    type: anthropic
    model: "model-a"
    base_url: "{}"
    api_key_env: {KEY}
prompts:
  - id: only
    template: "hello {{{{ x }}}}"
tests:
  - id: case-a
    vars: {{x: "a"}}
"#,
        server.uri()
    );
    let cache = MemCache::default();
    run_default(&yaml, &cache).await;

    let entry = cache.map.lock().unwrap().values().next().unwrap().clone();
    let request = entry.request.expect("a warm entry records its request");
    // The keyed request carries the path alone — `base_url` is not part of what
    // makes two calls interchangeable, so a gateway and a direct connection
    // share entries.
    assert_eq!(
        request["path"],
        json!("/v1/messages"),
        "the keyed request is unaddressed: {request}"
    );
    assert!(request.get("url").is_none(), "…deliberately: {request}");
    // The address is still recorded, just as evidence rather than identity, so
    // a hit from a different endpoint can be reported.
    assert_eq!(
        entry.address.as_deref(),
        Some(format!("{}/v1/messages", server.uri()).as_str()),
        "a warm entry records where the answer came from"
    );
    assert_eq!(request["method"], json!("POST"));
    assert_eq!(request["body"]["model"], json!("model-a"));
    assert!(
        entry.provider_fingerprint.is_none(),
        "the request is what identifies the entry now, not the fingerprint"
    );
}

/// A warm 0.4 store keeps serving, and settles onto the new key after one run.
///
/// The flag-day test. 0.5.0 moved every provider key, including for the three
/// network providers whose fingerprints had never changed and which therefore
/// had no migration path at all before now — an anthropic suite's entire cache
/// would have been stranded silently, and the only symptom is a bill.
///
/// The store is seeded exactly as 0.4 left it: keyed the seven-part way, with a
/// `provider_fingerprint` and no `request`. Nothing about the assertion trusts
/// the new code to describe the old — the seeded key comes from watching what
/// the runner probes for.
#[tokio::test]
async fn a_zero_four_entry_is_adopted_re_filed_and_then_found_directly() {
    const KEY: &str = "DOMARINN_CACHE_E2E_ADOPT_KEY";
    std::env::set_var(KEY, "sk-test");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{"type": "text", "text": "a fresh answer"}],
            "usage": {"input_tokens": 10, "output_tokens": 5},
        })))
        .mount(&server)
        .await;

    let yaml = format!(
        r#"
version: 1
project: test
suite: cache
providers:
  - id: p
    type: anthropic
    model: "model-a"
    base_url: "{}"
    api_key_env: {KEY}
prompts:
  - id: only
    template: "hello {{{{ x }}}}"
tests:
  - id: case-a
    vars: {{x: "a"}}
"#,
        server.uri()
    );

    // What a cold case looks for, and what it writes. A vendor provider has two
    // historical shapes, probed newest-first: [live, pre-0.8 canonical, ≤0.4.x
    // fingerprint].
    let discovery = SpyCache::default();
    run_default(&yaml, &discovery).await;
    let probed = discovery.asked();
    assert_eq!(
        probed.len(),
        3,
        "a vendor provider probes its own key and two historical shapes"
    );
    let (live_key, legacy_key) = (probed[0].clone(), probed[2].clone());

    // Roll that entry back to the shape 0.4 wrote: keyed the old way, carrying a
    // fingerprint instead of a request. The output is distinct so a hit can be
    // told apart from a fresh call.
    let suite = domarinn_core::load_str(&yaml).unwrap();
    let provider =
        domarinn_core::provider_factory::build_provider(&suite.providers[0], None).unwrap();
    let mut as_zero_four = discovery.entry_at(&live_key).expect("the cold run wrote");
    as_zero_four.request = None;
    as_zero_four.provider_fingerprint = Some(provider.fingerprint());
    as_zero_four.output = domarinn_core::types::Output::Text("from the 0.4 store".into());

    let seeded = || async {
        let store = SpyCache::default();
        store.put(&legacy_key, &as_zero_four).await.unwrap();
        store
    };

    // The control: without migration that entry is simply unreachable. Without
    // this the rest of the test would pass just as well against a store the
    // runner found by some other route.
    let before = server.received_requests().await.unwrap().len();
    let unmigrated = seeded().await;
    run_suite(
        &yaml,
        RunOptions {
            cache_migration: false,
            ..Default::default()
        },
        &unmigrated,
    )
    .await;
    assert_eq!(
        server.received_requests().await.unwrap().len() - before,
        1,
        "--no-cache-migration must re-pay: the seeded key is only reachable by probing"
    );

    let store = seeded().await;
    let before = server.received_requests().await.unwrap().len();

    let adopted = run_default(&yaml, &store).await;
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        before,
        "an adoptable entry must not be re-paid for"
    );
    assert_eq!(adopted.summary.cache_hits, 1);
    assert_eq!(
        adopted.cases[0].output.as_ref().unwrap().as_text(),
        "from the 0.4 store",
        "the answer served must be the stored one, not a fresh call"
    );

    let re_filed = store
        .entry_at(&live_key)
        .expect("an adopted entry is re-filed under the live key");
    assert_eq!(
        re_filed.request.expect("re-filed with its request")["path"],
        json!("/v1/messages"),
        "an entry that crosses into the new era must carry what its key is derived from"
    );

    // Settled: the next run finds it on the first lookup and probes nothing.
    store.forget_lookups();
    let settled = run_default(&yaml, &store).await;
    assert_eq!(settled.summary.cache_hits, 1);
    assert_eq!(
        store.asked(),
        vec![live_key],
        "a migrated store must stop paying for probes"
    );
}
