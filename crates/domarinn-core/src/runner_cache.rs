//! The provider call path: consult the cache, retry what is retriable, and
//! translate between a live [`ProviderResponse`] and a stored [`CacheEntry`].
//!
//! Split out of `runner.rs` — which orchestrates the matrix — so each file stays
//! under the per-file line ratchet (`tests/file_length.rs`), following the seam
//! `runner_asserts.rs` established. Included as a private child module of
//! `runner`, hence the `pub(super)` items.
//!
//! The two translation functions are deliberately adjacent: they are inverses,
//! and a field added to one and forgotten in the other is how a cache hit starts
//! replaying less than the call it stood in for.

use chrono::Utc;
use serde_json::Value as Json;

use crate::cache::{CacheBackend, CacheEntry, CacheMode};
use crate::cache_key::provider_cache_key;
use crate::error_class::ErrorClass;
use crate::provider::{CallCtx, Provider, ProviderError, ProviderRequest, ProviderResponse};
use crate::retry::{with_retry, RetryPolicy, RetryStats};

use super::runner_result::json_to_persist;

/// One provider call's result, plus how it was obtained.
pub(super) struct CallOutcome {
    pub response: ProviderResponse,
    pub cached: bool,
    /// `None` only for a cache hit on an entry written before entries recorded
    /// attempts — honest about not knowing, where the old `0` sentinel was not.
    pub attempts: Option<u32>,
    /// In-flight provider time, excluding retry backoff. On a cache hit this is
    /// the *original* call's latency replayed from the entry, not the cache-read
    /// time.
    pub provider_latency_ms: Option<u64>,
}

/// A failed provider call. Carries the attempt count so an errored case can
/// report what it actually spent instead of a hardcoded `1`.
#[derive(Debug)]
pub(super) struct CallFailure {
    pub message: String,
    pub attempts: u32,
    /// What kind of failure this was, carried alongside the prose rather than
    /// re-derived from it. See [`crate::error_class`].
    pub class: ErrorClass,
    /// Structured diagnostics the provider attached, if any.
    pub details: Option<Json>,
}

impl CallFailure {
    /// A failure that never reached the provider (cache read error, cache-only
    /// miss) — no attempt was made against the system under test.
    pub(super) fn before_any_attempt(class: &str, message: String) -> Self {
        CallFailure {
            message,
            attempts: 0,
            class: ErrorClass::new(class),
            // domarinn generated this failure itself, so there is no
            // provider-authored detail to carry.
            details: None,
        }
    }
}

/// Call a provider, consulting the cache per `mode` and retrying retriable
/// errors with backoff.
#[tracing::instrument(name = "provider_call", skip_all, fields(provider = %provider.id()))]
pub(super) async fn call_with_cache(
    provider: &dyn Provider,
    req: &ProviderRequest,
    ctx: &CallCtx,
    cache: &dyn CacheBackend,
    mode: CacheMode,
    repeat: u32,
    retry_cfg: &RetryPolicy,
) -> Result<CallOutcome, CallFailure> {
    let use_cache = mode != CacheMode::Disabled && provider.cacheable();
    let key = use_cache.then(|| provider_cache_key(&provider.fingerprint(), req, repeat));

    if let Some(key) = &key {
        match cache.get(key).await {
            Ok(Some(entry)) => {
                tracing::debug!(%key, "cache hit");
                let attempts = entry.attempts;
                let provider_latency_ms = entry.provider_latency_ms;
                return Ok(CallOutcome {
                    response: entry_to_response(entry),
                    cached: true,
                    attempts,
                    provider_latency_ms,
                });
            }
            Ok(None) => {
                tracing::debug!(%key, "cache miss");
                if mode == CacheMode::ReadOnlyStrict {
                    return Err(CallFailure::before_any_attempt(
                        ErrorClass::CACHE_MISS,
                        format!("cache-only: miss for key {key}"),
                    ));
                }
            }
            Err(e) => {
                return Err(CallFailure::before_any_attempt(
                    ErrorClass::CACHE_UNAVAILABLE,
                    format!("cache read error: {e}"),
                ))
            }
        }
    }

    let (result, stats) = with_retry(retry_cfg, |_attempt| provider.call(req, ctx)).await;

    match result {
        Ok(response) => {
            if let Some(key) = &key {
                if mode == CacheMode::ReadWrite {
                    let entry = response_to_entry(provider, &response, stats);
                    // A cache write failure must not fail the run.
                    if let Err(e) = cache.put(key, &entry).await {
                        tracing::warn!(error = %e, "cache write failed");
                    }
                }
            }
            Ok(CallOutcome {
                response,
                cached: false,
                attempts: Some(stats.attempts),
                provider_latency_ms: Some(stats.in_flight.as_millis() as u64),
            })
        }
        Err(err) => {
            let class = err.class().clone();
            let details = err.details().cloned();
            let message = match &err {
                ProviderError::Retriable { source, .. } => format!(
                    "provider error after {} attempt(s): {source}",
                    stats.attempts
                ),
                ProviderError::Fatal { source, .. } => format!("provider error: {source}"),
            };
            Err(CallFailure {
                message,
                attempts: stats.attempts,
                class,
                details,
            })
        }
    }
}

pub(super) fn entry_to_response(entry: CacheEntry) -> ProviderResponse {
    ProviderResponse {
        output: entry.output,
        usage: entry.usage,
        cost_usd: entry.cost_usd,
        stop_reason: entry.stop_reason,
        raw: entry.raw,
        reasoning: entry.reasoning,
        empty_reason: entry.empty_reason,
        model: entry.model,
        tool_calls: entry.tool_calls,
    }
}

pub(super) fn response_to_entry(
    provider: &dyn Provider,
    response: &ProviderResponse,
    stats: RetryStats,
) -> CacheEntry {
    CacheEntry {
        created_at: Utc::now(),
        provider_fingerprint: provider.fingerprint(),
        output: response.output.clone(),
        usage: response.usage.clone(),
        cost_usd: response.cost_usd,
        stop_reason: response.stop_reason.clone(),
        attempts: Some(stats.attempts),
        provider_latency_ms: Some(stats.in_flight.as_millis() as u64),
        reasoning: response.reasoning.clone(),
        empty_reason: response.empty_reason.clone(),
        model: response.model.clone(),
        tool_calls: response.tool_calls.clone(),
        // Provider responses never carry a verdict; its absence is what marks
        // this entry as a provider response rather than a grading result.
        verdict: None,
        // Same size cap as persistence, so a pathological payload can't bloat
        // the shared cache. `--no-raw` intentionally does NOT strip the cache
        // copy: a later run without the flag replaying this entry should still
        // get the metadata.
        raw: json_to_persist(true, response.raw.clone(), "raw"),
        domarinn_version: crate::VERSION.to_string(),
    }
}
