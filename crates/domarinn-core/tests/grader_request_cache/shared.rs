//! Fixtures shared by `grader_request_cache.rs` and its
//! `grader_request_cache_tool_calls.rs` sibling.
//!
//! The two files are one subject split for the repo's 1000-line source cap, and
//! they have to agree on the *bytes* of the fixture suites: half the claims in
//! either are "this key did not change", which is only meaningful if both files
//! ask the identical question. Duplicating the YAML would let one copy drift and
//! quietly turn a re-keying test into a tautology, so it lives here once.
//!
//! Pulled in with `#[path]` from a directory rather than a bare `mod`: Cargo
//! auto-discovers `tests/*.rs` as separate binaries, so a sibling file here
//! would become a third test target with no tests in it. A `tests/…/` directory
//! with no `main.rs` is not a target — the same shape `domarinn-cli`'s
//! `tests/examples/` uses.
//!
//! Each test binary compiles this module separately and neither uses all of it,
//! so unused items are expected rather than rot.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use domarinn_core::cache::{
    CacheBackend, CacheEntry, CacheError, CacheKey, CacheStats, PurgeFilter,
};
use domarinn_core::cache_migrate::{
    legacy_graded_payload, legacy_grader_verdict_key, legacy_grading_fingerprint, LegacyGraded,
};
use domarinn_core::config::{Assert, AssertKind};
use domarinn_core::grader::SYSTEM_PROMPT;
use domarinn_core::runner::{run, RunOptions};
use domarinn_core::types::Output;
use domarinn_core::{DefaultGrader, RunResult};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A cache that remembers what it was *asked* for, not only what it holds.
///
/// The `gets` log is what makes "zero probes on the second run" observable:
/// adoption is invisible from the outside once it has happened, so the only
/// evidence that the budget stopped being spent is the lookup that no longer
/// occurs.
#[derive(Default)]
pub struct MemCache {
    map: Mutex<HashMap<String, CacheEntry>>,
    gets: Mutex<Vec<String>>,
}

impl MemCache {
    pub fn seed(&self, key: &CacheKey, entry: CacheEntry) {
        self.map.lock().unwrap().insert(key.0.clone(), entry);
    }
    pub fn asked_for(&self, key: &CacheKey) -> usize {
        self.gets
            .lock()
            .unwrap()
            .iter()
            .filter(|k| **k == key.0)
            .count()
    }
    pub fn forget_gets(&self) {
        self.gets.lock().unwrap().clear();
    }
    pub fn entries(&self) -> Vec<CacheEntry> {
        self.map.lock().unwrap().values().cloned().collect()
    }
    /// The first entry matching `pred`, with the key it lives under.
    pub fn find(&self, pred: impl Fn(&CacheEntry) -> bool) -> Option<(CacheKey, CacheEntry)> {
        self.map
            .lock()
            .unwrap()
            .iter()
            .find(|(_, e)| pred(e))
            .map(|(k, e)| (CacheKey(k.clone()), e.clone()))
    }
    /// Every entry matching `pred`, for the tests that count key spaces rather
    /// than inspect one entry.
    pub fn all(&self, pred: impl Fn(&CacheEntry) -> bool) -> Vec<CacheEntry> {
        self.map
            .lock()
            .unwrap()
            .values()
            .filter(|e| pred(e))
            .cloned()
            .collect()
    }
    /// Replace an entry in place. Only a test may do this — `put` is
    /// first-write-wins, which is exactly the property that makes an
    /// unparseable entry unfixable in the field.
    pub fn overwrite(&self, key: &CacheKey, entry: CacheEntry) {
        self.map.lock().unwrap().insert(key.0.clone(), entry);
    }
}

#[async_trait]
impl CacheBackend for MemCache {
    async fn get(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        self.gets.lock().unwrap().push(key.0.clone());
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

pub async fn run_suite(
    yaml: &str,
    base_dir: &Path,
    cache: &MemCache,
    opts: &RunOptions,
) -> RunResult {
    let suite = domarinn_core::load_str(yaml).unwrap();
    let mut grader = DefaultGrader::new(suite.grader.clone());
    if let Some(embeddings) = domarinn_core::provider_factory::build_embeddings(&suite) {
        grader = grader.with_embeddings(embeddings);
    }
    run(&suite, base_dir, cache, Some(&grader), opts)
        .await
        .unwrap()
}

/// A judge that always passes, so a second call is evidence of a cache miss and
/// nothing else.
pub async fn always_passes() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "claude-x-20260101",
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 10, "output_tokens": 5},
            "content": [{
                "type": "tool_use", "name": "submit_verdict",
                "input": {"reasoning": "live verdict", "pass": true, "score": 1.0}
            }]
        })))
        .mount(&server)
        .await;
    server
}

pub async fn judge_calls(server: &MockServer) -> usize {
    server.received_requests().await.unwrap().len()
}

/// Serializes the tests that *mutate* the process environment against the ones
/// that depend on a snapshot of it.
///
/// The exec-adoption tests derive a ≤0.4.x key over the render context, which
/// carries every environment variable — so a sibling calling `set_var` between
/// the derivation and the run would move the key out from under them. Tests in
/// one integration binary share a process and run in parallel, so this is a real
/// race rather than a theoretical one; the file runs in well under a second, so
/// serializing the env-touching subset costs nothing.
///
/// A `tokio::sync::Mutex` rather than a `std` one because the holders await
/// across it — a blocking guard held over an await is the shape that deadlocks
/// a single-threaded runtime, and `clippy::await_holding_lock` is right to say
/// so even where these particular tests would have got away with it.
///
/// One lock per test *binary*, which is all that is needed: the two binaries are
/// separate processes with separate environments.
pub static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Set an environment variable without racing the snapshot-dependent tests.
pub async fn set_env(key: &str, value: &str) {
    let _guard = ENV_LOCK.lock().await;
    std::env::set_var(key, value);
}

// ── The system under test ────────────────────────────────────────────────────
//
// Each of these is the body of an `sh -c` that reads its request and prints one
// `ProviderResp` document. They are `const` rather than inline in the YAML so
// that a tool-calling variant differs from its tool-less twin in *exactly* the
// `tool_calls` member: every re-keying claim in the sibling file is an argument
// about that one difference, and an accidentally-edited `output` would prove the
// same thing for the wrong reason.

/// Prints a refusal and calls nothing.
pub const SUT_DECLINES: &str = r#"cat >/dev/null; printf '{\"output\":\"I cannot help\"}'"#;

/// The same refusal, with one tool call alongside it.
pub const SUT_DECLINES_AFTER_A_CALL: &str = r#"cat >/dev/null; printf '{\"output\":\"I cannot help\",\"tool_calls\":[{\"id\":\"toolu_01ABCDEF\",\"name\":\"get_weather\",\"arguments\":{\"city\":\"Oslo\"}}]}'"#;

/// The exec-assert fixture's SUT: a fixed output, no calls.
pub const SUT_SAME: &str = r#"cat >/dev/null; printf '{\"output\":\"same\"}'"#;

/// …and the same output, reached by calling a tool.
pub const SUT_SAME_AFTER_A_CALL: &str = r#"cat >/dev/null; printf '{\"output\":\"same\",\"tool_calls\":[{\"id\":\"toolu_01ABCDEF\",\"name\":\"lookup_capital\",\"arguments\":{\"country\":\"France\"}}]}'"#;

/// One rubric-graded case over a fixed exec SUT.
pub fn rubric_suite(uri: &str, template: Option<&str>) -> String {
    rubric_suite_over(uri, template, SUT_DECLINES, false)
}

/// The same, with the SUT and the `include_tool_calls` opt-in as knobs.
///
/// With the defaults [`rubric_suite`] passes, this emits the byte-identical YAML
/// it always did — `include_tool_calls` is absent from the document rather than
/// written as `false`, because a suite that never mentions the flag is the case
/// whose judge requests must not move.
pub fn rubric_suite_over(
    uri: &str,
    template: Option<&str>,
    sut: &str,
    include_tool_calls: bool,
) -> String {
    let template = template
        .map(|t| format!("\n  template: \"{t}\""))
        .unwrap_or_default();
    let opt_in = if include_tool_calls {
        "\n  include_tool_calls: true"
    } else {
        ""
    };
    format!(
        r#"
version: 1
suite: grader-request-cache
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "{sut}"]
    cache_salt: "v1"
grader:
  provider: {{type: anthropic, model: claude-x, base_url: "{uri}", api_key_env: DOMARINN_REQCACHE_KEY}}{template}{opt_in}
tests:
  - id: decline
    vars: {{}}
    assert:
      - {{type: llm-rubric, value: "declines the task"}}
"#
    )
}

/// The assert the fixture grades, as a value — for deriving the ≤0.4.x key a
/// seeded entry has to live under.
pub fn rubric_assert() -> Assert {
    Assert {
        weight: 1.0,
        negate: false,
        kind: AssertKind::LlmRubric {
            value: "declines the task".into(),
            grader: None,
            threshold: None,
            params: None,
        },
    }
}

// ── exec asserts ─────────────────────────────────────────────────────────────

/// A counter script that answers every assert request identically and records
/// that it was asked.
pub fn counting_judge(dir: &Path) -> (String, std::path::PathBuf) {
    let counter = dir.join("calls");
    let judge = dir.join("judge.sh");
    std::fs::write(
        &judge,
        format!(
            "#!/bin/sh\ncat >/dev/null\necho x >> {counter}\nprintf '{{\"pass\":true,\"score\":1.0,\"reason\":\"child says ok\"}}'\n",
            counter = counter.display()
        ),
    )
    .unwrap();
    (judge.display().to_string(), counter)
}

pub fn calls(counter: &Path) -> usize {
    std::fs::read_to_string(counter)
        .unwrap_or_default()
        .lines()
        .count()
}

pub fn exec_assert_suite(judge: &str, salt: Option<&str>) -> String {
    exec_assert_suite_over(judge, salt, SUT_SAME)
}

/// The same, with the SUT as a knob — so a tool-calling cell can be graded by
/// the byte-identical assert.
pub fn exec_assert_suite_over(judge: &str, salt: Option<&str>, sut: &str) -> String {
    let salt = salt
        .map(|s| format!(", cache_salt: \"{s}\""))
        .unwrap_or_default();
    format!(
        r#"
version: 1
suite: exec-assert-cache
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "{sut}"]
    cache_salt: "v1"
tests:
  - id: t
    vars: {{expected: Paris}}
    assert: [{{type: exec, command: ["sh", "{judge}"]{salt}}}]
"#
    )
}

// ── Adoption ─────────────────────────────────────────────────────────────────

/// The ≤0.4.x key one seeded verdict has to live under.
pub fn legacy_rubric_key(output: &Output, grader: &domarinn_core::config::Grader) -> CacheKey {
    let assert = rubric_assert();
    let vars = json!({});
    let fingerprint =
        legacy_grading_fingerprint(&assert, Some(grader), SYSTEM_PROMPT, Some(Path::new(".")))
            .expect("an llm-rubric assert with a grader had a fingerprint");
    let graded = legacy_graded_payload(
        &assert,
        &LegacyGraded {
            output,
            rubric: "declines the task",
            vars: &vars,
            test_id: "decline",
            test_tags: &[],
            provider_id: "p",
        },
    )
    .expect("llm-rubric is adopted");
    legacy_grader_verdict_key(&fingerprint, &graded, 0)
}

pub fn verdict_entry(reasoning: &str) -> CacheEntry {
    serde_json::from_value(json!({
        "created_at": "2026-01-01T00:00:00Z",
        "provider_fingerprint": {"assert": "llm-rubric"},
        "output": reasoning,
        "cost_usd": 0.25,
        "verdict": {"kind": "rubric", "score": 1.0, "pass": true, "reasoning": reasoning},
        "domarinn_version": "0.4.0",
    }))
    .expect("a 0.4.x verdict entry")
}

/// The ≤0.4.x key an `exec` assert's verdict lived under, derived the way the
/// runtime derives it.
///
/// The fiddly one, and the reason this is worth a test of its own rather than
/// trust in the goldens: the payload has six members, and one of them is the
/// *render context* — the case's rendered vars plus a snapshot of the whole
/// process environment, which is what `evaluate_asserts` grades with. Getting
/// that object wrong strands every `exec` verdict in every 0.4 store, silently.
/// [`ENV_LOCK`] is what keeps the snapshot still between here and the run.
pub fn legacy_exec_key(judge: &str, base_dir: &Path, output: &Output) -> CacheKey {
    let assert = Assert {
        weight: 1.0,
        negate: false,
        kind: AssertKind::Exec {
            command: vec!["sh".into(), judge.to_string()],
            config: None,
            cache_salt: None,
        },
    };
    let mut case_vars = serde_json::Map::new();
    case_vars.insert("expected".into(), json!("Paris"));
    let vars = domarinn_core::render::context_with_env(&case_vars);
    // No `grader:` block in the exec fixture, so no default grader — exactly
    // what `DefaultGrader::new(None)` passes at runtime.
    let fingerprint = legacy_grading_fingerprint(&assert, None, SYSTEM_PROMPT, Some(base_dir))
        .expect("an exec assert always had a fingerprint");
    let graded = legacy_graded_payload(
        &assert,
        &LegacyGraded {
            output,
            rubric: "",
            vars: &vars,
            test_id: "t",
            test_tags: &[],
            provider_id: "p",
        },
    )
    .expect("exec is adopted");
    legacy_grader_verdict_key(&fingerprint, &graded, 0)
}

pub fn exec_verdict_entry(reason: &str) -> CacheEntry {
    serde_json::from_value(json!({
        "created_at": "2026-01-01T00:00:00Z",
        "provider_fingerprint": {"assert": "exec"},
        "output": reason,
        "verdict": {"kind": "exec", "pass": true, "score": 1.0, "reason": reason},
        "domarinn_version": "0.4.0",
    }))
    .expect("a 0.4.x exec verdict entry")
}

/// True for entries written by an `exec` *assert*'s protocol exchange.
///
/// The transport alone is not enough to say so: the fixtures' system under test
/// is itself an `exec` provider, so a run writes two `"transport": "exec"`
/// entries and only one of them is the assert. The envelope's `kind` is what
/// separates them, which is one of the reasons it is kept in the hashed
/// document.
pub fn is_exec_assert_entry(entry: &CacheEntry) -> bool {
    entry
        .request
        .as_ref()
        .is_some_and(|r| r["transport"] == "exec" && r["stdin"]["domarinn"]["kind"] == "assert")
}
