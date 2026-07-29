//! What a grader's cache key has to separate.
//!
//! Grader caching is on by default, so every gap in this key is a *silent*
//! wrong answer rather than a slow run: the second case is reported PASS with
//! `cached: true`, the judge is never asked, and nothing in the output says the
//! question it answered was a different one.
//!
//! Sibling of `grader_request_cache.rs`, which covers what a grader entry *is*
//! and what a warm one replays, and of `cache_integration.rs`, which covers the
//! provider-response cache. This file covers the inverse property — the cases
//! that must not share an entry.
//!
//! Every claim here is re-plumbed since 0.5.0 and survives unchanged, which is
//! the point of pinning behaviour rather than mechanism: the separations used to
//! be enforced by a hand-built `graded_payload` hashed alongside a grader
//! fingerprint, and now fall out of the request itself.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use domarinn_core::cache::{
    CacheBackend, CacheEntry, CacheError, CacheKey, CacheMode, CacheStats, PurgeFilter,
};
use domarinn_core::result::CaseStatus;
use domarinn_core::runner::{run, RunOptions};
use domarinn_core::{DefaultGrader, RunResult};
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

async fn run_suite(yaml: &str, base_dir: &Path, cache: &MemCache) -> RunResult {
    let suite = domarinn_core::load_str(yaml).unwrap();
    let grader = DefaultGrader::new(suite.grader.clone());
    run(
        &suite,
        base_dir,
        cache,
        Some(&grader),
        &RunOptions::default(),
    )
    .await
    .unwrap()
}

/// A judge that always passes, so the *only* thing a second call can be
/// evidence of is a cache miss.
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

/// Two cases whose rubrics render differently ask the judge different
/// questions, so they never share an entry.
///
/// Historically this was a bug — the key hashed `AssertKind::LlmRubric.value`,
/// the *template*, so two cases of one matrix collapsed onto a single entry
/// whenever their outputs matched, and the second reported PASS from a verdict
/// about the first case's rubric. Since 0.5.0 the separation is structural: the
/// key is the judge's request body, and the rendered rubric is what that body
/// says.
#[tokio::test]
async fn cases_whose_rendered_rubrics_differ_do_not_share_a_verdict() {
    let server = always_passes().await;
    std::env::set_var("DOMARINN_RUBRIC_KEY_TEST", "sk-test");
    let yaml = format!(
        r#"
version: 1
suite: rubric-key
providers:
  - id: p
    type: exec
    # The same output for both cases: that is the collision's precondition.
    command: ["sh", "-c", "cat >/dev/null; printf '{{\"output\":\"Oslo\"}}'"]
    cache_salt: "v1"
grader:
  provider: {{type: anthropic, model: claude-x, base_url: "{uri}", api_key_env: DOMARINN_RUBRIC_KEY_TEST}}
tests:
  - id: norway
    vars: {{country: Norway}}
    assert: [{{type: llm-rubric, value: "names the capital of {{{{ country }}}}"}}]
  - id: france
    vars: {{country: France}}
    assert: [{{type: llm-rubric, value: "names the capital of {{{{ country }}}}"}}]
"#,
        uri = server.uri()
    );

    let cache = MemCache::default();
    let result = run_suite(&yaml, Path::new("."), &cache).await;
    assert_eq!(result.cases.len(), 2);

    assert_eq!(
        server.received_requests().await.unwrap().len(),
        2,
        "each case asks about its own rendered rubric; sharing one verdict \
         reports a judgement of a different question"
    );
    assert!(
        result.cases.iter().all(|c| !c.asserts[0].cached),
        "neither verdict is a replay of the other"
    );
}

/// …and the flip side: an identical rendered rubric over identical output still
/// reuses, so fixing the collision did not turn verdict caching off.
#[tokio::test]
async fn cases_whose_rendered_rubrics_match_still_share_a_verdict() {
    let server = always_passes().await;
    std::env::set_var("DOMARINN_RUBRIC_KEY_TEST", "sk-test");
    let yaml = format!(
        r#"
version: 1
suite: rubric-key
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{{\"output\":\"Oslo\"}}'"]
    cache_salt: "v1"
grader:
  provider: {{type: anthropic, model: claude-x, base_url: "{uri}", api_key_env: DOMARINN_RUBRIC_KEY_TEST}}
tests:
  - id: a
    vars: {{country: Norway}}
    assert: [{{type: llm-rubric, value: "names the capital of {{{{ country }}}}"}}]
  - id: b
    vars: {{country: Norway}}
    assert: [{{type: llm-rubric, value: "names the capital of {{{{ country }}}}"}}]
"#,
        uri = server.uri()
    );

    let cache = MemCache::default();
    let result = run_suite(&yaml, Path::new("."), &cache).await;
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
        "same judge, same rubric, same output — one verdict"
    );
    assert!(result.cases.iter().any(|c| c.asserts[0].cached));
}

/// An `exec` grader child is given the case's `vars`, so two cells with equal
/// output but different vars are not the same question and must not share an
/// entry.
///
/// Narrowed in 0.5.0, and the narrowing is deliberate. The key is now the
/// document written to the child's stdin, from which `test` is stripped — a
/// test id and its tags are correlation metadata, exactly as on the provider
/// side, and two cases asking the same thing must keep sharing an entry. So what
/// separates these two cells is `vars.expected`, not their ids. A suite that
/// genuinely wants per-case separation with identical inputs says so with a
/// `cache_salt`.
#[tokio::test]
async fn exec_verdicts_are_separated_by_the_vars_the_child_was_told_about() {
    let dir = tempfile::tempdir().unwrap();
    let counter = dir.path().join("calls");
    let judge = dir.path().join("judge.sh");
    std::fs::write(
        &judge,
        format!(
            "#!/bin/sh\ncat >/dev/null\necho x >> {counter}\nprintf '{{\"pass\":true,\"score\":1.0,\"reason\":\"ok\"}}'\n",
            counter = counter.display()
        ),
    )
    .unwrap();

    let yaml = format!(
        r#"
version: 1
suite: exec-verdict-key
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{{\"output\":\"same\"}}'"]
    cache_salt: "v1"
tests:
  - id: paris
    vars: {{expected: Paris}}
    assert: [{{type: exec, command: ["sh", "{judge}"]}}]
  - id: rome
    vars: {{expected: Rome}}
    assert: [{{type: exec, command: ["sh", "{judge}"]}}]
"#,
        judge = judge.display()
    );

    let cache = MemCache::default();
    let result = run_suite(&yaml, dir.path(), &cache).await;
    assert!(result.cases.iter().all(|c| c.status == CaseStatus::Pass));

    let calls = std::fs::read_to_string(&counter)
        .unwrap_or_default()
        .lines()
        .count();
    assert_eq!(
        calls, 2,
        "the child is asked about each case; one verdict for both means the \
         Rome case was graded against Paris"
    );
}

/// `--cache-only` is the documented way to replay a warm cache offline, in CI,
/// with no secrets in the environment. Preflight demanding a live credential the
/// run will never read turned that into exit 2 before the first lookup.
#[tokio::test]
async fn cache_only_does_not_demand_credentials_it_will_never_read() {
    std::env::remove_var("DOMARINN_OFFLINE_TEST_KEY");
    let yaml = r#"
version: 1
suite: offline
providers:
  - {id: claude, type: anthropic, model: m, api_key_env: DOMARINN_OFFLINE_TEST_KEY}
prompts:
  - {id: ask, template: "hi {{ x }}"}
tests:
  - id: t
    vars: {x: "1"}
    assert: [{type: contains, value: "anything"}]
"#;
    let suite = domarinn_core::load_str(yaml).unwrap();
    let cache = MemCache::default();
    let err = run(
        &suite,
        Path::new("."),
        &cache,
        None,
        &RunOptions {
            cache_mode: CacheMode::ReadOnlyStrict,
            ..RunOptions::default()
        },
    )
    .await;

    // The run still fails — the cache is empty — but on the *cache miss*, which
    // is a fact about this cache, not on a credential nothing would have read.
    match err {
        Err(e) => panic!("a cache-only run must reach the cache, got: {e}"),
        Ok(result) => assert_eq!(
            result.cases[0].status,
            CaseStatus::Error,
            "an empty cache is a per-case miss, not a pre-run credential abort"
        ),
    }
}

/// Preflight's stated property is that it checks only what the run will use. It
/// was handed the *unfiltered* tests, so `--tag smoke` over five local-only
/// cases still demanded the judge key for the 195 rubric cases it excluded.
#[tokio::test]
async fn a_filtered_out_rubric_does_not_demand_the_graders_credential() {
    std::env::remove_var("DOMARINN_FILTERED_GRADER_KEY");
    let yaml = r#"
version: 1
suite: filtered
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"hello\"}'"]
    cache_salt: "v1"
grader:
  provider: {type: anthropic, model: judge, api_key_env: DOMARINN_FILTERED_GRADER_KEY}
tests:
  - id: smoke
    tags: [smoke]
    vars: {}
    assert: [{type: contains, value: "hello"}]
  - id: graded
    tags: [full]
    vars: {}
    assert: [{type: llm-rubric, value: "is polite"}]
"#;
    let suite = domarinn_core::load_str(yaml).unwrap();
    let cache = MemCache::default();
    let mut opts = RunOptions::default();
    opts.filter.tags = vec!["smoke".into()];

    let result = run(&suite, Path::new("."), &cache, None, &opts)
        .await
        .expect("a tag-filtered run must not require a key nothing will read");
    assert_eq!(result.cases.len(), 1);
    assert_eq!(result.cases[0].status, CaseStatus::Pass);
}

/// …and the property it protects still holds: a rubric that *survives* the
/// filter still fails the run up front rather than 401-ing every case.
#[tokio::test]
async fn a_surviving_rubric_still_demands_the_graders_credential() {
    std::env::remove_var("DOMARINN_SURVIVING_GRADER_KEY");
    let yaml = r#"
version: 1
suite: filtered
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"hello\"}'"]
    cache_salt: "v1"
grader:
  provider: {type: anthropic, model: judge, api_key_env: DOMARINN_SURVIVING_GRADER_KEY}
tests:
  - id: graded
    tags: [full]
    vars: {}
    assert: [{type: llm-rubric, value: "is polite"}]
"#;
    let suite = domarinn_core::load_str(yaml).unwrap();
    let cache = MemCache::default();
    let err = run(&suite, Path::new("."), &cache, None, &RunOptions::default())
        .await
        .expect_err("the judge key is genuinely needed here");
    assert!(
        err.to_string().contains("DOMARINN_SURVIVING_GRADER_KEY"),
        "{err}"
    );
}

/// A generator's cases are resolved after `expand_tests`, so everything that
/// function does has to be re-applied to them — or a generated case is a
/// second-class one.
///
/// `docs/caching.md` tells generator authors to emit a `cache_salt` per case and
/// documents `$digest:` as the way to compute one, but the literal string used
/// to survive as the salt: one constant shared by every generated case, which
/// never moves when the file it names is edited. Observed here through the cache
/// itself — edit one prompt, and exactly the case that digests it must miss.
#[tokio::test]
async fn a_generator_emitted_digest_salt_busts_only_the_case_that_names_it() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("prompts")).unwrap();
    std::fs::write(dir.path().join("prompts/a.md"), "alpha").unwrap();
    std::fs::write(dir.path().join("prompts/b.md"), "beta").unwrap();

    let generated = r#"{\"tests\":[{\"id\":\"a\",\"vars\":{\"id\":\"a\"},\"cache_salt\":\"$digest: prompts/{{ id }}.md\"},{\"id\":\"b\",\"vars\":{\"id\":\"b\"},\"cache_salt\":\"$digest: prompts/{{ id }}.md\"}]}"#;
    let yaml = format!(
        r#"
version: 1
suite: generated-salts
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{{\"output\":\"ok\"}}'"]
    cache_salt: "v1"
tests:
  - generator:
      command: ["sh", "-c", "cat >/dev/null; printf '{generated}'"]
"#
    );

    let cache = MemCache::default();
    let cold = run_suite(&yaml, dir.path(), &cache).await;
    assert_eq!(cold.cases.len(), 2);
    assert_eq!(cold.summary.cache_hits, 0, "cold run");

    // Nothing changed: both cases replay.
    let warm = run_suite(&yaml, dir.path(), &cache).await;
    assert_eq!(warm.summary.cache_hits, 2, "sanity: a warm cache is reused");

    // One prompt edited. With the salt resolved, exactly that case misses; with
    // the `$digest:` marker left as a literal string, both would still hit.
    std::fs::write(dir.path().join("prompts/a.md"), "alpha edited").unwrap();
    let edited = run_suite(&yaml, dir.path(), &cache).await;
    assert_eq!(
        edited.summary.cache_hits, 1,
        "editing one digested prompt must bust exactly its own case"
    );
}

/// The same gap for `$file`: a generated assertion's `{$file: …}` schema left
/// unresolved is a JSON Schema of entirely unknown keywords, and validates
/// *everything* — a green check over a document that does not match.
#[tokio::test]
async fn a_generator_emitted_file_schema_is_resolved_before_it_is_validated() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("schema.json"),
        r#"{"type": "object", "required": ["missing_key"]}"#,
    )
    .unwrap();

    // Written to disk rather than embedded in the suite, so the shell quoting
    // stays readable and the JSON stays the JSON a real generator would emit.
    let gen = dir.path().join("gen.sh");
    std::fs::write(
        &gen,
        r#"#!/bin/sh
cat >/dev/null
cat <<'JSON'
{"tests":[{"id":"t","assert":[{"type":"contains-json","schema":{"$file":"schema.json"}}]}]}
JSON
"#,
    )
    .unwrap();
    let sut = dir.path().join("sut.sh");
    std::fs::write(
        &sut,
        "#!/bin/sh\ncat >/dev/null\ncat <<'JSON'\n{\"output\":\"{\\\"present\\\": 1}\"}\nJSON\n",
    )
    .unwrap();

    let yaml = format!(
        r#"
version: 1
suite: generated-file-schema
providers:
  - id: p
    type: exec
    command: ["sh", "{sut}"]
    cache_salt: "v1"
tests:
  - generator:
      command: ["sh", "{gen}"]
"#,
        sut = sut.display(),
        gen = gen.display()
    );

    let result = run_suite(&yaml, dir.path(), &MemCache::default()).await;
    assert_eq!(result.cases.len(), 1);
    let case = &result.cases[0];
    assert_eq!(
        case.status,
        CaseStatus::Fail,
        "the output lacks `missing_key`; a pass here means the marker object was \
         compiled as the schema and matched everything. case: {case:?}"
    );
}
