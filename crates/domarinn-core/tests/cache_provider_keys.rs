//! What separates, and what shares, for the three network-backed providers.
//!
//! `http` had no cache coverage at all, which is how it kept two stale-replay
//! bugs: `headers` were absent from the fingerprint, and `{{ env.X }}` changes
//! the request without changing the key. Both are the shape where a test is
//! worth most — the run *succeeds*, reports a plausible number, and is wrong.
//!
//! The ground truth throughout is the mock server's request count, not
//! `summary.cache_hits`. A hit is only a hit if nobody was called: counting
//! domarinn's own bookkeeping would let a broken key agree with itself.

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
use wiremock::{Mock, MockServer, ResponseTemplate};

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

async fn run_suite(yaml: &str, cache: &dyn CacheBackend) -> RunResult {
    let suite = domarinn_core::load_str(yaml).unwrap();
    run(&suite, Path::new("."), cache, None, &RunOptions::default())
        .await
        .unwrap()
}

/// A server that answers anything, so the only variable is whether it is asked.
async fn always_answers() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "resp_1",
            "model": "m",
            "content": [{"type": "text", "text": "hello"}],
            "choices": [{"message": {"role": "assistant", "content": "hello"}}],
            "usage": {"input_tokens": 10, "output_tokens": 5,
                      "prompt_tokens": 10, "completion_tokens": 5},
        })))
        .mount(&server)
        .await;
    server
}

async fn calls(server: &MockServer) -> usize {
    server.received_requests().await.unwrap().len()
}

/// Run `a` then `b` against one shared cache and report how many live calls the
/// second suite made. Zero means they share a key; more means they separate.
async fn live_calls_for_second(server: &MockServer, a: &str, b: &str) -> usize {
    let cache = MemCache::default();
    run_suite(a, &cache).await;
    let before = calls(server).await;
    run_suite(b, &cache).await;
    calls(server).await - before
}

// ── `http`: the two bugs this file exists for ────────────────────────────────

fn http_suite(uri: &str, headers: &str, body: &str) -> String {
    format!(
        r#"
version: 1
project: test
suite: keys
providers:
  - id: p
    type: http
    url: "{uri}/generate"
    method: post
    headers: {headers}
    body: {body}
tests:
  - id: case-a
    vars: {{x: "a"}}
"#
    )
}

/// Two providers differing only in a header must not share a cache entry.
///
/// This failed before `headers` joined the fingerprint. The failure mode is the
/// bad kind: the run succeeds, the second model's column is filled with the
/// first model's answers, and the comparison the suite exists to make reports a
/// difference of zero that it never measured.
#[tokio::test]
async fn http_providers_differing_only_in_a_header_do_not_share_a_key() {
    let server = always_answers().await;
    let with_model = |model: &str| {
        http_suite(
            &server.uri(),
            &format!(r#"{{X-Model: "{model}"}}"#),
            r#"{prompt: "{{ x }}"}"#,
        )
    };

    let second =
        live_calls_for_second(&server, &with_model("gpt-5"), &with_model("claude-opus-5")).await;
    assert_eq!(
        second, 1,
        "a different model header must be a different cache entry"
    );
}

/// …and the same header must still share, or the fix would have traded a stale
/// replay for a cache that never hits.
#[tokio::test]
async fn http_providers_with_the_same_headers_still_share() {
    let server = always_answers().await;
    let suite = http_suite(
        &server.uri(),
        r#"{X-Model: "gpt-5"}"#,
        r#"{prompt: "{{ x }}"}"#,
    );
    assert_eq!(live_calls_for_second(&server, &suite, &suite).await, 0);
}

/// A provider that declares no headers keeps the key it had before the member
/// existed. The `headers` member is inserted only when set, for exactly this
/// reason — an unconditional `null` would hash differently from an absent
/// member and re-key every headerless `http` provider in every store.
#[tokio::test]
async fn a_headerless_http_provider_is_unaffected_by_the_new_member() {
    use domarinn_core::cache::canonical_json;

    let server = always_answers().await;
    let yaml = format!(
        r#"
version: 1
project: test
suite: keys
providers:
  - id: p
    type: http
    url: "{}/generate"
    method: post
    body: {{prompt: "{{{{ x }}}}"}}
tests:
  - id: case-a
    vars: {{x: "a"}}
"#,
        server.uri()
    );
    let suite = domarinn_core::load_str(&yaml).unwrap();
    let provider =
        domarinn_core::provider_factory::build_provider(&suite.providers[0], None).unwrap();
    let fp = canonical_json(&provider.fingerprint());
    assert!(
        !fp.contains("headers"),
        "no header declared means no member at all: {fp}"
    );
}

/// The header digest must not leak the header values into the entry.
///
/// A fingerprint is persisted into every cache entry, and a header is precisely
/// where a bearer token sits — so this publishes a digest, never the map.
#[tokio::test]
async fn the_header_digest_does_not_leak_its_values() {
    use domarinn_core::cache::canonical_json;

    let yaml = r#"
version: 1
project: test
suite: keys
providers:
  - id: p
    type: http
    url: "http://example.invalid/generate"
    headers: {Authorization: "Bearer super-secret-value"}
tests:
  - id: case-a
    vars: {x: "a"}
"#;
    let suite = domarinn_core::load_str(yaml).unwrap();
    let provider =
        domarinn_core::provider_factory::build_provider(&suite.providers[0], None).unwrap();
    let fp = canonical_json(&provider.fingerprint());
    assert!(fp.contains("blake3:"), "{fp}");
    assert!(!fp.contains("super-secret-value"), "{fp}");
    assert!(!fp.contains("Authorization"), "{fp}");
}

/// `${env:VAR}` resolves at load time, so its value lands in the fingerprint.
///
/// This is the keyed way to vary an `http` provider from the environment, and
/// the reason the warning about `{{ env.X }}` can point somewhere useful rather
/// than just saying "careful".
#[tokio::test]
async fn load_time_env_interpolation_separates_http_providers() {
    let server = always_answers().await;
    let yaml = http_suite(
        &server.uri(),
        r#"{X-Model: "${env:DOMARINN_KEYS_MODEL}"}"#,
        r#"{prompt: "{{ x }}"}"#,
    );

    let cache = MemCache::default();
    std::env::set_var("DOMARINN_KEYS_MODEL", "gpt-5");
    run_suite(&yaml, &cache).await;
    let before = calls(&server).await;
    std::env::set_var("DOMARINN_KEYS_MODEL", "claude-opus-5");
    run_suite(&yaml, &cache).await;
    let after = calls(&server).await;
    std::env::remove_var("DOMARINN_KEYS_MODEL");

    assert_eq!(after - before, 1, "two models must not share one entry");
    assert_eq!(cache.entries(), 2);
}

/// The counterpart, documenting today's behaviour rather than wishing it away:
/// `{{ env.X }}` is rendered per request and is **not** in the key, so two
/// values do collide. domarinn cannot tell a model selector from a credential,
/// so it warns at construction and points at `${env:X}` instead of guessing.
#[tokio::test]
async fn runtime_env_templating_shares_a_key_and_is_warned_about() {
    let server = always_answers().await;
    let yaml = http_suite(
        &server.uri(),
        r#"{X-Model: "{{ env.DOMARINN_KEYS_RUNTIME }}"}"#,
        r#"{prompt: "{{ x }}"}"#,
    );

    let cache = MemCache::default();
    std::env::set_var("DOMARINN_KEYS_RUNTIME", "gpt-5");
    run_suite(&yaml, &cache).await;
    let before = calls(&server).await;
    std::env::set_var("DOMARINN_KEYS_RUNTIME", "claude-opus-5");
    run_suite(&yaml, &cache).await;
    let after = calls(&server).await;
    std::env::remove_var("DOMARINN_KEYS_RUNTIME");

    assert_eq!(
        after - before,
        0,
        "documenting the hazard: a runtime-rendered value is not in the key"
    );
    assert_eq!(cache.entries(), 1);
}

/// A request nobody can render the same way twice is not cached at all.
///
/// `uuid()` without a seed is a template error here by design — a persisted
/// render must be reproducible. What this pins is the *cache's* response to
/// that: no canonical request means no key, so the call goes live and unstored.
/// The alternative, keying on a value that changes per call, is the worst of
/// both: it never hits, and it grows the store by one dead entry per run
/// forever.
#[tokio::test]
async fn a_request_that_cannot_be_rendered_deterministically_is_never_keyed() {
    use domarinn_core::provider::ProviderRequest;

    let server = always_answers().await;
    let yaml = http_suite(&server.uri(), "{}", r#"{prompt: "{{ uuid() }}"}"#);
    let suite = domarinn_core::load_str(&yaml).unwrap();
    let provider =
        domarinn_core::provider_factory::build_provider(&suite.providers[0], None).unwrap();
    assert!(
        provider
            .canonical_request(&ProviderRequest::default())
            .is_none(),
        "an unrenderable request has no identity to key on"
    );

    let cache = MemCache::default();
    let result = run_suite(&yaml, &cache).await;
    assert_eq!(cache.entries(), 0, "no key, so nothing to store under one");
    // The live call renders the same templates and fails the same way, so this
    // surfaces as a case error rather than a silent uncached success. Recorded
    // because it is the behaviour, not because the cache depends on it.
    assert_eq!(
        result.cases[0].error_class.as_ref().map(|c| c.0.as_str()),
        Some("render_failed"),
        "the live call reports the render failure: {:?}",
        result.cases[0].error
    );
}

/// A `{{ env.X | default(…) }}` template keys differently on a machine where
/// `X` is unset — documented, because the direction it fails in is the safe one.
///
/// The canonical request renders against placeholder `env`, which enumerates the
/// *names* this process has: with `X` set the template resolves to the
/// `${env:X}` placeholder, and with it unset the filter's default wins. Two
/// keys, so two entries. That is a duplicate, never a false share — the failure
/// mode this whole file exists to prevent is one machine replaying another's
/// answers, and this cannot produce it. Worth a test rather than a note because
/// the *reverse* would be a silent stale replay, and a future change to how
/// placeholders are built could turn one into the other.
#[tokio::test]
async fn a_defaulted_env_template_partitions_rather_than_shares() {
    const VAR: &str = "DOMARINN_KEYS_DEFAULTED";
    let server = always_answers().await;
    let yaml = http_suite(
        &server.uri(),
        &format!(r#"{{X-Model: "{{{{ env.{VAR} | default('fallback') }}}}"}}"#),
        r#"{prompt: "{{ x }}"}"#,
    );

    let cache = MemCache::default();
    std::env::remove_var(VAR);
    run_suite(&yaml, &cache).await;
    let before = calls(&server).await;
    std::env::set_var(VAR, "anything at all");
    run_suite(&yaml, &cache).await;
    let after = calls(&server).await;
    std::env::remove_var(VAR);

    assert_eq!(
        after - before,
        1,
        "the two renders are different requests, so they are different entries"
    );
    assert_eq!(
        cache.entries(),
        2,
        "a duplicate entry is the cost; replaying the wrong answer is not"
    );
}

/// Everything else about an `http` provider that must separate.
#[tokio::test]
async fn http_url_method_and_body_each_bust() {
    let server = always_answers().await;
    let base = http_suite(&server.uri(), "{}", r#"{prompt: "{{ x }}"}"#);

    let other_body = http_suite(&server.uri(), "{}", r#"{prompt: "{{ x }}", top_k: 5}"#);
    assert_eq!(
        live_calls_for_second(&server, &base, &other_body).await,
        1,
        "a different request body is a different question"
    );

    let other_url = http_suite(&server.uri(), "{}", r#"{prompt: "{{ x }}"}"#)
        .replace("/generate", "/generate-v2");
    assert_eq!(
        live_calls_for_second(&server, &base, &other_url).await,
        1,
        "a different endpoint is a different provider"
    );
}

/// Editing an `output_expr` busts exactly that provider's entries.
///
/// It never goes on the wire, so the two runs below send byte-identical
/// requests — but an entry stores the *projected* output, so the expression
/// decides what the stored answer means. Without it in the key the second run
/// is a hit that replays the first projection and labels it as the second's:
/// the run succeeds, the numbers are plausible, and the field being asserted on
/// is not the one the suite now names.
#[tokio::test]
async fn editing_an_output_expr_busts_the_entries_it_projected() {
    let server = always_answers().await;
    let projecting = |expr: &str| {
        format!(
            r#"
version: 1
project: test
suite: keys
providers:
  - id: p
    type: http
    url: "{uri}/generate"
    method: post
    body: {{prompt: "{{{{ x }}}}"}}
    output_expr: "{expr}"
tests:
  - id: case-a
    vars: {{x: "a"}}
"#,
            uri = server.uri()
        )
    };

    let cache = MemCache::default();
    let first = run_suite(&projecting("response.json.id"), &cache).await;
    assert_eq!(first.cases[0].output.as_ref().unwrap().as_text(), "resp_1");

    let before = calls(&server).await;
    let second = run_suite(&projecting("response.json.model"), &cache).await;
    assert_eq!(
        calls(&server).await - before,
        1,
        "a re-projected response is a fresh call, not a replay"
    );
    assert_eq!(
        second.cases[0].output.as_ref().unwrap().as_text(),
        "m",
        "the new expression's projection, not the old one's"
    );
    assert_eq!(cache.entries(), 2);
}

// ── `anthropic` / `openai` ───────────────────────────────────────────────────

/// `key_env` is a parameter, not a constant, because these tests run in
/// parallel in one process and `std::env` is process-wide: a shared name meant
/// one test's cleanup pulled the credential out from under another's run. Each
/// test owns a name and never removes anyone else's.
fn vendor_suite(kind: &str, uri: &str, model: &str, params: &str, key_env: &str) -> String {
    format!(
        r#"
version: 1
project: test
suite: keys
providers:
  - id: p
    type: {kind}
    model: "{model}"
    base_url: "{uri}"
    api_key_env: {key_env}
    params: {params}
prompts:
  - id: only
    template: "say hello to {{{{ x }}}}"
tests:
  - id: case-a
    vars: {{x: "a"}}
"#
    )
}

#[tokio::test]
async fn a_vendor_provider_is_separated_by_model_and_params() {
    const KEY: &str = "DOMARINN_KEYS_VENDOR_KEY";
    std::env::set_var(KEY, "sk-test");
    let server = always_answers().await;
    let other = always_answers().await;

    for kind in ["anthropic", "openai"] {
        let base = vendor_suite(kind, &server.uri(), "model-a", "{}", KEY);

        assert_eq!(
            live_calls_for_second(
                &server,
                &base,
                &vendor_suite(kind, &server.uri(), "model-b", "{}", KEY)
            )
            .await,
            1,
            "{kind}: a different model must not replay"
        );

        assert_eq!(
            live_calls_for_second(
                &server,
                &base,
                &vendor_suite(kind, &server.uri(), "model-a", "{temperature: 0.7}", KEY)
            )
            .await,
            1,
            "{kind}: params change the request"
        );

        // …but a different base_url does *not*. Pointing a suite at a gateway
        // must not make it re-pay for answers it already has, so the second run
        // is served from cache and the other server is never called.
        let cache = MemCache::default();
        run_suite(&base, &cache).await;
        let before = calls(&other).await;
        run_suite(
            &vendor_suite(kind, &other.uri(), "model-a", "{}", KEY),
            &cache,
        )
        .await;
        assert_eq!(
            calls(&other).await - before,
            0,
            "{kind}: a gateway and a direct connection share the cache"
        );
    }
}

/// …and `cache_salt` is how a suite opts back out.
///
/// The safety valve for the property above. Two endpoints that answer the same
/// question should share entries; two that answer *differently* — a local stub
/// standing in for a vendor — must not, and since `base_url` no longer separates
/// them, this is the mechanism that does. A run also warns on the mismatch, but
/// a warning is a report and this is a guarantee.
#[tokio::test]
async fn a_cache_salt_separates_two_endpoints_that_share_a_key() {
    const KEY: &str = "DOMARINN_KEYS_SALT_KEY";
    std::env::set_var(KEY, "sk-test");
    let server = always_answers().await;

    for kind in ["anthropic", "openai"] {
        let plain = vendor_suite(kind, &server.uri(), "model-a", "{}", KEY);
        let salted = plain.replace(
            "    model: \"model-a\"",
            "    model: \"model-a\"\n    cache_salt: \"local-stub\"",
        );
        assert_ne!(plain, salted, "{kind}: the fixture must actually differ");

        assert_eq!(
            live_calls_for_second(&server, &plain, &salted).await,
            1,
            "{kind}: a salted provider does not replay an unsalted one's answers"
        );
    }
}

/// Two spellings of one gateway must not partition the cache.
///
/// `base_url` is a caller-authored string and both spellings resolve to the same
/// endpoint, so a trailing slash used to hand two teammates private halves of a
/// shared store — the fingerprint carried the string verbatim. Keying the
/// *resolved request* closes it: the url each provider would send is what the
/// hash sees, and `endpoint()` trims before it builds one. Covered as a unit in
/// Task 4; this is the same property through the live key path.
#[tokio::test]
async fn a_trailing_slash_on_base_url_shares_the_entry() {
    const KEY: &str = "DOMARINN_KEYS_SLASH_KEY";
    std::env::set_var(KEY, "sk-test");
    let server = always_answers().await;

    for kind in ["anthropic", "openai"] {
        let plain = vendor_suite(kind, &server.uri(), "model-a", "{}", KEY);
        let slashed = vendor_suite(kind, &format!("{}/", server.uri()), "model-a", "{}", KEY);
        assert_eq!(
            live_calls_for_second(&server, &plain, &slashed).await,
            0,
            "{kind}: one gateway spelled two ways is one question"
        );
    }
}

/// Which *variable* a credential is read from must not partition a shared cache.
///
/// Two teammates, two API keys, one answer to the same question. Keying the
/// credential channel would give each of them a private cache wearing the
/// clothes of a shared one.
#[tokio::test]
async fn the_credential_does_not_partition_the_cache() {
    const ONE: &str = "DOMARINN_KEYS_CRED_ONE";
    const TWO: &str = "DOMARINN_KEYS_CRED_TWO";
    std::env::set_var(ONE, "sk-one");
    std::env::set_var(TWO, "sk-two");
    let server = always_answers().await;

    let base = vendor_suite("anthropic", &server.uri(), "model-a", "{}", ONE);
    let other_env = vendor_suite("anthropic", &server.uri(), "model-a", "{}", TWO);

    assert_eq!(
        live_calls_for_second(&server, &base, &other_env).await,
        0,
        "a different credential is the same question"
    );
}

/// Declared tools are part of the request: a call offered a tool and one that
/// was not are different questions, and replaying one for the other reports "no
/// tools were called" for a call that was never given any.
///
/// Covered as units in `cache_key.rs`; this is the same property through a real
/// run, where the suite-level `tools:` block has to actually reach the request.
#[tokio::test]
async fn declaring_tools_busts_but_declaring_none_does_not() {
    const KEY: &str = "DOMARINN_KEYS_TOOLS_KEY";
    std::env::set_var(KEY, "sk-test");
    let server = always_answers().await;
    let with_tools = |tools: &str| {
        format!(
            r#"
version: 1
project: test
suite: keys
providers:
  - id: p
    type: anthropic
    model: "model-a"
    base_url: "{}"
    api_key_env: {KEY}
{tools}
prompts:
  - id: only
    template: "hello {{{{ x }}}}"
tests:
  - id: case-a
    vars: {{x: "a"}}
"#,
            server.uri()
        )
    };

    let none = with_tools("");
    let one = with_tools("tools:\n  - name: get_weather");
    let two = with_tools("tools:\n  - name: get_weather\n  - name: get_time");

    assert_eq!(
        live_calls_for_second(&server, &none, &one).await,
        1,
        "offering a tool changes the question"
    );
    assert_eq!(
        live_calls_for_second(&server, &one, &two).await,
        1,
        "so does offering a second one"
    );
    assert_eq!(
        live_calls_for_second(&server, &none, &none).await,
        0,
        "and declaring none is the absence of a declaration, not an empty one"
    );
}

/// A pricing edit re-costs a cached case without re-calling anybody.
///
/// `cost_usd` used to be frozen into the entry, so a warm suite reported
/// whatever the rate sheet said the day it first ran — forever, and a `cost:`
/// budget scored against it. Pricing is deliberately not in the key (that would
/// discard every entry the day a vendor changes a price), so it is applied on
/// read, exactly as a grading `threshold` is.
#[tokio::test]
async fn a_pricing_change_re_costs_a_cached_case_without_calling_again() {
    const KEY: &str = "DOMARINN_KEYS_PRICING_KEY";
    std::env::set_var(KEY, "sk-test");
    let server = always_answers().await;
    let priced = |input: f64| {
        format!(
            r#"
version: 1
project: test
suite: keys
providers:
  - id: p
    type: anthropic
    model: "model-a"
    base_url: "{}"
    api_key_env: {KEY}
    pricing: {{input_per_mtok: {input}, output_per_mtok: {input}}}
prompts:
  - id: only
    template: "hello {{{{ x }}}}"
tests:
  - id: case-a
    vars: {{x: "a"}}
"#,
            server.uri()
        )
    };

    let cache = MemCache::default();
    let cheap = run_suite(&priced(1.0), &cache).await;
    let first_cost = cheap.cases[0]
        .cost_usd
        .expect("a priced provider reports cost");

    // Ten times the price, same provider identity — pricing is not in the key,
    // so this must be a hit.
    let before = calls(&server).await;
    let dear = run_suite(&priced(10.0), &cache).await;
    assert_eq!(
        calls(&server).await - before,
        0,
        "a pricing edit must not re-run the suite"
    );
    assert!(dear.cases[0].cached);

    let second_cost = dear.cases[0].cost_usd.expect("still priced");
    assert!(
        second_cost > first_cost * 9.0,
        "the cached case must be re-costed at the current rate: {first_cost} then {second_cost}"
    );
}
