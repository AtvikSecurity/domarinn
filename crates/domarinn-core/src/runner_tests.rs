//! Unit tests for [`super`] (the runner). Split out of `runner.rs` via
//! `#[path]` to keep that file under the repo's 1000-line source cap;
//! this is still the runner's private child module (`use super::*`).

use super::runner_cache::{entry_to_response, response_to_entry};
use super::*;
use crate::cache::{CacheEntry, CacheError, CacheStats, PurgeFilter};
use crate::provider::{ProviderError, ProviderResponse};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

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

/// The retry warning must carry `attempt` and `delay_ms` as structured
/// fields (not just interpolated into the message), so a `-vv` / JSON log can
/// filter on them. Scoped capture subscriber; inert for every other test.
#[test]
fn retry_warn_carries_structured_attempt_and_delay_fields() {
    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(BufWriter(buf.clone()))
        .finish();

    tracing::subscriber::with_default(subscriber, || {
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
            let outcome = call_with_cache(
                &provider,
                &req,
                &ctx,
                crate::runner::runner_cache::CacheCall {
                    backend: &cache,
                    mode: CacheMode::Disabled,
                    repeat: 0,
                    state: &state,
                },
                &retry_cfg,
            )
            .await
            .expect("second attempt succeeds");
            assert!(!outcome.cached);
            assert_eq!(outcome.attempts, Some(2));
        });
    });

    let logged = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
    assert!(
        logged.contains("\"attempt\""),
        "retry warn must record an `attempt` field; got: {logged}"
    );
    assert!(
        logged.contains("\"delay_ms\""),
        "retry warn must record a `delay_ms` field; got: {logged}"
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
