//! End-to-end runner tests against an in-memory cache and exec providers.
//!
//! These exercise the whole `run()` path offline: matrix expansion, provider
//! calls, caching, deterministic assertions, short-circuiting, and summary.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use domarinn_core::cache::CacheMode;
use domarinn_core::cache::{
    CacheBackend, CacheEntry, CacheError, CacheKey, CacheStats, PurgeFilter,
};
use domarinn_core::progress::{ProgressEvent, ProgressSink};
use domarinn_core::result::{AssertStatus, CaseStatus};
use domarinn_core::runner::{run, run_with_progress, RunOptions};
use domarinn_core::DefaultGrader;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A minimal in-memory cache for tests.
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
        Ok(CacheStats {
            entries: self.map.lock().unwrap().len() as u64,
            ..Default::default()
        })
    }
    async fn purge(&self, _filter: &PurgeFilter) -> Result<u64, CacheError> {
        Ok(0)
    }
}

/// A provider that echoes a fixed string, with an optional cache_salt.
fn fixed_output_suite(output: &str, cacheable: bool, assert_yaml: &str) -> String {
    let salt = if cacheable { "cache_salt: v1" } else { "" };
    format!(
        r#"
version: 1
project: test
suite: s
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{{\"output\":\"{output}\"}}'"]
    {salt}
tests:
  - id: t1
    vars: {{x: "1"}}
    assert:
{assert_yaml}
"#
    )
}

async fn run_suite(
    yaml: &str,
    opts: RunOptions,
    cache: &dyn CacheBackend,
) -> domarinn_core::RunResult {
    let suite = domarinn_core::load_str(yaml).unwrap();
    run(&suite, Path::new("."), cache, None, &opts)
        .await
        .unwrap()
}

#[tokio::test]
async fn basic_pass() {
    let yaml = fixed_output_suite(
        "hello world",
        false,
        "      - {type: contains, value: \"hello\"}",
    );
    let cache = MemCache::default();
    let result = run_suite(&yaml, RunOptions::default(), &cache).await;
    assert_eq!(result.cases.len(), 1);
    assert_eq!(result.cases[0].status, CaseStatus::Pass);
    assert_eq!(result.summary.passed, 1);
    assert_eq!(result.summary.total, 1);
}

#[tokio::test]
async fn failing_assert_produces_fail() {
    let yaml = fixed_output_suite(
        "hello",
        false,
        "      - {type: contains, value: \"goodbye\"}",
    );
    let cache = MemCache::default();
    let result = run_suite(&yaml, RunOptions::default(), &cache).await;
    assert_eq!(result.cases[0].status, CaseStatus::Fail);
    assert_eq!(result.summary.failed, 1);
}

#[tokio::test]
async fn cache_hit_on_second_run() {
    let yaml = fixed_output_suite(
        "cached",
        true,
        "      - {type: contains, value: \"cached\"}",
    );
    let cache = MemCache::default();

    let first = run_suite(&yaml, RunOptions::default(), &cache).await;
    assert_eq!(first.summary.cache_hits, 0);
    assert_eq!(first.summary.cache_misses, 1);

    let second = run_suite(&yaml, RunOptions::default(), &cache).await;
    assert_eq!(
        second.summary.cache_hits, 1,
        "second run should hit the cache"
    );
    assert!(second.cases[0].cached);
}

#[tokio::test]
async fn cache_key_is_stable_across_unrelated_env_changes() {
    // The request identity (cache key) must not include the process environment,
    // or unrelated env changes (and different machines) would never share cache.
    let yaml = fixed_output_suite(
        "cached",
        true,
        "      - {type: contains, value: \"cached\"}",
    );
    let cache = MemCache::default();
    let first = run_suite(&yaml, RunOptions::default(), &cache).await;
    assert_eq!(first.summary.cache_hits, 0);

    std::env::set_var("DOMARINN_UNRELATED_ENV_PROBE", "changed-between-runs");
    let second = run_suite(&yaml, RunOptions::default(), &cache).await;
    std::env::remove_var("DOMARINN_UNRELATED_ENV_PROBE");

    assert_eq!(
        second.summary.cache_hits, 1,
        "an unrelated environment change must not bust the cache"
    );
}

#[tokio::test]
async fn no_cache_mode_never_hits() {
    let yaml = fixed_output_suite("x", true, "      - {type: contains, value: \"x\"}");
    let cache = MemCache::default();
    let opts = RunOptions {
        cache_mode: CacheMode::Disabled,
        ..Default::default()
    };
    run_suite(&yaml, opts.clone(), &cache).await;
    let second = run_suite(&yaml, opts, &cache).await;
    assert_eq!(second.summary.cache_hits, 0);
}

#[tokio::test]
async fn deferred_assert_without_grader_fails_closed() {
    // An llm-rubric assert with no grader must error (never silently pass).
    let yaml = fixed_output_suite(
        "anything",
        false,
        "      - {type: llm-rubric, value: \"is good\"}",
    );
    let cache = MemCache::default();
    let result = run_suite(&yaml, RunOptions::default(), &cache).await;
    assert_eq!(result.cases[0].status, CaseStatus::Error);
    assert_eq!(result.cases[0].asserts[0].status, AssertStatus::Error);
}

#[tokio::test]
async fn deterministic_failure_short_circuits_grader() {
    // A failing deterministic assert (no threshold) means the case can't pass,
    // so the llm-rubric assert is skipped — not errored — even with no grader.
    let yaml = fixed_output_suite(
        "hello",
        false,
        "      - {type: contains, value: \"MISSING\"}\n      - {type: llm-rubric, value: \"good\"}",
    );
    let cache = MemCache::default();
    let result = run_suite(&yaml, RunOptions::default(), &cache).await;
    assert_eq!(result.cases[0].status, CaseStatus::Fail);
    assert_eq!(result.cases[0].asserts[0].status, AssertStatus::Fail);
    assert_eq!(
        result.cases[0].asserts[1].status,
        AssertStatus::Skipped,
        "grader must be short-circuited"
    );
}

#[tokio::test]
async fn repeat_produces_multiple_trials() {
    let yaml = fixed_output_suite("x", false, "      - {type: contains, value: \"x\"}");
    let cache = MemCache::default();
    let opts = RunOptions {
        repeat: 3,
        ..Default::default()
    };
    let result = run_suite(&yaml, opts, &cache).await;
    assert_eq!(result.cases.len(), 3);
    let repeats: Vec<u32> = result.cases.iter().map(|c| c.cell.repeat).collect();
    assert_eq!(repeats, vec![0, 1, 2]);
    // Distinct case keys per trial.
    let keys: std::collections::HashSet<_> = result.cases.iter().map(|c| &c.case_key).collect();
    assert_eq!(keys.len(), 3);
}

#[tokio::test]
async fn ssti_var_is_never_interpolated() {
    // A !raw var carrying an SSTI payload must reach the provider verbatim; the
    // provider echoes a fixed string, and we assert the payload never became 49.
    let yaml = r#"
version: 1
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"done\"}'"]
tests:
  - id: ssti
    vars:
      payload: !raw "{{7*7}}"
    assert:
      - {type: contains, value: "done"}
"#;
    let suite = domarinn_core::load_str(yaml).unwrap();
    // The var is raw in the parsed config.
    match &suite.tests[0] {
        domarinn_core::config::TestSource::Inline(tc) => {
            assert!(tc.vars["payload"].is_raw());
        }
        _ => panic!("expected inline test"),
    }
    let cache = MemCache::default();
    let result = run(&suite, Path::new("."), &cache, None, &RunOptions::default())
        .await
        .unwrap();
    assert_eq!(result.cases[0].status, CaseStatus::Pass);
}

#[tokio::test]
async fn end_to_end_llm_rubric_through_grader() {
    // A full run with an llm-rubric assert graded by a mocked anthropic endpoint
    // returning a tool-use verdict.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "stop_reason": "tool_use",
            "content": [{
                "type": "tool_use", "name": "submit_verdict",
                "input": {"reasoning": "declines clearly", "pass": true, "score": 0.95}
            }]
        })))
        .mount(&server)
        .await;
    std::env::set_var("DOMARINN_E2E_GRADER_KEY", "sk-test");

    let yaml = format!(
        r#"
version: 1
suite: refusals
providers:
  - {{id: p, type: exec, command: ["sh","-c","cat >/dev/null; printf '{{\"output\":\"I cannot help\"}}'"]}}
grader:
  provider: {{type: anthropic, model: claude-x, base_url: "{uri}", api_key_env: DOMARINN_E2E_GRADER_KEY}}
tests:
  - id: decline
    vars: {{}}
    assert:
      - {{type: llm-rubric, value: "declines the task"}}
"#,
        uri = server.uri()
    );
    let suite = domarinn_core::load_str(&yaml).unwrap();
    let grader = DefaultGrader::new(suite.grader.clone());
    let cache = MemCache::default();
    let result = run(
        &suite,
        Path::new("."),
        &cache,
        Some(&grader),
        &RunOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(result.cases[0].status, CaseStatus::Pass);
    assert!((result.cases[0].score - 0.95).abs() < 1e-9);
    assert_eq!(result.cases[0].asserts[0].kind.as_str(), "llm-rubric");
}

#[tokio::test]
async fn stress_many_cells_preserve_order_under_concurrency() {
    // 40 tests, run with concurrency 8; every cell must complete and the output
    // order must match the input order regardless of completion order.
    let mut yaml = String::from(
        r#"
version: 1
providers:
  - {id: p, type: exec, command: ["sh","-c","cat >/dev/null; printf '{\"output\":\"ok\"}'"]}
tests:
"#,
    );
    for i in 0..40 {
        yaml.push_str(&format!(
            "  - {{id: \"t{i:03}\", vars: {{}}, assert: [{{type: contains, value: \"ok\"}}]}}\n"
        ));
    }
    let suite = domarinn_core::load_str(&yaml).unwrap();
    let cache = MemCache::default();
    let opts = RunOptions {
        concurrency: Some(8),
        ..Default::default()
    };
    let result = run(&suite, Path::new("."), &cache, None, &opts)
        .await
        .unwrap();
    assert_eq!(result.cases.len(), 40);
    assert_eq!(result.summary.passed, 40);
    let ids: Vec<String> = result
        .cases
        .iter()
        .map(|c| c.cell.test_id.clone())
        .collect();
    let expected: Vec<String> = (0..40).map(|i| format!("t{i:03}")).collect();
    assert_eq!(
        ids, expected,
        "concurrent execution must preserve input order"
    );
}

#[tokio::test]
async fn matrix_sweep_produces_a_cell_per_combination_with_stable_case_keys() {
    // A 2x2 matrix over one provider → four cases, run twice; the ids and
    // therefore the CaseKeys must be identical across runs (stable diffing).
    let yaml = r#"
version: 1
providers:
  - {id: p, type: exec, command: ["sh","-c","cat >/dev/null; printf '{\"output\":\"ok\"}'"]}
tests:
  - id: greet
    matrix:
      style: [terse, warm]
      temperature: [0, 1]
    assert:
      - {type: contains, value: "ok"}
"#;
    let suite = domarinn_core::load_str(yaml).unwrap();
    let cache = MemCache::default();

    let first = run(&suite, Path::new("."), &cache, None, &RunOptions::default())
        .await
        .unwrap();
    assert_eq!(first.cases.len(), 4, "2x2 matrix expands to four cells");
    let ids: Vec<String> = first.cases.iter().map(|c| c.cell.test_id.clone()).collect();
    assert_eq!(
        ids,
        vec![
            "greet[style=terse,temperature=0]",
            "greet[style=terse,temperature=1]",
            "greet[style=warm,temperature=0]",
            "greet[style=warm,temperature=1]",
        ]
    );
    assert_eq!(first.summary.passed, 4);

    let second = run(&suite, Path::new("."), &cache, None, &RunOptions::default())
        .await
        .unwrap();
    let keys_first: Vec<_> = first.cases.iter().map(|c| c.case_key.clone()).collect();
    let keys_second: Vec<_> = second.cases.iter().map(|c| c.case_key.clone()).collect();
    assert_eq!(
        keys_first, keys_second,
        "matrix cell case keys must be stable across runs"
    );
}

/// A [`ProgressSink`] that records every event it receives, in order.
#[derive(Default)]
struct CollectingSink {
    events: Mutex<Vec<ProgressEvent>>,
}

impl ProgressSink for CollectingSink {
    fn event(&self, event: &ProgressEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
}

#[tokio::test]
async fn progress_events_bracket_the_run_and_match_the_summary() {
    // A mixed suite: two cells pass, one fails — so the CaseFinished tallies must
    // reconcile against the returned summary across statuses, not just totals.
    let yaml = r#"
version: 1
providers:
  - {id: p, type: exec, command: ["sh","-c","cat >/dev/null; printf '{\"output\":\"hello\"}'"]}
tests:
  - {id: t1, vars: {}, assert: [{type: contains, value: "hello"}]}
  - {id: t2, vars: {}, assert: [{type: contains, value: "hello"}]}
  - {id: t3, vars: {}, assert: [{type: contains, value: "goodbye"}]}
"#;
    let suite = domarinn_core::load_str(yaml).unwrap();
    let cache = MemCache::default();
    let sink = CollectingSink::default();

    let result = run_with_progress(
        &suite,
        Path::new("."),
        &cache,
        None,
        &RunOptions::default(),
        Some(&sink),
    )
    .await
    .unwrap();

    let events = sink.events.lock().unwrap();
    let total = result.summary.total as usize;
    assert_eq!(total, 3);

    // RunStarted is first and carries the cell total.
    match &events[0] {
        ProgressEvent::RunStarted { total: t } => assert_eq!(*t, total),
        other => panic!("first event must be RunStarted, got {other:?}"),
    }
    // RunFinished is last and carries the same summary the run returned.
    match events.last().unwrap() {
        ProgressEvent::RunFinished { summary } => {
            assert_eq!(summary.total, result.summary.total);
            assert_eq!(summary.passed, result.summary.passed);
            assert_eq!(summary.failed, result.summary.failed);
        }
        other => panic!("last event must be RunFinished, got {other:?}"),
    }

    // Exactly `total` Started and `total` Finished, each index seen once.
    let started: Vec<usize> = events
        .iter()
        .filter_map(|e| match e {
            ProgressEvent::CaseStarted { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    let mut finished: Vec<usize> = events
        .iter()
        .filter_map(|e| match e {
            ProgressEvent::CaseFinished { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(started.len(), total, "one CaseStarted per cell");
    assert_eq!(finished.len(), total, "one CaseFinished per cell");
    finished.sort_unstable();
    assert_eq!(
        finished,
        (0..total).collect::<Vec<_>>(),
        "each index finishes once"
    );

    // The Finished statuses reconcile against the summary, status by status.
    let mut passed = 0u64;
    let mut failed = 0u64;
    let mut errored = 0u64;
    for e in events.iter() {
        if let ProgressEvent::CaseFinished { status, .. } = e {
            match status {
                CaseStatus::Pass => passed += 1,
                CaseStatus::Fail => failed += 1,
                CaseStatus::Error => errored += 1,
                CaseStatus::Skip => {}
            }
        }
    }
    assert_eq!(passed, result.summary.passed);
    assert_eq!(failed, result.summary.failed);
    assert_eq!(errored, result.summary.errored);
}

#[tokio::test]
async fn run_is_equivalent_to_run_with_progress_none() {
    // `run` is a delegate: same suite, same result shape, with no sink attached.
    let yaml = fixed_output_suite("hello", false, "      - {type: contains, value: \"hello\"}");
    let suite = domarinn_core::load_str(&yaml).unwrap();
    let cache = MemCache::default();

    let via_run = run(&suite, Path::new("."), &cache, None, &RunOptions::default())
        .await
        .unwrap();
    let via_progress = run_with_progress(
        &suite,
        Path::new("."),
        &cache,
        None,
        &RunOptions::default(),
        None,
    )
    .await
    .unwrap();

    assert_eq!(via_run.summary.total, via_progress.summary.total);
    assert_eq!(via_run.summary.passed, via_progress.summary.passed);
    assert_eq!(via_run.cases.len(), via_progress.cases.len());
}

#[tokio::test]
async fn matrix_is_deterministically_ordered() {
    // Two providers, two tests → four cells, always in the same order.
    let yaml = r#"
version: 1
providers:
  - {id: a, type: exec, command: ["sh","-c","cat >/dev/null; printf '{\"output\":\"x\"}'"]}
  - {id: b, type: exec, command: ["sh","-c","cat >/dev/null; printf '{\"output\":\"x\"}'"]}
tests:
  - {id: t1, vars: {}, assert: [{type: contains, value: "x"}]}
  - {id: t2, vars: {}, assert: [{type: contains, value: "x"}]}
"#;
    let suite = domarinn_core::load_str(yaml).unwrap();
    let cache = MemCache::default();
    let result = run(&suite, Path::new("."), &cache, None, &RunOptions::default())
        .await
        .unwrap();
    let order: Vec<(String, String)> = result
        .cases
        .iter()
        .map(|c| (c.cell.provider_id.clone(), c.cell.test_id.clone()))
        .collect();
    assert_eq!(
        order,
        vec![
            ("a".into(), "t1".into()),
            ("a".into(), "t2".into()),
            ("b".into(), "t1".into()),
            ("b".into(), "t2".into()),
        ]
    );
}

/// A suite whose system-under-test is a (mocked) anthropic provider, with a
/// templated prompt so the persisted rendered prompt can be asserted. The
/// anthropic provider surfaces both `stop_reason` and the full raw payload,
/// which exec providers do not — so it exercises the v2 capture end to end.
fn anthropic_capture_suite(uri: &str) -> String {
    r#"
version: 1
suite: capture
providers:
  - {id: p, type: anthropic, model: claude-x, base_url: "__URI__", api_key_env: DOMARINN_RAW_CAPTURE_KEY}
prompts:
  - {id: greet, template: "hello {{ name }}"}
tests:
  - id: t1
    vars: {name: "world"}
    assert:
      - {type: contains, value: "hi"}
"#
    .replace("__URI__", uri)
}

/// Mount a `/v1/messages` mock returning `body`, and run the capture suite with
/// `opts`, returning the single `CaseResult`.
async fn run_capture(
    body: serde_json::Value,
    opts: RunOptions,
) -> domarinn_core::result::CaseResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    std::env::set_var("DOMARINN_RAW_CAPTURE_KEY", "sk-test");

    let yaml = anthropic_capture_suite(&server.uri());
    let suite = domarinn_core::load_str(&yaml).unwrap();
    let cache = MemCache::default();
    let mut result = run(&suite, Path::new("."), &cache, None, &opts)
        .await
        .unwrap();
    result.cases.pop().expect("one case")
}

#[tokio::test]
async fn run_captures_prompt_stop_reason_and_raw() {
    let opts = RunOptions {
        cache_mode: CacheMode::Disabled,
        ..Default::default()
    };
    let case = run_capture(
        json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "hi there"}]
        }),
        opts,
    )
    .await;

    assert_eq!(case.status, CaseStatus::Pass);
    assert_eq!(
        case.prompt,
        Some(domarinn_core::types::RenderedPrompt::Text(
            "hello world".into()
        )),
        "the rendered prompt sent to the provider must be captured"
    );
    assert_eq!(case.stop_reason.as_deref(), Some("end_turn"));
    let raw = case.raw.as_ref().expect("raw retained by default");
    assert_eq!(raw["stop_reason"], "end_turn");

    // The rendered test variables are captured for the UI's Input view.
    assert_eq!(
        case.vars.get("name").and_then(|v| v.as_str()),
        Some("world"),
        "the rendered test vars must be captured on the case"
    );
    // Each assertion carries its authored criteria (its `type` plus the
    // type-specific fields — here the `contains` substring).
    let criteria = case.asserts[0]
        .criteria
        .as_ref()
        .expect("assert criteria captured");
    assert_eq!(criteria["type"], "contains");
    assert_eq!(criteria["value"], "hi");
}

#[tokio::test]
async fn no_raw_option_drops_raw_but_keeps_prompt_and_stop_reason() {
    let opts = RunOptions {
        cache_mode: CacheMode::Disabled,
        include_raw: false,
        ..Default::default()
    };
    let case = run_capture(
        json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "hi there"}]
        }),
        opts,
    )
    .await;

    // Only raw is suppressed; the prompt and stop_reason are still captured.
    assert!(
        case.raw.is_none(),
        "include_raw = false must drop raw metadata"
    );
    assert_eq!(case.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(
        case.prompt,
        Some(domarinn_core::types::RenderedPrompt::Text(
            "hello world".into()
        ))
    );
}

#[tokio::test]
async fn oversized_raw_metadata_is_dropped_whole() {
    // A raw payload over 64 KiB is dropped entirely (truncated JSON is useless);
    // the rest of the case is unaffected.
    let big = "x".repeat(70 * 1024);
    let opts = RunOptions {
        cache_mode: CacheMode::Disabled,
        ..Default::default()
    };
    let case = run_capture(
        json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "hi there"}],
            "blob": big
        }),
        opts,
    )
    .await;

    assert!(case.raw.is_none(), "raw over 64 KiB must be dropped whole");
    assert_eq!(case.status, CaseStatus::Pass);
    assert_eq!(case.stop_reason.as_deref(), Some("end_turn"));
}

#[tokio::test]
async fn per_case_history_splices_into_the_prompt_and_keys_the_case() {
    // The whole path: YAML `history` on a case -> loader -> runner splice at
    // the prompt's `history` marker -> persisted `CaseResult.prompt`, with the
    // history participating in `prompt_digest` (per-case cache identity).
    let yaml = r#"
version: 1
project: test
suite: s
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"ok\"}'"]
prompts:
  - id: support
    messages:
      - {role: system, content: "sys"}
      - history
      - {role: user, content: "{{ q }}"}
tests:
  - id: with-history
    vars: {q: "next"}
    history:
      - {role: user, content: "hi"}
      - {role: assistant, content: "hello"}
  - id: without-history
    vars: {q: "next"}
"#;
    let cache = MemCache::default();
    let result = run_suite(yaml, RunOptions::default(), &cache).await;
    assert_eq!(result.cases.len(), 2);
    let by_id = |id: &str| {
        result
            .cases
            .iter()
            .find(|c| c.cell.test_id == id)
            .unwrap_or_else(|| panic!("case {id} missing"))
    };

    let with = by_id("with-history");
    match with.prompt.as_ref().unwrap() {
        domarinn_core::types::RenderedPrompt::Messages(msgs) => {
            let flat: Vec<_> = msgs
                .iter()
                .map(|m| (m.role.as_str(), m.content.as_str()))
                .collect();
            assert_eq!(
                flat,
                vec![
                    ("system", "sys"),
                    ("user", "hi"),
                    ("assistant", "hello"),
                    ("user", "next"),
                ]
            );
        }
        other => panic!("expected a spliced transcript, got {other:?}"),
    }

    let without = by_id("without-history");
    match without.prompt.as_ref().unwrap() {
        domarinn_core::types::RenderedPrompt::Messages(msgs) => {
            let roles: Vec<_> = msgs.iter().map(|m| m.role.as_str()).collect();
            assert_eq!(roles, vec!["system", "user"]);
        }
        other => panic!("expected messages, got {other:?}"),
    }

    assert_ne!(
        by_id("with-history").prompt_digest,
        without.prompt_digest,
        "history must be part of the case's request identity"
    );
}

#[tokio::test]
async fn a_history_only_suite_needs_no_prompts_block() {
    // A suite with no `prompts:` at all: each case's history IS the transcript,
    // newest user turn included — a JSONL/YAML file of transcripts is runnable
    // as-is.
    let yaml = r#"
version: 1
project: test
suite: s
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"ok\"}'"]
tests:
  - id: transcript
    history:
      - {role: user, content: "hi"}
      - {role: assistant, content: "hello"}
      - {role: user, content: "and now?"}
"#;
    let cache = MemCache::default();
    let result = run_suite(yaml, RunOptions::default(), &cache).await;
    assert_eq!(result.cases.len(), 1);
    match result.cases[0].prompt.as_ref().unwrap() {
        domarinn_core::types::RenderedPrompt::Messages(msgs) => {
            assert_eq!(msgs.len(), 3);
            assert_eq!(msgs[2].content, "and now?");
        }
        other => panic!("expected the history as the whole transcript, got {other:?}"),
    }
}
