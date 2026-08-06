//! Unit tests for [`super`] (the runner). Split out of `runner.rs` via
//! `#[path]` to keep that file under the repo's 1000-line source cap;
//! this is still the runner's private child module (`use super::*`).

use super::runner_cache::{entry_to_response, response_to_entry};
use super::*;
use crate::cache::{CacheEntry, CacheError, CacheStats, PurgeFilter};
use crate::provider::{ProviderError, ProviderResponse};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// A cache that never hits and never fails — the retry path under test uses
/// `CacheMode::Disabled`, so these are inert, but the signature requires one.
struct NoopCache;

#[async_trait]
impl CacheBackend for NoopCache {
    async fn get(&self, _key: &crate::cache::CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        Ok(None)
    }
    async fn put(
        &self,
        _key: &crate::cache::CacheKey,
        _entry: &CacheEntry,
    ) -> Result<(), CacheError> {
        Ok(())
    }
    async fn stats(&self) -> Result<CacheStats, CacheError> {
        Ok(CacheStats::default())
    }
    async fn purge(&self, _filter: &PurgeFilter) -> Result<u64, CacheError> {
        Ok(0)
    }
}

/// A provider that fails retriably on its first call, then succeeds — enough
/// to fire exactly one retry warning.
struct FlakyProvider {
    calls: AtomicU32,
}

#[async_trait]
impl Provider for FlakyProvider {
    fn id(&self) -> &str {
        "flaky"
    }
    fn fingerprint(&self) -> Json {
        serde_json::json!({ "type": "flaky" })
    }
    async fn call(
        &self,
        _req: &ProviderRequest,
        _ctx: &CallCtx,
    ) -> Result<ProviderResponse, ProviderError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            Err(ProviderError::retriable(
                crate::error_class::ErrorClass::PROVIDER_UNAVAILABLE,
                anyhow::anyhow!("boom"),
                None,
            ))
        } else {
            Ok(ProviderResponse::text("ok"))
        }
    }
}

/// The canonical request a fixture entry is keyed on. Shape-faithful to an
/// `exec` envelope so the entry looks like one a real call would write; nothing
/// in these tests depends on its contents.
fn test_request() -> Json {
    serde_json::json!({"transport": "exec", "command": "./sut", "args": []})
}

/// Stats for fixtures that do not exercise the retry loop itself.
fn test_stats() -> crate::retry::RetryStats {
    crate::retry::RetryStats {
        attempts: 1,
        in_flight: std::time::Duration::from_millis(0),
    }
}

/// Cached replays must carry the same raw provider metadata a fresh call
/// would — the drawer's "Provider metadata" section disappeared for every
/// cached case because `CacheEntry` silently dropped `raw`.
#[test]
fn cache_entries_preserve_raw_provider_metadata() {
    let provider = FlakyProvider {
        calls: AtomicU32::new(0),
    };
    let raw = serde_json::json!({ "id": "resp_1", "model": "m", "finish_reason": "stop" });
    let response = ProviderResponse {
        tool_calls: Vec::new(),
        output: crate::types::Output::Text("hi".to_string()),
        usage: None,
        cost_usd: None,
        stop_reason: Some("stop".to_string()),
        raw: Some(raw.clone()),
        reasoning: None,
        empty_reason: None,
        model: None,
    };

    let entry = response_to_entry(&provider, &response, test_stats(), &test_request());
    assert_eq!(entry.raw, Some(raw.clone()));
    let replayed = entry_to_response(entry, &provider);
    assert_eq!(replayed.raw, Some(raw));
}

/// Every field on [`ProviderResponse`] must survive the cache round-trip.
///
/// The destructuring below is deliberate: adding a field to `ProviderResponse`
/// makes this test **fail to compile**, forcing you to decide whether it needs
/// a `CacheEntry` counterpart. A field without one silently replays as `None`
/// on every cache hit — which is the common path, not the rare one — so the
/// diagnostic is present on the first run and gone on every run after.
/// `cache_entries_preserve_raw_provider_metadata` above records the time that
/// shipped; this generalizes the guard so the next field cannot repeat it.
#[test]
fn cache_round_trip_preserves_every_response_field() {
    let provider = FlakyProvider {
        calls: AtomicU32::new(0),
    };
    let original = ProviderResponse {
        tool_calls: vec![crate::result::ToolCall {
            id: Some("call_1".to_string()),
            name: "lookup_order".to_string(),
            arguments: serde_json::json!({ "id": 42 }),
        }],
        output: crate::types::Output::Text("answer".to_string()),
        usage: Some(crate::types::TokenUsage {
            input_tokens: 11,
            output_tokens: 7,
            cache_read_tokens: Some(3),
            cache_write_tokens: Some(5),
            cache_write_1h_tokens: Some(2),
        }),
        cost_usd: Some(0.0125),
        stop_reason: Some("end_turn".to_string()),
        raw: Some(serde_json::json!({ "id": "resp_1", "model": "m" })),
        reasoning: Some("let me work through this".to_string()),
        empty_reason: Some(crate::empty::EmptyReason::new(
            crate::empty::EmptyReason::THINKING_ONLY,
        )),
        model: Some("m-2026-01-01".to_string()),
    };

    let replayed = entry_to_response(
        response_to_entry(&provider, &original, test_stats(), &test_request()),
        &provider,
    );

    let ProviderResponse {
        tool_calls,
        output,
        usage,
        cost_usd,
        stop_reason,
        raw,
        reasoning,
        empty_reason,
        model,
    } = replayed;
    assert_eq!(output, original.output);
    assert_eq!(usage, original.usage);
    assert_eq!(cost_usd, original.cost_usd);
    assert_eq!(stop_reason, original.stop_reason);
    assert_eq!(raw, original.raw);
    assert_eq!(reasoning, original.reasoning);
    assert_eq!(empty_reason, original.empty_reason);
    assert_eq!(model, original.model);
    // Without this a `tool-call` assertion passes on the first run of a suite
    // and fails on every run after, which is the common path.
    assert_eq!(tool_calls, original.tool_calls);
}

/// An entry records the request whole, and when it cannot, records what the
/// request *addressed* rather than nothing.
///
/// `raw` answers an oversized payload by dropping it — truncated JSON is
/// useless. A request cannot take that answer: the url or command is what makes
/// a stored key legible at all, and it is the one thing a reader always wants.
/// So the cap trims the payload and keeps the address.
#[test]
fn a_request_is_stored_whole_unless_it_blows_the_cap() {
    let provider = FlakyProvider {
        calls: AtomicU32::new(0),
    };
    let response = ProviderResponse::text("hi");

    let small = response_to_entry(&provider, &response, test_stats(), &test_request());
    assert_eq!(
        small.request.as_ref(),
        Some(&test_request()),
        "an ordinary request is stored verbatim"
    );

    let huge_http = serde_json::json!({
        "transport": "http",
        "method": "POST",
        "url": "https://sut.test/v1/messages",
        "headers_digest": "blake3:deadbeef",
        "body": {"prompt": "x".repeat(128 * 1024)},
    });
    let trimmed = response_to_entry(&provider, &response, test_stats(), &huge_http)
        .request
        .expect("a request is recorded even when it does not fit");
    assert_eq!(
        trimmed["url"], huge_http["url"],
        "the endpoint is never lost"
    );
    assert_eq!(trimmed["method"], serde_json::json!("POST"));
    assert_eq!(trimmed["transport"], serde_json::json!("http"));
    assert!(
        trimmed.get("body").is_none(),
        "the payload is what got trimmed: {trimmed}"
    );

    let huge_exec = serde_json::json!({
        "transport": "exec",
        "command": "./sut",
        "args": ["--mode", "strict"],
        "stdin": {"vars": {"doc": "y".repeat(128 * 1024)}},
    });
    let trimmed = response_to_entry(&provider, &response, test_stats(), &huge_exec)
        .request
        .expect("a request is recorded even when it does not fit");
    assert_eq!(
        trimmed["command"],
        serde_json::json!("./sut"),
        "the command is never lost"
    );
    assert_eq!(trimmed["args"], serde_json::json!(["--mode", "strict"]));
    assert!(
        trimmed.get("stdin").is_none(),
        "the payload is what got trimmed: {trimmed}"
    );
}

/// Entries written before the `raw` field existed keep deserializing (and
/// replay with no raw metadata, as they always did).
#[test]
fn legacy_cache_entries_without_raw_still_parse() {
    let legacy = serde_json::json!({
        "created_at": "2026-01-01T00:00:00Z",
        "provider_fingerprint": { "type": "flaky" },
        "output": "hi",
        "domarinn_version": "0.1.0",
    });
    let entry: CacheEntry = serde_json::from_value(legacy).unwrap();
    assert_eq!(entry.raw, None);
    let provider = FlakyProvider {
        calls: AtomicU32::new(0),
    };
    assert_eq!(entry_to_response(entry, &provider).raw, None);
}

// ── Tracing capture ─────────────────────────────────────────────────────────

/// One recorded event: its level and the NAMES of the fields it carried.
///
/// Field names rather than a rendered line, because the assertion below is
/// specifically that these are *structured fields* and not text interpolated
/// into the message — and a substring match over rendered JSON is satisfied by
/// either.
#[derive(Debug, Clone)]
struct CapturedEvent {
    level: tracing::Level,
    fields: Vec<&'static str>,
}

thread_local! {
    /// `Some` only on a thread currently inside [`capture`].
    static CAPTURED: std::cell::RefCell<Option<Vec<CapturedEvent>>> =
        const { std::cell::RefCell::new(None) };
}

struct FieldNames(Vec<&'static str>);

impl tracing::field::Visit for FieldNames {
    // Every other `record_*` defaults to delegating here.
    fn record_debug(&mut self, field: &tracing::field::Field, _: &dyn std::fmt::Debug) {
        self.0.push(field.name());
    }
}

struct Capture;

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        Some(tracing::level_filters::LevelFilter::TRACE)
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        // Ids must be non-zero; nothing here ever interprets one.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        tracing::span::Id::from_u64(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        CAPTURED.with(|slot| {
            let mut slot = slot.borrow_mut();
            // Not capturing on this thread: leave before visiting anything, so
            // an unrelated test never pays to format a field value.
            let Some(buf) = slot.as_mut() else { return };
            let mut names = FieldNames(Vec::new());
            event.record(&mut names);
            buf.push(CapturedEvent {
                level: *event.metadata().level(),
                fields: names.0,
            });
        });
    }
}

/// Run `f`, returning its value alongside every event it emitted on this thread.
///
/// The subscriber is installed **globally, once** — deliberately, and not the
/// obvious `tracing::subscriber::with_default`, which is what this replaces
/// after that scoped form flaked in CI twice (2026-07-28, 2026-07-30) with an
/// empty buffer while the logic under test ran perfectly.
///
/// The cause is the process-global *callsite interest cache*, not the level
/// filter. `warn!` consults a cached `Interest` for its callsite, and any
/// thread registering a callsite triggers a rebuild that recomputes that cache
/// from the dispatchers it can see. A scoped dispatcher is installed for one
/// thread for the length of one scope, so a rebuild racing that window can
/// cache `Interest::never()` and the macro then drops the event before ever
/// reaching our subscriber. Measured directly: under concurrent dispatcher
/// churn the scoped form lost **5,786–12,553 of 20,000** events across runs;
/// this global form lost 0 of 20,000, three runs in a row. (It is specifically
/// not the max-level hint — of those misses, zero had it below `WARN`.)
///
/// A permanently-registered global dispatcher that is interested in everything
/// leaves no window: every rebuild can see it, so `never` is unreachable.
/// Threads that are not capturing pay one thread-local load per event —
/// [`Capture::event`] returns before visiting, so no field is ever formatted
/// for an unrelated test.
fn capture<T>(f: impl FnOnce() -> T) -> (T, Vec<CapturedEvent>) {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        tracing::subscriber::set_global_default(Capture)
            .expect("no other test in this binary may install a global subscriber");
    });
    CAPTURED.with(|slot| *slot.borrow_mut() = Some(Vec::new()));
    let value = f();
    let events = CAPTURED
        .with(|slot| slot.borrow_mut().take())
        .unwrap_or_default();
    (value, events)
}

/// Guard on the guard: capture must survive the conditions that broke it.
///
/// This reproduces what a loaded CI runner does to a 425-test binary — threads
/// entering and leaving their own dispatchers and forcing interest rebuilds
/// while one thread captures. Restore [`capture`] to `with_default` and this
/// fails on roughly a third to two thirds of its iterations; the whole point is
/// that it is a *reproduction*, since the original flake never reproduced
/// locally in any amount of plain re-running.
#[test]
fn capture_survives_concurrent_dispatcher_churn() {
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut threads = Vec::new();
    for _ in 0..6 {
        let stop = stop.clone();
        threads.push(std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let noop = tracing::subscriber::NoSubscriber::default();
                tracing::subscriber::with_default(noop, || tracing::warn!(noise = 1, "noise"));
            }
        }));
    }
    for _ in 0..2 {
        let stop = stop.clone();
        threads.push(std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                tracing::callsite::rebuild_interest_cache();
            }
        }));
    }

    let mut missed = 0;
    let rounds = 2_000;
    for _ in 0..rounds {
        let (_, events) = capture(|| tracing::warn!(attempt = 1u32, delay_ms = 5u64, "retrying"));
        if !events.iter().any(|e| e.fields.contains(&"attempt")) {
            missed += 1;
        }
    }

    stop.store(true, Ordering::Relaxed);
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(missed, 0, "capture dropped {missed} of {rounds} events");
}

/// The retry warning must carry `attempt` and `delay_ms` as structured
/// fields (not just interpolated into the message), so a `-vv` / JSON log can
/// filter on them.
#[test]
fn retry_warn_carries_structured_attempt_and_delay_fields() {
    let (outcome, events) = capture(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let provider = FlakyProvider {
                calls: AtomicU32::new(0),
            };
            let req = ProviderRequest {
                tools: Vec::new(),
                prompt: None,
                vars: std::collections::BTreeMap::new(),
                params: serde_json::Map::new(),
                test: TestMeta::default(),
                case_salt: None,
            };
            let ctx = CallCtx::default();
            let cache = NoopCache;
            // initial_ms = 1 keeps the single backoff sleep sub-millisecond.
            let retry_cfg = RetryPolicy {
                max: 1,
                initial_ms: 1,
                max_ms: 1,
                ..Default::default()
            };
            let state = crate::runner::runner_cache::CacheRunState::default();
            call_with_cache(
                &provider,
                &req,
                &ctx,
                crate::runner::runner_cache::CacheCall {
                    policy: &crate::empty_policy::EmptyPolicy::default(),
                    backend: &cache,
                    mode: CacheMode::Disabled,
                    repeat: 0,
                    state: &state,
                },
                &retry_cfg,
            )
            .await
            .expect("second attempt succeeds")
        })
    });

    // The logic first: if these fail, the retry itself regressed. Only the
    // field assertions below are about logging.
    assert!(!outcome.cached);
    assert_eq!(outcome.attempts, Some(2));

    let warn = events
        .iter()
        .find(|e| e.level == tracing::Level::WARN)
        .unwrap_or_else(|| panic!("the retry path must warn; captured {events:?}"));
    assert!(
        warn.fields.contains(&"attempt"),
        "retry warn must record `attempt` as a field; got {:?}",
        warn.fields
    );
    assert!(
        warn.fields.contains(&"delay_ms"),
        "retry warn must record `delay_ms` as a field; got {:?}",
        warn.fields
    );
}

// ── The grader-auth circuit breaker ──────────────────────────────────────────

/// The first rejection is the cause; the rest are it happening again.
#[test]
fn the_abort_flag_keeps_the_first_reason() {
    let flag = AbortFlag::default();
    assert!(!flag.is_poisoned());
    assert_eq!(flag.reason(), None);

    flag.poison("grader credential rejected (HTTP 401)".into());
    flag.poison("grader credential rejected (HTTP 403)".into());

    assert!(flag.is_poisoned());
    let reason = flag.reason().expect("poisoned");
    assert!(reason.contains("401"), "{reason}");
    assert!(
        !reason.contains("403"),
        "later failures must not overwrite: {reason}"
    );
    assert!(reason.starts_with("aborted: "), "{reason}");
}

/// A rejected credential is the caller's problem, not a flaky grader — the
/// distinction that decides whether CI retries or stops.
#[test]
fn a_rejected_grader_credential_is_classified_as_auth() {
    use crate::errors::Classify;
    let auth = crate::errors::GraderError::AuthRejected { status: 401 };
    let broke = crate::errors::GraderError::Transport("connection reset".into());
    assert_eq!(auth.class().as_str(), ErrorClass::PROVIDER_AUTH);
    assert_eq!(broke.class().as_str(), ErrorClass::GRADER_FAILED);
}

// ── CaseStatus::Skip ─────────────────────────────────────────────────────────

/// `Skip` has been defined, counted, rendered and TS-exported since it shipped,
/// and never produced. This is the policy that produces it.
#[test]
fn an_empty_reason_only_skips_when_the_suite_asked_for_it() {
    let tool_use = crate::empty::EmptyReason::new(crate::empty::EmptyReason::TOOL_USE_ONLY);
    let refusal = crate::empty::EmptyReason::new(crate::empty::EmptyReason::REFUSAL);
    let configured = vec![crate::empty::EmptyReason::TOOL_USE_ONLY.to_string()];

    assert!(reasoning_is_skippable(Some(&tool_use), &configured));
    // A refusal is a real result about the prompt unless the suite says
    // otherwise, so it is graded rather than skipped.
    assert!(!reasoning_is_skippable(Some(&refusal), &configured));
    // Opt-in: an empty list changes nothing, which is the default.
    assert!(!reasoning_is_skippable(Some(&tool_use), &[]));
    assert!(!reasoning_is_skippable(None, &configured));
}

/// `EmptyReason` is open by construction; this must not be the one place that
/// closes it, so a vendor reason from the future can still be skipped.
#[test]
fn a_reason_this_build_has_never_heard_of_can_still_be_skipped() {
    let future = crate::empty::EmptyReason::new("invented_next_year");
    assert!(reasoning_is_skippable(
        Some(&future),
        &["invented_next_year".to_string()]
    ));
}

// ── The vacuous-pass guard, end to end ───────────────────────────────────────

/// A provider that produces nothing gradeable: an empty output, whatever
/// reason it cares to report, and whatever calls it made on the way.
struct EmptyProvider {
    empty_reason: Option<crate::empty::EmptyReason>,
    tool_calls: Vec<crate::result::ToolCall>,
}

impl EmptyProvider {
    /// Declined outright: nothing said, nothing done.
    fn refusing() -> Self {
        EmptyProvider {
            empty_reason: Some(crate::empty::EmptyReason::new(
                crate::empty::EmptyReason::REFUSAL,
            )),
            tool_calls: Vec::new(),
        }
    }

    /// Answered with a tool call and said nothing else — an empty *text*
    /// output over a response that did something.
    fn tool_only() -> Self {
        EmptyProvider {
            empty_reason: Some(crate::empty::EmptyReason::new(
                crate::empty::EmptyReason::TOOL_USE_ONLY,
            )),
            tool_calls: vec![crate::result::ToolCall {
                id: None,
                name: "get_weather".into(),
                arguments: serde_json::json!({"city": "Oslo"}),
            }],
        }
    }

    /// Blank with no vendor reason at all, so `classify_empty` is what supplies
    /// one — the case where the raw field and the classified reason differ.
    fn blank() -> Self {
        EmptyProvider {
            empty_reason: None,
            tool_calls: Vec::new(),
        }
    }
}

#[async_trait]
impl Provider for EmptyProvider {
    fn id(&self) -> &str {
        "empty"
    }
    fn fingerprint(&self) -> Json {
        serde_json::json!({ "type": "empty" })
    }
    async fn call(
        &self,
        _req: &ProviderRequest,
        _ctx: &CallCtx,
    ) -> Result<ProviderResponse, ProviderError> {
        Ok(ProviderResponse {
            empty_reason: self.empty_reason.clone(),
            tool_calls: self.tool_calls.clone(),
            ..ProviderResponse::text("")
        })
    }
}

fn not_contains() -> crate::config::AssertKind {
    crate::config::AssertKind::Contains {
        value: "forbidden".into(),
    }
}

fn rubric() -> crate::config::AssertKind {
    crate::config::AssertKind::LlmRubric {
        value: "is rude".into(),
        grader: None,
        threshold: None,
        params: None,
    }
}

/// A grader whose judgement is always "the output did not satisfy the rubric"
/// — which a negated assert would otherwise turn into a pass.
struct FailingGrader;

#[async_trait]
impl AssertGrader for FailingGrader {
    async fn grade(
        &self,
        _assert: &crate::config::Assert,
        _output: &crate::types::Output,
        _ctx: &GradeCtx<'_>,
    ) -> Result<crate::cache::Graded, crate::errors::GraderError> {
        Ok(crate::cache::Graded::unpriced(
            crate::cache::GradedVerdict::Rubric {
                score: 0.0,
                pass: false,
                reasoning: "nothing to judge".into(),
            },
        ))
    }
}

/// Run one cell against an [`EmptyProvider`] with a single assert.
async fn empty_case(
    provider: &EmptyProvider,
    kind: crate::config::AssertKind,
    negate: bool,
    skip_on_empty_reason: &[String],
) -> CaseResult {
    let engine = TemplateEngine::new();
    let cache = NoopCache;
    let schemas = crate::jsonschema_cache::SchemaCache::new();
    let aborted = AbortFlag::default();
    let cache_state = crate::runner::runner_cache::CacheRunState::default();
    let test = crate::config::TestCase {
        id: Some("t1".into()),
        assert: vec![crate::config::Assert {
            weight: 1.0,
            negate,
            kind,
        }],
        ..Default::default()
    };
    run_cell(
        provider,
        &[],
        None,
        &test,
        0,
        &engine,
        &cache,
        Some(&FailingGrader),
        &CallCtx::default(),
        Path::new("."),
        CacheMode::Disabled,
        false,
        &RetryPolicy::default(),
        &schemas,
        false,
        &aborted,
        skip_on_empty_reason,
        &[],
        &cache_state,
        &crate::empty_policy::EmptyPolicy::default(),
        &crate::runner::runner_fallback::FallbackPolicy::default(),
    )
    .await
}

/// The hole this closes: `not-contains` over a refusal scored 1.00 and the
/// case reported green, for a run the model never attempted.
#[tokio::test]
async fn a_refusal_with_negated_asserts_lands_in_fail_not_pass() {
    let case = empty_case(&EmptyProvider::refusing(), not_contains(), true, &[]).await;
    assert_eq!(case.status, CaseStatus::Fail, "{:?}", case.asserts);
    assert_eq!(case.score, 0.0);
    assert!(
        case.asserts[0].reason.contains("vacuously"),
        "{}",
        case.asserts[0].reason
    );
}

/// `skip_on_empty_reason` is checked before the verdict, so a suite that opted
/// out of grading refusals still gets `Skip` rather than the guard's `Fail`.
#[tokio::test]
async fn skip_on_empty_reason_still_wins_over_the_vacuous_pass_guard() {
    let case = empty_case(
        &EmptyProvider::refusing(),
        not_contains(),
        true,
        &[crate::empty::EmptyReason::REFUSAL.to_string()],
    )
    .await;
    assert_eq!(case.status, CaseStatus::Skip, "{:?}", case.asserts);
}

/// The graded seam: a negated `llm-rubric` over a refusal is the same vacuous
/// pass, and the guard has to sit before the score the case is graded on.
#[tokio::test]
async fn a_negated_graded_assert_cannot_pass_vacuously_either() {
    let case = empty_case(&EmptyProvider::refusing(), rubric(), true, &[]).await;
    assert_eq!(case.status, CaseStatus::Fail, "{:?}", case.asserts);
    assert_eq!(case.score, 0.0, "the scored verdict agrees with the result");
    assert!(
        case.asserts[0].reason.contains("vacuously"),
        "{}",
        case.asserts[0].reason
    );
}

/// ...and the graded seam honours the same exemption as the local one: the
/// judge is shown the tool calls, so a verdict over a tool-only response is a
/// judgement about something the model actually did.
#[tokio::test]
async fn a_negated_graded_assert_keeps_its_verdict_when_the_model_called_a_tool() {
    let case = empty_case(&EmptyProvider::tool_only(), rubric(), true, &[]).await;
    assert_eq!(case.status, CaseStatus::Pass, "{:?}", case.asserts);
    assert_eq!(case.score, 1.0);
}

/// `skip_on_empty_reason` matches the reason the case *reports*, which for a
/// blank answer no provider diagnosed is the one `classify_empty` supplied.
/// Reading the raw provider field instead made `["blank"]` match nothing.
#[tokio::test]
async fn a_blank_output_is_skippable_by_the_reason_the_case_reports() {
    let provider = EmptyProvider::blank();
    // Un-negated and failing, so `Skip` is the ladder's doing and not the
    // vacuous-pass guard's.
    let graded = empty_case(&provider, not_contains(), false, &[]).await;
    assert_eq!(graded.status, CaseStatus::Fail);
    assert_eq!(
        graded.empty_reason.as_ref().map(|r| r.as_str()),
        Some(crate::empty::EmptyReason::BLANK),
        "the case reports the classified reason"
    );

    let skipped = empty_case(
        &provider,
        not_contains(),
        false,
        &[crate::empty::EmptyReason::BLANK.to_string()],
    )
    .await;
    assert_eq!(skipped.status, CaseStatus::Skip, "{:?}", skipped.asserts);
}
