//! End-to-end behaviour of a provider's `request:` block, through the runner.
//!
//! The unit tests in `request_cfg.rs` cover the resolver. These cover what a
//! *run* does with it: what reaches the wire, what reaches the cache, and what
//! happens to a store written before `base_url` left the key.
//!
//! Separate from `cache_provider_keys.rs`, which asks what shares and what busts
//! within one shape, because everything here needs to inspect the actual bytes a
//! stub received.

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
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[derive(Default)]
struct MemCache {
    map: Mutex<HashMap<String, CacheEntry>>,
}

impl MemCache {
    fn entries(&self) -> Vec<CacheEntry> {
        self.map.lock().unwrap().values().cloned().collect()
    }
    fn seed(&self, key: &CacheKey, entry: CacheEntry) {
        self.map.lock().unwrap().insert(key.0.clone(), entry);
    }
    fn take(&self, key: &CacheKey) -> Option<CacheEntry> {
        self.map.lock().unwrap().remove(&key.0)
    }
    fn only_key(&self) -> CacheKey {
        let map = self.map.lock().unwrap();
        assert_eq!(map.len(), 1, "expected exactly one entry");
        CacheKey(map.keys().next().unwrap().clone())
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
        Ok(CacheStats::default())
    }
    async fn purge(&self, _filter: &PurgeFilter) -> Result<u64, CacheError> {
        Ok(0)
    }
}

async fn answering_stub() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{"type": "text", "text": "an answer"}],
            "usage": {"input_tokens": 3, "output_tokens": 2},
        })))
        .mount(&server)
        .await;
    server
}

async fn served(server: &MockServer) -> Vec<Request> {
    server.received_requests().await.unwrap_or_default()
}

async fn run_suite(yaml: &str, cache: &dyn CacheBackend) -> RunResult {
    let suite = domarinn_core::load_str(yaml).unwrap();
    run(&suite, Path::new("."), cache, None, &RunOptions::default())
        .await
        .unwrap()
}

/// One anthropic provider against `base`, with `request` spliced in verbatim.
fn suite_with(base: &str, key_env: &str, request: &str) -> String {
    format!(
        r#"
version: 1
project: test
suite: request-cfg
providers:
  - id: p
    type: anthropic
    model: "model-a"
    base_url: "{base}"
    api_key_env: {key_env}
{request}
prompts:
  - id: only
    template: "say hello"
tests:
  - id: case-a
"#
    )
}

/// The whole point of the feature, on the wire.
///
/// An Anthropic OAuth token is rejected as `x-api-key` and accepted as a bearer
/// token, so `auth: bearer` has to actually change which header carries it — and
/// the vendor header the provider always sends has to survive alongside.
#[tokio::test]
async fn auth_bearer_moves_the_credential_and_keeps_the_vendor_header() {
    const KEY: &str = "REQCFG_E2E_OAUTH_TOKEN";
    std::env::set_var(KEY, "sk-ant-oat01-NOT-REAL");
    let server = answering_stub().await;

    run_suite(
        &suite_with(
            &server.uri(),
            KEY,
            "    request:\n      auth: bearer\n      headers:\n        anthropic-beta: \"oauth-2025-04-20\"\n",
        ),
        &MemCache::default(),
    )
    .await;

    let requests = served(&server).await;
    assert_eq!(requests.len(), 1);
    let headers = &requests[0].headers;

    assert_eq!(
        headers.get("authorization").map(|v| v.to_str().unwrap()),
        Some("Bearer sk-ant-oat01-NOT-REAL"),
    );
    assert!(
        headers.get("x-api-key").is_none(),
        "`auth: bearer` replaces the default scheme rather than adding to it"
    );
    assert_eq!(
        headers.get("anthropic-beta").map(|v| v.to_str().unwrap()),
        Some("oauth-2025-04-20"),
    );
    assert_eq!(
        headers
            .get("anthropic-version")
            .map(|v| v.to_str().unwrap()),
        Some("2023-06-01"),
        "the vendor header the provider always sends is not lost"
    );
}

/// `auth: none` sends no credential of its own and, crucially, does not demand
/// one: the whole point is that the header carries it instead. A provider that
/// still required `api_key_env` to resolve would fail a correct config.
#[tokio::test]
async fn auth_none_sends_no_credential_and_requires_no_variable() {
    let server = answering_stub().await;

    let outcome = run_suite(
        &suite_with(
            &server.uri(),
            "REQCFG_E2E_DEFINITELY_UNSET",
            "    request:\n      auth: none\n      headers:\n        x-gateway: \"static\"\n",
        ),
        &MemCache::default(),
    )
    .await;

    assert_eq!(outcome.summary.errored, 0, "{:?}", outcome.cases[0].error);
    let requests = served(&server).await;
    let headers = &requests[0].headers;
    assert!(headers.get("x-api-key").is_none());
    assert!(headers.get("authorization").is_none());
    assert_eq!(
        headers.get("x-gateway").map(|v| v.to_str().unwrap()),
        Some("static")
    );
}

/// `path`, `query` and `body` all reach the wire, and the query is ordered by
/// name so two suites writing the same pairs differently still share an entry.
#[tokio::test]
async fn path_query_and_body_overlay_reach_the_wire() {
    const KEY: &str = "REQCFG_E2E_PATH_KEY";
    std::env::set_var(KEY, "sk-test");
    let server = answering_stub().await;

    run_suite(
        &suite_with(
            &server.uri(),
            KEY,
            "    request:\n      path: \"/openai/deployments/d1/messages\"\n      query:\n        zeta: \"1\"\n        api-version: \"2024-10-01\"\n      body:\n        system: \"injected by the gateway\"\n",
        ),
        &MemCache::default(),
    )
    .await;

    let requests = served(&server).await;
    assert_eq!(
        requests[0].url.path(),
        "/openai/deployments/d1/messages",
        "the vendor's own suffix is replaced, not appended to"
    );
    assert_eq!(
        requests[0].url.query(),
        Some("api-version=2024-10-01&zeta=1"),
        "sorted by name"
    );

    let body: serde_json::Value = requests[0].body_json().unwrap();
    assert_eq!(
        body["system"], "injected by the gateway",
        "the overlay merges after the provider built the body, so it reaches \
         `system` — which `params:` cannot"
    );
    assert_eq!(body["model"], "model-a", "untouched fields survive");
}

/// The redaction property, on the two documents that get persisted.
///
/// Both the cache entry and a `--share`d run publish these, so a credential
/// pulled in with `{{ env.X }}` must appear in neither. Mirrors
/// `a_call_time_credential_is_replaced_by_its_placeholder_in_both_documents`
/// in `http_provider.rs`, for the vendor providers.
#[tokio::test]
async fn a_call_time_credential_never_reaches_the_cache_or_the_run_document() {
    const KEY: &str = "REQCFG_E2E_REDACT_KEY";
    const SECRET: &str = "REQCFG_E2E_TENANT_TOKEN";
    std::env::set_var(KEY, "sk-test");
    std::env::set_var(SECRET, "SENTINEL-SECRET");
    let server = answering_stub().await;
    let cache = MemCache::default();

    let outcome = run_suite(
        &suite_with(
            &server.uri(),
            KEY,
            &format!(
                "    request:\n      headers:\n        x-tenant: \"Bearer {{{{ env.{SECRET} }}}}\"\n"
            ),
        ),
        &cache,
    )
    .await;

    // It did reach the wire…
    let requests = served(&server).await;
    assert_eq!(
        requests[0]
            .headers
            .get("x-tenant")
            .map(|v| v.to_str().unwrap()),
        Some("Bearer SENTINEL-SECRET"),
    );

    // …and neither persisted document carries it.
    let entry = cache.entries().pop().expect("one entry was written");
    let stored = serde_json::to_string(&entry).unwrap();
    assert!(!stored.contains("SENTINEL-SECRET"), "{stored}");

    let published = serde_json::to_string(&outcome.cases[0]).unwrap();
    assert!(!published.contains("SENTINEL-SECRET"), "{published}");
}

/// A header is keyed as a digest, so it separates two providers without
/// publishing what it contains.
#[tokio::test]
async fn a_declared_header_is_keyed_as_a_digest_not_verbatim() {
    const KEY: &str = "REQCFG_E2E_DIGEST_KEY";
    std::env::set_var(KEY, "sk-test");
    let server = answering_stub().await;
    let cache = MemCache::default();

    run_suite(
        &suite_with(
            &server.uri(),
            KEY,
            "    request:\n      headers:\n        x-tier: \"fast-and-loud\"\n",
        ),
        &cache,
    )
    .await;

    let entry = cache.entries().pop().expect("one entry");
    let request = entry.request.expect("a warm entry records its request");
    assert!(
        request["headers_digest"]
            .as_str()
            .is_some_and(|d| d.starts_with("blake3:")),
        "{request}"
    );
    assert!(
        !serde_json::to_string(&request)
            .unwrap()
            .contains("fast-and-loud"),
        "the value is digested, never published: {request}"
    );
}

/// The upgrade rehearsal, deterministically.
///
/// A store written before 0.8.0 keyed the full `base_url` into a `url` member.
/// Rather than building the old binary, the entry is seeded under exactly the
/// key the provider itself reports having published — the same manoeuvre
/// `cache_era.rs` uses for the ≤0.4.x shape, and for the same reason: nothing
/// here should assert the new code's reconstruction against itself.
///
/// The property that matters is that the upgrade costs nothing: no live call,
/// and the entry settles under the new key so the probe is not spent again.
#[tokio::test]
async fn an_entry_keyed_before_base_url_left_the_key_is_adopted_without_a_live_call() {
    use domarinn_core::cache_key::request_cache_key;
    use domarinn_core::provider::{ProviderRequest, TestMeta};
    use domarinn_core::types::RenderedPrompt;

    const KEY: &str = "REQCFG_E2E_ADOPT_KEY";
    std::env::set_var(KEY, "sk-test");
    let server = answering_stub().await;
    let yaml = suite_with(&server.uri(), KEY, "");

    // A cold run, to get a real entry rather than a hand-built one.
    let cache = MemCache::default();
    run_suite(&yaml, &cache).await;
    let live_key = cache.only_key();
    let mut entry = cache.take(&live_key).expect("the cold run wrote one");

    // Ask the provider itself what it published before 0.8.0, and re-file that
    // entry under it. Nothing here asserts the new code's reconstruction against
    // itself: the shape comes from the provider, the key from the same function
    // the runner uses.
    let suite = domarinn_core::load_str(&yaml).unwrap();
    let provider =
        domarinn_core::provider_factory::build_provider(&suite.providers[0], None).unwrap();
    let req = ProviderRequest {
        prompt: Some(RenderedPrompt::Text("say hello".into())),
        vars: Default::default(),
        params: Default::default(),
        test: TestMeta::default(),
        case_salt: None,
        tools: Vec::new(),
    };
    let legacy = provider
        .legacy_canonical_requests(&req)
        .pop()
        .expect("the vendor providers publish one prior shape");
    assert!(
        legacy["url"]
            .as_str()
            .is_some_and(|u| u.starts_with(&server.uri())),
        "the prior shape carried the full base_url: {legacy}"
    );
    assert!(legacy.get("path").is_none(), "…and not the new member");

    entry.output = domarinn_core::types::Output::Text("from the 0.7 store".into());
    entry.request = Some(legacy.clone());
    let legacy_key = request_cache_key(&legacy, 0, None, None);
    assert_ne!(legacy_key.0, live_key.0, "the key really did move");
    cache.seed(&legacy_key, entry);

    let before = served(&server).await.len();
    let outcome = run_suite(&yaml, &cache).await;

    assert_eq!(
        outcome.cases[0].output.as_ref().unwrap().as_text(),
        "from the 0.7 store",
        "the stored answer is served, not a fresh call"
    );
    assert_eq!(outcome.summary.cache_hits, 1);
    assert_eq!(
        served(&server).await.len(),
        before,
        "an adopted entry costs no live call"
    );

    // Settled: re-filed under today's key, so the next run finds it directly.
    let again = run_suite(&yaml, &cache).await;
    assert_eq!(again.summary.cache_hits, 1);
    assert_eq!(served(&server).await.len(), before);
    assert!(
        cache.take(&live_key).is_some(),
        "adoption re-files under the live key rather than probing forever"
    );
}

/// The control for the test above: without the probe, that entry is simply
/// unreachable. Without this, the adoption test would pass just as well against
/// a store the runner found by some other route.
#[tokio::test]
async fn no_cache_migration_leaves_the_older_entry_unreachable() {
    use domarinn_core::cache_key::request_cache_key;
    use domarinn_core::provider::{ProviderRequest, TestMeta};
    use domarinn_core::types::RenderedPrompt;

    const KEY: &str = "REQCFG_E2E_NOADOPT_KEY";
    std::env::set_var(KEY, "sk-test");
    let server = answering_stub().await;
    let yaml = suite_with(&server.uri(), KEY, "");

    let cache = MemCache::default();
    run_suite(&yaml, &cache).await;
    let live_key = cache.only_key();
    let mut entry = cache.take(&live_key).expect("the cold run wrote one");

    let suite = domarinn_core::load_str(&yaml).unwrap();
    let provider =
        domarinn_core::provider_factory::build_provider(&suite.providers[0], None).unwrap();
    let legacy = provider
        .legacy_canonical_requests(&ProviderRequest {
            prompt: Some(RenderedPrompt::Text("say hello".into())),
            vars: Default::default(),
            params: Default::default(),
            test: TestMeta::default(),
            case_salt: None,
            tools: Vec::new(),
        })
        .pop()
        .unwrap();
    entry.output = domarinn_core::types::Output::Text("from the 0.7 store".into());
    cache.seed(&request_cache_key(&legacy, 0, None, None), entry);

    let before = served(&server).await.len();
    let suite = domarinn_core::load_str(&yaml).unwrap();
    let outcome = run(
        &suite,
        Path::new("."),
        &cache,
        None,
        &RunOptions {
            cache_migration: false,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(
        served(&server).await.len(),
        before + 1,
        "--no-cache-migration must re-pay: the older key is only reachable by probing"
    );
    assert_ne!(
        outcome.cases[0].output.as_ref().unwrap().as_text(),
        "from the 0.7 store"
    );
}

/// Two endpoints answering the same question share a cache, and `cache_salt` is
/// how a suite opts back out. The end-to-end form of the trade `base_url`'s
/// removal from the key makes.
#[tokio::test]
async fn a_gateway_shares_the_cache_and_a_salt_separates_it() {
    const KEY: &str = "REQCFG_E2E_SHARE_KEY";
    std::env::set_var(KEY, "sk-test");
    let direct = answering_stub().await;
    let gateway = answering_stub().await;
    let cache = MemCache::default();

    run_suite(&suite_with(&direct.uri(), KEY, ""), &cache).await;
    run_suite(&suite_with(&gateway.uri(), KEY, ""), &cache).await;
    assert!(
        served(&gateway).await.is_empty(),
        "the gateway is never called: it shares the direct connection's entries"
    );

    let salted = suite_with(&gateway.uri(), KEY, "    cache_salt: \"local-stub\"\n");
    run_suite(&salted, &cache).await;
    assert_eq!(
        served(&gateway).await.len(),
        1,
        "a salt stops the sharing outright, for endpoints that answer differently"
    );
}
