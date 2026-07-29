//! The two levers that turn verdict caching off, and how they compose.
//!
//! `cache.grader` (suite-wide, deprecated) and `--no-grader-cache`
//! (`RunOptions.grader_cache`, per run) are ANDed: either can disable verdict
//! caching, neither can force it on over the other's objection. Sibling of
//! `cache_integration.rs`, which pins that verdicts are cached at all.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use domarinn_core::cache::{
    CacheBackend, CacheEntry, CacheError, CacheKey, CacheStats, PurgeFilter,
};
use domarinn_core::runner::{run, RunOptions};
use domarinn_core::DefaultGrader;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Default)]
struct MemCache {
    map: Mutex<HashMap<String, CacheEntry>>,
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

/// A judge that always passes, so a second call is evidence of a cache miss and
/// nothing else.
async fn always_passes() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "stop_reason": "tool_use",
            "content": [{
                "type": "tool_use", "name": "submit_verdict",
                "input": {"reasoning": "ok", "pass": true, "score": 1.0}
            }]
        })))
        .mount(&server)
        .await;
    server
}

/// One LLM-graded case, with `cache_block` spliced in as the suite's `cache:`
/// section (empty for a suite that says nothing).
fn graded_suite(uri: &str, cache_block: &str) -> String {
    format!(
        r#"
version: 1
suite: grader-toggle
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{{\"output\":\"I cannot help\"}}'"]
    cache_salt: "v1"
grader:
  provider: {{type: anthropic, model: claude-x, base_url: "{uri}", api_key_env: DOMARINN_GRADER_TOGGLE_KEY}}
{cache_block}tests:
  - id: decline
    vars: {{}}
    assert:
      - {{type: llm-rubric, value: "declines the task"}}
"#
    )
}

/// How many times the judge is called across two runs sharing one cache.
///
/// Two runs rather than two cases, because `cache.grader` is documented as
/// disabling verdict caching "for every run of this suite" — a reuse that only
/// held within a process would not be the thing under test. One call means the
/// second run replayed the verdict; two means it re-paid the judge.
async fn judge_calls_over_two_runs(cache_block: &str, opts: RunOptions) -> usize {
    let server = always_passes().await;
    std::env::set_var("DOMARINN_GRADER_TOGGLE_KEY", "sk-test");
    let suite = domarinn_core::load_str(&graded_suite(&server.uri(), cache_block)).unwrap();
    let grader = DefaultGrader::new(suite.grader.clone());
    let cache = MemCache::default();

    for _ in 0..2 {
        let result = run(&suite, Path::new("."), &cache, Some(&grader), &opts)
            .await
            .unwrap();
        assert_eq!(result.cases.len(), 1, "the fixture runs exactly one case");
    }
    server.received_requests().await.unwrap().len()
}

#[tokio::test]
async fn verdicts_are_cached_when_the_suite_says_nothing() {
    assert_eq!(
        judge_calls_over_two_runs("", RunOptions::default()).await,
        1,
        "verdict caching is on by default"
    );
}

#[tokio::test]
async fn cache_grader_false_disables_verdict_caching() {
    assert_eq!(
        judge_calls_over_two_runs("cache: {grader: false}\n", RunOptions::default()).await,
        2,
        "`cache.grader: false` must re-grade on every run"
    );
}

/// `true` is what the field defaulted to, so it must be indistinguishable from
/// leaving it out — a deprecation that changed the meaning of the value people
/// already wrote would be a breaking change wearing a warning.
#[tokio::test]
async fn cache_grader_true_leaves_verdict_caching_on() {
    assert_eq!(
        judge_calls_over_two_runs("cache: {grader: true}\n", RunOptions::default()).await,
        1,
        "`cache.grader: true` must behave exactly as the default does"
    );
}

/// The precedence that makes `--no-grader-cache` usable: the flag disables
/// caching for the run it is passed to, whatever the suite asked for.
#[tokio::test]
async fn the_no_grader_cache_flag_wins_over_cache_grader_true() {
    let opts = RunOptions {
        grader_cache: false,
        ..Default::default()
    };
    assert_eq!(
        judge_calls_over_two_runs("cache: {grader: true}\n", opts).await,
        2,
        "the run flag must be able to disable what the suite enabled"
    );
}

// ── The deprecation warning ──────────────────────────────────────────────────

/// A `MakeWriter` that appends every line into a shared buffer.
#[derive(Clone)]
struct BufWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for BufWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufWriter {
    type Writer = BufWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Two ungraded cases, so "once per run" has teeth: a warning emitted per case
/// would show up twice.
const TWO_CASE_SUITE: &str = r#"
version: 1
suite: grader-deprecation
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"hi\"}'"]
CACHE_BLOCK
tests:
  - id: a
    vars: {x: "1"}
    assert: [{type: contains, value: "hi"}]
  - id: b
    vars: {x: "2"}
    assert: [{type: contains, value: "hi"}]
"#;

/// Run `TWO_CASE_SUITE` with `cache_block` spliced in and return everything it
/// logged. The capture subscriber is scoped to this thread, and the run happens
/// on a current-thread runtime inside that scope, so it stays inert for every
/// other test.
fn log_of_a_run(cache_block: &str) -> String {
    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_ansi(false)
        .with_writer(BufWriter(buf.clone()))
        .finish();
    let yaml = TWO_CASE_SUITE.replace("CACHE_BLOCK", cache_block);

    tracing::subscriber::with_default(subscriber, || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let suite = domarinn_core::load_str(&yaml).unwrap();
            let cache = MemCache::default();
            let result = run(&suite, Path::new("."), &cache, None, &RunOptions::default())
                .await
                .unwrap();
            assert_eq!(result.cases.len(), 2);
        });
    });

    let bytes = buf.lock().unwrap().clone();
    String::from_utf8(bytes).unwrap()
}

/// The warning lives in the runner rather than the CLI so that an embedder —
/// the server runs suites too — tells its users the same thing.
#[test]
fn setting_cache_grader_warns_once_per_run_that_it_is_deprecated() {
    let logged = log_of_a_run("cache: {grader: true}");
    assert_eq!(
        logged.matches("cache.grader is deprecated").count(),
        1,
        "one warning per run, not one per case; got: {logged}"
    );
    assert!(
        logged.contains("--no-grader-cache"),
        "the warning must name the replacement; got: {logged}"
    );
}

/// `false` is the value worth warning about most — it is the one people wrote
/// on purpose — so the warning is keyed on the field being *set*, not on which
/// way it was set.
#[test]
fn cache_grader_false_is_warned_about_too() {
    let logged = log_of_a_run("cache: {grader: false}");
    assert_eq!(logged.matches("cache.grader is deprecated").count(), 1);
}

/// A suite that never mentions the field has nothing to migrate, and a warning
/// it cannot act on is noise.
#[test]
fn a_suite_that_leaves_cache_grader_unset_is_not_warned() {
    let logged = log_of_a_run("");
    assert!(
        !logged.contains("cache.grader is deprecated"),
        "an unset field must not warn; got: {logged}"
    );
}
