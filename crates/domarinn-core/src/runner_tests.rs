//! Unit tests for [`super`] (the runner). Split out of `runner.rs` via
//! `#[path]` to keep that file under the repo's 1000-line source cap;
//! this is still the runner's private child module (`use super::*`).

use super::*;
use crate::cache::{CacheError, CacheStats, PurgeFilter};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
            Err(ProviderError::Retriable {
                source: anyhow::anyhow!("boom"),
                retry_after: None,
            })
        } else {
            Ok(ProviderResponse::text("ok"))
        }
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
        output: crate::types::Output::Text("hi".to_string()),
        usage: None,
        cost_usd: None,
        stop_reason: Some("stop".to_string()),
        raw: Some(raw.clone()),
    };

    let entry = response_to_entry(&provider, &response);
    assert_eq!(entry.raw, Some(raw.clone()));
    let replayed = entry_to_response(entry);
    assert_eq!(replayed.raw, Some(raw));
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
    assert_eq!(entry_to_response(entry).raw, None);
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
                prompt: None,
                vars: std::collections::BTreeMap::new(),
                params: serde_json::Map::new(),
                test: TestMeta::default(),
                case_salt: None,
            };
            let ctx = CallCtx::default();
            let cache = NoopCache;
            // initial_ms = 1 keeps the single backoff sleep sub-millisecond.
            let retry_cfg = RetryConfig {
                max: 1,
                initial_ms: 1,
                max_ms: 1,
            };
            let (_resp, cached, attempts) = call_with_cache(
                &provider,
                &req,
                &ctx,
                &cache,
                CacheMode::Disabled,
                0,
                &retry_cfg,
            )
            .await
            .expect("second attempt succeeds");
            assert!(!cached);
            assert_eq!(attempts, 2);
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

#[test]
fn backoff_grows_and_caps() {
    let cfg = RetryConfig {
        max: 5,
        initial_ms: 100,
        max_ms: 1000,
    };
    assert_eq!(cfg.backoff(1, None), Duration::from_millis(100));
    assert_eq!(cfg.backoff(2, None), Duration::from_millis(200));
    assert_eq!(cfg.backoff(3, None), Duration::from_millis(400));
    // Caps at max_ms.
    assert_eq!(cfg.backoff(10, None), Duration::from_millis(1000));
}

#[test]
fn backoff_honors_retry_after_hint() {
    let cfg = RetryConfig {
        max: 5,
        initial_ms: 100,
        max_ms: 1000,
    };
    assert_eq!(
        cfg.backoff(1, Some(Duration::from_secs(7))),
        Duration::from_secs(7)
    );
}
