//! Grader-originated requests are cached under the one rule, and ≤0.4.x
//! verdicts are adopted forward.
//!
//! Sibling of `grader_cache_identity.rs`, which covers what must *not* share an
//! entry. This file covers the mechanism: what is written, what a warm run
//! replays without calling, and what an old store still buys after the key space
//! it was written in stopped existing.
//!
//! `grader_request_cache_tool_calls.rs` is the other half of this file, split
//! off at the 1000-line source cap; the fixtures both use live in
//! `grader_request_cache/shared.rs`.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use domarinn_core::cache::{CacheKey, CacheMode, GradedVerdict};
use domarinn_core::grader::SYSTEM_PROMPT;
use domarinn_core::result::CaseStatus;
use domarinn_core::runner::RunOptions;
use domarinn_core::types::Output;
use serde_json::{json, Value as Json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[path = "grader_request_cache/shared.rs"]
mod shared;
use shared::*;

// ── What a judge entry is, and what a warm one replays ───────────────────────

/// An entry whose payload today's parser rejects is a miss, and the run
/// recovers by asking the judge.
///
/// Entries are immutable, so the alternative — erroring — could never be
/// written past: one incompatible entry would fail the same assertion on every
/// future run, with no remedy but purging a store the message never named. This
/// is not a silent pass; the judge is genuinely re-asked and answers.
#[tokio::test]
async fn an_unparseable_stored_payload_is_re_asked_rather_than_replayed() {
    let server = always_passes().await;
    set_env("DOMARINN_REQCACHE_KEY", "sk-test").await;
    let yaml = rubric_suite(&server.uri(), None);
    let cache = MemCache::default();

    run_suite(&yaml, Path::new("."), &cache, &RunOptions::default()).await;
    assert_eq!(judge_calls(&server).await, 1);

    // Corrupt the stored payload in place, keeping the key.
    let (key, mut entry) = cache
        .find(|e| e.raw.is_some() && e.verdict.is_none())
        .expect("the judge exchange is stored");
    entry.raw = Some(json!({"content": [{"type": "text", "text": "not a verdict"}]}));
    cache.overwrite(&key, entry);

    let recovered = run_suite(&yaml, Path::new("."), &cache, &RunOptions::default()).await;
    assert_eq!(
        judge_calls(&server).await,
        2,
        "an unreadable payload must send the run back to the judge"
    );
    assert_eq!(recovered.cases[0].status, CaseStatus::Pass);
    assert!(!recovered.cases[0].asserts[0].cached);
}

/// The entry records the judge's url and body, and a warm run re-derives the
/// verdict from the stored payload rather than calling.
///
/// Both halves matter. The `request` member is the offline-migration and
/// debugging contract — an entry that cannot say what it answered is a hash with
/// a value attached. The re-parse is what makes the verdict *derived* rather
/// than stored: fix the parser, and everything already cached is fixed with it.
#[tokio::test]
async fn a_judge_entry_stores_the_request_and_a_warm_hit_re_parses_it() {
    let server = always_passes().await;
    set_env("DOMARINN_REQCACHE_KEY", "sk-test").await;
    let yaml = rubric_suite(&server.uri(), None);
    let cache = MemCache::default();

    let cold = run_suite(&yaml, Path::new("."), &cache, &RunOptions::default()).await;
    assert_eq!(cold.cases[0].status, CaseStatus::Pass);
    assert!(
        !cold.cases[0].asserts[0].cached,
        "a cold run pays the judge"
    );
    assert_eq!(judge_calls(&server).await, 1);

    let judge_entry = cache
        .entries()
        .into_iter()
        .find(|e| e.verdict.is_none() && e.raw.is_some())
        .expect("the judge exchange is stored as a request/response entry");
    let request = judge_entry.request.expect("the entry records its request");
    assert_eq!(request["transport"], json!("http"));
    assert_eq!(request["method"], json!("POST"));
    assert_eq!(
        request["url"],
        json!(format!("{}/v1/messages", server.uri()))
    );
    assert_eq!(request["body"]["model"], json!("claude-x"));
    assert_eq!(request["body"]["system"], json!(SYSTEM_PROMPT));
    // The rendered rubric and the graded output travel in the user message, so
    // the key covers both without a second hashed document.
    let user = request["body"]["messages"][0]["content"].as_str().unwrap();
    assert!(user.contains("declines the task"), "{user}");
    assert!(user.contains("I cannot help"), "{user}");
    // No credential anywhere in it: the api key is a header.
    assert!(
        !serde_json::to_string(&request).unwrap().contains("sk-test"),
        "the stored request must never carry the key"
    );
    // The payload the verdict is re-derived from, and the human-readable view.
    assert_eq!(judge_entry.raw.unwrap()["stop_reason"], json!("tool_use"));
    assert_eq!(judge_entry.output, Output::Text("live verdict".into()));
    assert_eq!(judge_entry.model.as_deref(), Some("claude-x-20260101"));

    let warm = run_suite(&yaml, Path::new("."), &cache, &RunOptions::default()).await;
    assert_eq!(
        judge_calls(&server).await,
        1,
        "the warm run must not call the judge"
    );
    assert!(warm.cases[0].asserts[0].cached, "and must say it replayed");
    assert_eq!(warm.cases[0].asserts[0].reason, "live verdict");
}

/// Editing the `grader.template` file busts the entry.
///
/// This is the whole replacement for the deleted `template_digest`. That field
/// existed because the key hashed a fingerprint which could not see the prompt;
/// the file's contents are now *in the request body*, so the guarantee is a
/// consequence of the key rather than a member of it that has to be maintained.
#[tokio::test]
async fn editing_the_grader_template_busts_the_judge_entry() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("judge.md"),
        "Be lenient.\n{{rubric}}\n{{output}}",
    )
    .unwrap();
    let server = always_passes().await;
    set_env("DOMARINN_REQCACHE_KEY", "sk-test").await;
    let yaml = rubric_suite(&server.uri(), Some("file://judge.md"));
    let cache = MemCache::default();

    run_suite(&yaml, dir.path(), &cache, &RunOptions::default()).await;
    assert_eq!(judge_calls(&server).await, 1);
    run_suite(&yaml, dir.path(), &cache, &RunOptions::default()).await;
    assert_eq!(
        judge_calls(&server).await,
        1,
        "sanity: an untouched template replays"
    );

    std::fs::write(
        dir.path().join("judge.md"),
        "Be strict.\n{{rubric}}\n{{output}}",
    )
    .unwrap();
    let edited = run_suite(&yaml, dir.path(), &cache, &RunOptions::default()).await;
    assert_eq!(
        judge_calls(&server).await,
        2,
        "a different grading prompt is a different question"
    );
    assert!(!edited.cases[0].asserts[0].cached);
}

// ── exec asserts ─────────────────────────────────────────────────────────────

/// An `exec` assert's protocol exchange is a request like any other: the child
/// is spawned once and every later run replays its answer.
///
/// Before 0.5.0 this worked through a hand-built verdict key; now it is the
/// same `request_cache_key` a provider call uses, over the document written to
/// the child's stdin.
#[tokio::test]
async fn an_exec_assert_replays_warm_and_a_cache_salt_busts_it() {
    let dir = tempfile::tempdir().unwrap();
    let (judge, counter) = counting_judge(dir.path());
    let cache = MemCache::default();
    let yaml = exec_assert_suite(&judge, None);

    let cold = run_suite(&yaml, dir.path(), &cache, &RunOptions::default()).await;
    assert_eq!(cold.cases[0].status, CaseStatus::Pass);
    assert_eq!(calls(&counter), 1, "the child is asked once");
    assert!(!cold.cases[0].asserts[0].cached);

    let warm = run_suite(&yaml, dir.path(), &cache, &RunOptions::default()).await;
    assert_eq!(
        calls(&counter),
        1,
        "the second run must not spawn the child"
    );
    assert!(warm.cases[0].asserts[0].cached);
    assert_eq!(warm.cases[0].asserts[0].reason, "child says ok");

    // The per-assert salt is how a suite says the child is a different version.
    let salted = run_suite(
        &exec_assert_suite(&judge, Some("v2")),
        dir.path(),
        &cache,
        &RunOptions::default(),
    )
    .await;
    assert_eq!(calls(&counter), 2, "a new cache_salt re-asks the child");
    assert!(!salted.cases[0].asserts[0].cached);
}

/// The environment is not in an exec assert's key, and is not written into the
/// entry.
///
/// An assert's `vars` is the *render context*, which carries a snapshot of the
/// whole process environment so `{{ env.X }}` resolves in sibling assertions.
/// Keying it would make every entry a property of one machine; storing it would
/// put that machine's secrets into a shared cache. The child still receives it.
#[tokio::test]
async fn an_exec_assert_entry_carries_neither_the_environment_nor_the_test_id() {
    let dir = tempfile::tempdir().unwrap();
    let (judge, counter) = counting_judge(dir.path());
    let cache = MemCache::default();
    set_env("DOMARINN_REQCACHE_SECRET", "hunter2").await;
    let yaml = exec_assert_suite(&judge, None);

    run_suite(&yaml, dir.path(), &cache, &RunOptions::default()).await;
    let entry = cache
        .entries()
        .into_iter()
        .find(|e| e.raw.is_some() && e.request.as_ref().is_some_and(|r| r["transport"] == "exec"))
        .expect("the exec exchange is stored");
    let request = entry.request.unwrap();
    let stored = serde_json::to_string(&request).unwrap();
    assert!(
        !stored.contains("hunter2"),
        "a secret reached the store: {stored}"
    );
    assert!(
        !stored.contains("\"test\""),
        "the test id is not keyed: {stored}"
    );
    assert_eq!(request["stdin"]["vars"]["expected"], json!("Paris"));
    assert!(request["stdin"]["vars"].get("env").is_none());
    // `provider` SURVIVES, unlike `test`. It is suite-authored rather than
    // per-case correlation metadata, and a child is entitled to branch on which
    // system under test produced the output — so two providers' answers are two
    // questions and must not share an entry.
    assert_eq!(request["stdin"]["provider"]["id"], json!("p"));
    // The protocol envelope is deliberately kept, so a bump re-keys everything.
    assert_eq!(request["stdin"]["domarinn"]["protocol"], json!(1));

    // Setting a different env var must not bust the entry.
    set_env("DOMARINN_REQCACHE_SECRET", "hunter3").await;
    run_suite(&yaml, dir.path(), &cache, &RunOptions::default()).await;
    assert_eq!(calls(&counter), 1, "the environment is not part of the key");
}

// ── Adoption ─────────────────────────────────────────────────────────────────

/// A verdict a 0.4.x run paid for is served, re-filed under the request key, and
/// never probed for again.
///
/// The expensive half of a graded suite is the judge, so a key change that
/// stranded every verdict in every store would cost real money per upgrade.
/// The adopted entry keeps its shape — `verdict` stays `Some`, `raw` stays
/// `None`, because there is no payload to invent — and the read contract keeps
/// it servable from then on.
#[tokio::test]
async fn a_legacy_verdict_is_adopted_and_then_found_directly() {
    let server = always_passes().await;
    set_env("DOMARINN_REQCACHE_KEY", "sk-test").await;
    let yaml = rubric_suite(&server.uri(), None);
    let suite = domarinn_core::load_str(&yaml).unwrap();
    let legacy = legacy_rubric_key(
        &Output::Text("I cannot help".into()),
        suite.grader.as_ref().unwrap(),
    );

    let cache = MemCache::default();
    cache.seed(&legacy, verdict_entry("verdict from 0.4"));

    let first = run_suite(&yaml, Path::new("."), &cache, &RunOptions::default()).await;
    assert_eq!(
        judge_calls(&server).await,
        0,
        "an adopted verdict costs nothing"
    );
    assert_eq!(first.cases[0].status, CaseStatus::Pass);
    assert!(first.cases[0].asserts[0].cached);
    assert_eq!(first.cases[0].asserts[0].reason, "verdict from 0.4");
    // The cost replays with the verdict rather than collapsing to nothing.
    assert_eq!(first.cases[0].asserts[0].cost_usd, Some(0.25));

    // Re-filed under the request key, carrying the request it answers — and
    // still a verdict entry, because that is all it ever was.
    let refiled = cache
        .entries()
        .into_iter()
        .find(|e| e.verdict.is_some() && e.request.is_some())
        .expect("the adopted entry is re-filed with its request");
    assert!(refiled.raw.is_none(), "there was no payload to invent");
    assert_eq!(refiled.request.unwrap()["body"]["model"], json!("claude-x"));

    cache.forget_gets();
    let second = run_suite(&yaml, Path::new("."), &cache, &RunOptions::default()).await;
    assert_eq!(judge_calls(&server).await, 0);
    assert!(second.cases[0].asserts[0].cached);
    assert_eq!(
        cache.asked_for(&legacy),
        0,
        "once re-filed, the legacy key is never probed again"
    );
}

/// `--no-cache-migration` turns the probe off, for both key spaces at once.
#[tokio::test]
async fn no_cache_migration_leaves_a_legacy_verdict_unadopted() {
    let server = always_passes().await;
    set_env("DOMARINN_REQCACHE_KEY", "sk-test").await;
    let yaml = rubric_suite(&server.uri(), None);
    let suite = domarinn_core::load_str(&yaml).unwrap();
    let legacy = legacy_rubric_key(
        &Output::Text("I cannot help".into()),
        suite.grader.as_ref().unwrap(),
    );

    let cache = MemCache::default();
    cache.seed(&legacy, verdict_entry("verdict from 0.4"));

    let result = run_suite(
        &yaml,
        Path::new("."),
        &cache,
        &RunOptions {
            cache_migration: false,
            ..RunOptions::default()
        },
    )
    .await;
    assert_eq!(judge_calls(&server).await, 1, "the judge is re-paid");
    assert_eq!(result.cases[0].asserts[0].reason, "live verdict");
    assert_eq!(cache.asked_for(&legacy), 0, "and the old key is not read");
}

/// The same, for an `exec` assert — the shape the wiring is most likely to get
/// wrong and the one no golden would catch.
///
/// The goldens pin the frozen *functions*; the rubric test above pins the
/// *wiring* for one call type. This pins the wiring for the other, where the
/// bridge has to hand across six payload members including the env-bearing
/// render context. Passing the rendered vars instead of the render context, or
/// the wrong `base_dir`, or dropping `provider`, would strand every `exec`
/// verdict in every 0.4 store — and every other test in this file would still
/// pass.
#[tokio::test]
async fn a_legacy_exec_verdict_is_adopted_and_then_found_directly() {
    // Held across derivation *and* both runs: the key covers the environment.
    let _env = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (judge, counter) = counting_judge(dir.path());
    let yaml = exec_assert_suite(&judge, None);
    let legacy = legacy_exec_key(&judge, dir.path(), &Output::Text("same".into()));

    let cache = MemCache::default();
    cache.seed(&legacy, exec_verdict_entry("child said so in 0.4"));

    let first = run_suite(&yaml, dir.path(), &cache, &RunOptions::default()).await;
    assert_eq!(
        calls(&counter),
        0,
        "an adopted verdict must not spawn the child"
    );
    assert_eq!(first.cases[0].status, CaseStatus::Pass);
    assert!(first.cases[0].asserts[0].cached);
    assert_eq!(first.cases[0].asserts[0].reason, "child said so in 0.4");

    // Re-filed under the request key, carrying the stdin document it answers,
    // and still verdict-only because there was no payload to invent.
    let refiled = cache
        .entries()
        .into_iter()
        .find(|e| e.verdict.is_some() && e.request.is_some())
        .expect("the adopted entry is re-filed with its request");
    assert!(refiled.raw.is_none());
    let request = refiled.request.unwrap();
    assert_eq!(request["transport"], json!("exec"));
    assert_eq!(request["stdin"]["vars"]["expected"], json!("Paris"));
    // The re-filed request is the *portable* one even though the key it came
    // from was not: no environment, no test id.
    assert!(request["stdin"]["vars"].get("env").is_none());
    assert!(request["stdin"].get("test").is_none());

    cache.forget_gets();
    let second = run_suite(&yaml, dir.path(), &cache, &RunOptions::default()).await;
    assert_eq!(calls(&counter), 0);
    assert!(second.cases[0].asserts[0].cached);
    assert_eq!(
        cache.asked_for(&legacy),
        0,
        "once re-filed, the legacy key is never probed again"
    );
}

/// …and `--no-cache-migration` leaves it alone, so the exec probe answers to the
/// same switch as the provider and rubric ones.
#[tokio::test]
async fn no_cache_migration_leaves_a_legacy_exec_verdict_unadopted() {
    let _env = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (judge, counter) = counting_judge(dir.path());
    let yaml = exec_assert_suite(&judge, None);
    let legacy = legacy_exec_key(&judge, dir.path(), &Output::Text("same".into()));

    let cache = MemCache::default();
    cache.seed(&legacy, exec_verdict_entry("child said so in 0.4"));

    let result = run_suite(
        &yaml,
        dir.path(),
        &cache,
        &RunOptions {
            cache_migration: false,
            ..RunOptions::default()
        },
    )
    .await;
    assert_eq!(calls(&counter), 1, "the child is re-asked");
    assert_eq!(result.cases[0].asserts[0].reason, "child says ok");
    assert_eq!(cache.asked_for(&legacy), 0, "and the old key is not read");
}

// ── similar ──────────────────────────────────────────────────────────────────

/// An embeddings endpoint that counts what it was asked to embed.
async fn counting_embedder(seen: Arc<AtomicUsize>) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(move |req: &wiremock::Request| {
            seen.fetch_add(1, Ordering::Relaxed);
            let body: Json = serde_json::from_slice(&req.body).unwrap();
            // A deterministic pseudo-vector, so a replayed one is comparable to
            // a fresh one and the cosine is stable across runs.
            let text = body["input"].as_str().unwrap_or_default();
            let v = [
                text.len() as f64,
                text.bytes().map(|b| b as f64).sum::<f64>() / 100.0,
                1.0,
            ];
            ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"embedding": v}],
                "usage": {"prompt_tokens": 3, "total_tokens": 3},
            }))
        })
        .mount(&server)
        .await;
    server
}

/// A `similar` assertion caches its two embedding requests — new capability —
/// and deliberately adopts nothing.
///
/// A ≤0.4.x entry held one cosine, which decomposes into neither vector. Rather
/// than invent a shape to hold an answer no lookup can ask for, the first run
/// pays the embedder (fractions of a cent) and every run after is warm. The
/// seeded old entry is there to prove it is *ignored*, not merely unused.
#[tokio::test]
async fn similar_caches_its_embeddings_and_adopts_no_cosine() {
    let calls = Arc::new(AtomicUsize::new(0));
    let server = counting_embedder(calls.clone()).await;
    set_env("DOMARINN_REQCACHE_EMBED_KEY", "sk-test").await;
    let yaml = format!(
        r#"
version: 1
suite: similar-cache
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{{\"output\":\"a polite refusal\"}}'"]
    cache_salt: "v1"
  - id: e
    type: embeddings
    model: text-embedding-3-small
    base_url: "{uri}"
    api_key_env: DOMARINN_REQCACHE_EMBED_KEY
tests:
  - id: t
    vars: {{}}
    assert: [{{type: similar, value: "declined, politely", threshold: 0.5}}]
"#,
        uri = server.uri()
    );

    let cache = MemCache::default();
    // A cosine verdict from the era that cached only the answer. Nothing may
    // read it; it is seeded so "not adopted" is observed rather than assumed.
    cache.seed(
        &CacheKey("sha256:0000000000000000000000000000000000000000000000000000000000000000".into()),
        serde_json::from_value(json!({
            "created_at": "2026-01-01T00:00:00Z",
            "output": "cosine similarity 0.999",
            "verdict": {"kind": "similarity", "cosine": 0.999},
            "domarinn_version": "0.4.0",
        }))
        .unwrap(),
    );

    let cold = run_suite(&yaml, Path::new("."), &cache, &RunOptions::default()).await;
    assert_eq!(cold.cases[0].status, CaseStatus::Pass);
    assert_eq!(
        calls.load(Ordering::Relaxed),
        2,
        "one embedding per text: the output and the reference"
    );
    assert!(!cold.cases[0].asserts[0].cached);

    let warm = run_suite(&yaml, Path::new("."), &cache, &RunOptions::default()).await;
    assert_eq!(
        calls.load(Ordering::Relaxed),
        2,
        "the second run must embed nothing"
    );
    assert!(
        warm.cases[0].asserts[0].cached,
        "both halves replayed, so the assertion says so"
    );
    assert_eq!(warm.cases[0].status, CaseStatus::Pass);

    // The stored entry is the embeddings exchange, not a cosine.
    let embed = cache
        .entries()
        .into_iter()
        .find(|e| {
            e.request.as_ref().is_some_and(|r| {
                r["url"]
                    .as_str()
                    .is_some_and(|u| u.ends_with("/embeddings"))
            })
        })
        .expect("an embeddings request is stored");
    assert_eq!(embed.output, Output::Json(json!({"dims": 3})));
    assert!(embed.verdict.is_none(), "a vector is not a verdict");
}

// ── The levers ───────────────────────────────────────────────────────────────

/// `--no-grader-cache` bypasses the grader's cache entirely — no read, no
/// write — while the provider response beside it still replays.
///
/// The two are one store and one key space now, so "the grader half is off"
/// has to be a property of the grader path rather than of the cache.
#[tokio::test]
async fn no_grader_cache_bypasses_warm_judge_entries_but_not_provider_ones() {
    let server = always_passes().await;
    set_env("DOMARINN_REQCACHE_KEY", "sk-test").await;
    let yaml = rubric_suite(&server.uri(), None);
    let cache = MemCache::default();

    run_suite(&yaml, Path::new("."), &cache, &RunOptions::default()).await;
    assert_eq!(judge_calls(&server).await, 1);

    let bypassed = run_suite(
        &yaml,
        Path::new("."),
        &cache,
        &RunOptions {
            grader_cache: false,
            ..RunOptions::default()
        },
    )
    .await;
    assert_eq!(
        judge_calls(&server).await,
        2,
        "the bypass is real: a warm entry is not read"
    );
    assert!(!bypassed.cases[0].asserts[0].cached);
    assert_eq!(
        bypassed.summary.cache_hits, 1,
        "…and the provider response beside it still replays"
    );
}

/// Warm the *provider* entry and nothing else, so a later `--cache-only` run
/// gets as far as the grading before it has anything to complain about.
///
/// `--no-grader-cache` is what makes the split possible: the run pays the judge
/// live and writes no entry for it.
async fn warm_only_the_provider(yaml: &str, cache: &MemCache) {
    run_suite(
        yaml,
        Path::new("."),
        cache,
        &RunOptions {
            grader_cache: false,
            ..RunOptions::default()
        },
    )
    .await;
}

/// `--cache-only` with nothing to replay is a hard grader error, not a live
/// call. The grader half of an offline run stays offline.
#[tokio::test]
async fn cache_only_with_no_judge_entry_errors_rather_than_calling() {
    let server = always_passes().await;
    set_env("DOMARINN_REQCACHE_KEY", "sk-test").await;
    let yaml = rubric_suite(&server.uri(), None);
    let cache = MemCache::default();
    warm_only_the_provider(&yaml, &cache).await;
    let before = judge_calls(&server).await;

    let result = run_suite(
        &yaml,
        Path::new("."),
        &cache,
        &RunOptions {
            cache_mode: CacheMode::ReadOnlyStrict,
            ..RunOptions::default()
        },
    )
    .await;
    assert_eq!(
        judge_calls(&server).await,
        before,
        "cache-only must not reach a judge"
    );
    let assertion = &result.cases[0].asserts[0];
    assert_eq!(assertion.status, domarinn_core::result::AssertStatus::Error);
    assert!(
        assertion.reason.contains("cache-only"),
        "the error must name the cause: {}",
        assertion.reason
    );
}

/// …and with grader caching *off*, `--cache-only` still refuses, because there
/// is not even a key to miss on. Preserved verbatim from the pre-0.5.0 path:
/// the only change is the wording.
#[tokio::test]
async fn cache_only_with_grader_caching_off_refuses_before_any_lookup() {
    let server = always_passes().await;
    set_env("DOMARINN_REQCACHE_KEY", "sk-test").await;
    let yaml = rubric_suite(&server.uri(), None);
    let cache = MemCache::default();
    warm_only_the_provider(&yaml, &cache).await;
    let before = judge_calls(&server).await;

    let result = run_suite(
        &yaml,
        Path::new("."),
        &cache,
        &RunOptions {
            cache_mode: CacheMode::ReadOnlyStrict,
            grader_cache: false,
            ..RunOptions::default()
        },
    )
    .await;
    assert_eq!(judge_calls(&server).await, before);
    let assertion = &result.cases[0].asserts[0];
    assert_eq!(assertion.status, domarinn_core::result::AssertStatus::Error);
    assert!(
        assertion.reason.contains("grader caching is off"),
        "{}",
        assertion.reason
    );
}

/// A `GradedVerdict` round-trips through the entry it is seeded as, or every
/// adoption test above is asserting on a shape the runner never sees.
#[test]
fn a_seeded_verdict_entry_deserializes_as_one() {
    let entry = verdict_entry("x");
    assert!(matches!(
        entry.verdict,
        Some(GradedVerdict::Rubric { pass: true, .. })
    ));
    assert!(entry.raw.is_none() && entry.request.is_none());
}
