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

use std::collections::HashSet;
use std::sync::Mutex;

use chrono::Utc;
use serde_json::Value as Json;

use crate::cache::{CacheBackend, CacheEntry, CacheKey, CacheMode};
use crate::cache_key::request_cache_key;
use crate::cache_migrate::{legacy_provider_key, MigrationProbe};
use crate::error_class::ErrorClass;
use crate::provider::{CallCtx, Provider, ProviderError, ProviderRequest, ProviderResponse};
use crate::retry::{with_retry, RetryPolicy, RetryStats};

use super::runner_result::{json_to_persist, RAW_MAX_BYTES};

/// Run-scoped state the cache path needs but a single cell must not own.
///
/// Both members exist to make something happen *once per run* rather than once
/// per case: a legacy-key probe that gives up when there is nothing to find,
/// and a rebuilt-program warning that would otherwise repeat for every cell of
/// an affected provider.
#[derive(Debug, Default)]
pub(super) struct CacheRunState {
    pub migration: MigrationProbe,
    /// Provider ids already warned about a program that has been rebuilt since
    /// its entries were written.
    drift_warned: Mutex<HashSet<String>>,
}

impl CacheRunState {
    pub(super) fn new(migration: MigrationProbe) -> Self {
        CacheRunState {
            migration,
            drift_warned: Mutex::new(HashSet::new()),
        }
    }

    /// True the first time this provider is seen, so a caller can warn once.
    ///
    /// A poisoned lock means another thread panicked mid-warning; the honest
    /// response is to say nothing rather than to bring the run down over a
    /// diagnostic, so this reports "already warned".
    fn claim_drift_warning(&self, provider_id: &str) -> bool {
        self.drift_warned
            .lock()
            .map(|mut seen| seen.insert(provider_id.to_string()))
            .unwrap_or(false)
    }
}

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

/// Everything the cache side of one provider call needs.
///
/// Grouped rather than passed loose because the four travel together and mean
/// nothing apart: which store, how to use it, which trial, and the run-scoped
/// state that makes probing and warning happen once instead of per case.
#[derive(Clone, Copy)]
pub(super) struct CacheCall<'a> {
    pub backend: &'a dyn CacheBackend,
    pub mode: CacheMode,
    pub repeat: u32,
    pub state: &'a CacheRunState,
}

/// Call a provider, consulting the cache per `mode` and retrying retriable
/// errors with backoff.
#[tracing::instrument(name = "provider_call", skip_all, fields(provider = %provider.id()))]
pub(super) async fn call_with_cache(
    provider: &dyn Provider,
    req: &ProviderRequest,
    ctx: &CallCtx,
    cache: CacheCall<'_>,
    retry_cfg: &RetryPolicy,
) -> Result<CallOutcome, CallFailure> {
    let CacheCall {
        backend: cache,
        mode,
        repeat,
        state,
    } = cache;
    // Two gates compose into one: a provider may decline caching outright
    // (`cacheable`), and a *call* may have no stable identity to key on — an
    // `http` request whose url or body will not render, a vendor call with no
    // prompt. Either way there is no key, so the call goes live and unrecorded.
    // A nondeterministic key would be worse than no key: it never hits, and it
    // fills the store with entries nobody will ever ask for again.
    let use_cache = mode != CacheMode::Disabled && provider.cacheable();
    let canonical = use_cache.then(|| provider.canonical_request(req)).flatten();
    let key = canonical.as_ref().map(|request| {
        request_cache_key(
            request,
            repeat,
            provider.cache_salt(),
            req.case_salt.as_deref(),
        )
    });

    if let (Some(key), Some(canonical)) = (&key, &canonical) {
        match cache.get(key).await {
            Ok(Some(entry)) => {
                tracing::debug!(%key, "cache hit");
                return Ok(hit(provider, entry, state));
            }
            Ok(None) => {
                tracing::debug!(%key, "cache miss");
                // Before paying for this, check whether an older domarinn already
                // answered it under the key shape that has since changed.
                if let Some(mut entry) = adopt_legacy(provider, req, repeat, cache, state).await {
                    // Re-file it under the current key so the next run finds it
                    // directly and the probe budget is not spent again. A write
                    // failure is not fatal: the entry was still served.
                    if mode == CacheMode::ReadWrite {
                        // An adopted entry predates the request being recorded,
                        // so give it one: under the new key the request is what
                        // the entry is *about*, and re-filing without it would
                        // put a ≤0.4-era entry into the new era half-formed.
                        // `provider_fingerprint` is left as written — it is the
                        // provenance of the call that actually happened.
                        entry.request = Some(request_to_persist(canonical));
                        if let Err(e) = cache.put(key, &entry).await {
                            tracing::warn!(error = %e, "adopting a legacy cache entry: write failed");
                        }
                    }
                    return Ok(hit(provider, entry, state));
                }
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
            if let (Some(key), Some(canonical)) = (&key, &canonical) {
                if mode == CacheMode::ReadWrite {
                    let entry = response_to_entry(provider, &response, stats, canonical);
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

/// Turn a found entry into an outcome, reporting anything about it that the key
/// deliberately does not cover.
fn hit(provider: &dyn Provider, entry: CacheEntry, state: &CacheRunState) -> CallOutcome {
    warn_on_program_drift(provider, &entry, state);
    let attempts = entry.attempts;
    let provider_latency_ms = entry.provider_latency_ms;
    CallOutcome {
        response: entry_to_response(entry, provider),
        cached: true,
        attempts,
        provider_latency_ms,
    }
}

/// Say so, once per provider, when the program on disk is not the one that
/// produced these answers.
///
/// This is the whole compensation for keeping the program's bytes out of the
/// cache key. The key stays portable; the rebuild stays visible. Nothing is
/// invalidated — deciding that a rebuild matters is the suite's call, and
/// `cache_salt` is how it makes it.
fn warn_on_program_drift(provider: &dyn Provider, entry: &CacheEntry, state: &CacheRunState) {
    let (Some(live), Some(stored)) = (provider.program_digest(), entry.program_digest.as_deref())
    else {
        // One side or the other has no digest: an entry written before the field
        // existed, or a command that resolves to nothing on disk. Absence is not
        // evidence of a change, so there is nothing honest to report.
        return;
    };
    if live == stored || !state.claim_drift_warning(provider.id()) {
        return;
    }
    tracing::warn!(
        provider = %provider.id(),
        "replaying cached answers produced by a different build of this provider's \
         program. The cache key names the command, not its bytes, so a rebuild does \
         not invalidate anything on its own — set `cache_salt` (a git SHA, or \
         `$digest:` over the sources) when a rebuild should re-run the suite."
    );
}

/// Look for this request under the key shape domarinn used to publish.
///
/// Returns the first entry found, or `None` when there is nothing to adopt,
/// when the probe budget is spent, or when the provider has no history. See
/// [`crate::cache_migrate`] for the budget's rationale.
///
/// A read error on a legacy key is swallowed rather than propagated: this is a
/// bonus lookup on a key that already missed, so failing the case over it would
/// turn an optimisation into an outage.
///
/// Runs in every cache mode that got this far, `--cache-only` included — that
/// is where it earns the most, since a run with no live call to fall back on
/// would otherwise report an infrastructure failure over an answer the store
/// already holds.
async fn adopt_legacy(
    provider: &dyn Provider,
    req: &ProviderRequest,
    repeat: u32,
    cache: &dyn CacheBackend,
    state: &CacheRunState,
) -> Option<CacheEntry> {
    let legacy = provider.legacy_fingerprints();
    if legacy.is_empty() || !state.migration.should_probe() {
        return None;
    }
    for fingerprint in legacy {
        let key: CacheKey = legacy_provider_key(&fingerprint, req, repeat);
        match cache.get(&key).await {
            Ok(Some(entry)) => {
                state.migration.record_adoption();
                tracing::debug!(%key, "adopted a cache entry written under a previous key shape");
                return Some(entry);
            }
            Ok(None) => continue,
            Err(e) => {
                tracing::debug!(error = %e, %key, "legacy cache probe failed; ignoring");
                // A backend that errors once will error for every remaining
                // shape too, so stop rather than multiplying the failure.
                break;
            }
        }
    }
    None
}

/// Replay a stored response, re-deriving anything that is a matter of current
/// policy rather than of what the provider said.
///
/// `cost_usd` is re-derived. The stored figure was priced by whatever rate sheet
/// was current when the call happened, so replaying it verbatim means a warm
/// suite reports last year's prices forever and a `cost:` budget scores against
/// them. Pricing is deliberately not in the cache key — keying it would discard
/// every entry the day a vendor changes a price — so it is applied on read, the
/// same shape as [`crate::cache::GradedVerdict::to_outcome`] applying a
/// threshold to a stored verdict.
///
/// The stored value remains the fallback, and it is the only answer for `exec`,
/// where the child reports a cost from its own accounting that domarinn cannot
/// recompute from token counts it never saw.
pub(super) fn entry_to_response(entry: CacheEntry, provider: &dyn Provider) -> ProviderResponse {
    let cost_usd = recost(entry.usage.as_ref(), provider).or(entry.cost_usd);
    ProviderResponse {
        output: entry.output,
        usage: entry.usage,
        cost_usd,
        stop_reason: entry.stop_reason,
        raw: entry.raw,
        reasoning: entry.reasoning,
        empty_reason: entry.empty_reason,
        model: entry.model,
        tool_calls: entry.tool_calls,
    }
}

/// Price stored token counts at the provider's current rate, or `None` when
/// either is missing.
fn recost(usage: Option<&crate::types::TokenUsage>, provider: &dyn Provider) -> Option<f64> {
    let rate = provider.rate()?;
    Some(crate::pricing::cost_of(usage?, rate)?.to_usd())
}

/// Fit a canonical request into an entry without ever losing what it addressed.
///
/// Same size cap as `raw` (an entry travels to a shared store, and a
/// pathological payload must not bloat it), but a different answer on overflow:
/// `raw` is dropped whole, where an oversized request keeps a slim envelope. The
/// url or command is what makes an entry legible — "which endpoint did this
/// answer come from" is the question a stored key cannot answer on its own — so
/// it is recorded unconditionally. The body/stdin is what got big, and it is the
/// part a reader can least act on.
///
/// Everything outside that address is dropped when it trims — for `http` that
/// is `body` and `headers_digest`, for `anthropic` also the `headers` member
/// carrying `anthropic-version`, for `exec` the `stdin` and `env_digest`. A slim
/// entry therefore no longer re-hashes to its own key. That is the trade: the
/// key is already stored (it is where the entry lives), so what is lost is
/// offline re-derivation for the rare huge request, and what is kept is every
/// entry staying under the cap.
///
/// `--no-raw` deliberately has no say here: it governs what a *run document*
/// publishes, and a cache entry is not a run document — a later run without the
/// flag replaying this entry should still see what was asked.
pub(crate) fn request_to_persist(canonical: &Json) -> Json {
    /// The members that name *what was addressed*, across both envelope shapes:
    /// `{transport, method, url}` for http, `{transport, command, args}` for
    /// exec. Projection rather than removal, so an envelope shape added later
    /// trims to something honest instead of leaking a payload this never saw.
    const ADDRESS_MEMBERS: &[&str] = &["transport", "method", "url", "command", "args"];

    let fits = serde_json::to_vec(canonical)
        .map(|bytes| bytes.len() <= RAW_MAX_BYTES)
        .unwrap_or(false);
    if fits {
        return canonical.clone();
    }
    let Some(members) = canonical.as_object() else {
        return Json::Null;
    };
    tracing::debug!(
        max_bytes = RAW_MAX_BYTES,
        "storing a slim canonical request: the full one exceeds the entry cap"
    );
    Json::Object(
        ADDRESS_MEMBERS
            .iter()
            .filter_map(|name| members.get_key_value(*name))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    )
}

pub(super) fn response_to_entry(
    provider: &dyn Provider,
    response: &ProviderResponse,
    stats: RetryStats,
    canonical: &Json,
) -> CacheEntry {
    CacheEntry {
        created_at: Utc::now(),
        // The request replaces the fingerprint rather than joining it: the key
        // is derived from the request now, so the fingerprint would be
        // provenance for a key nothing computes. Entries in the wild still carry
        // it, which is why the field survives as `Option`.
        provider_fingerprint: None,
        request: Some(request_to_persist(canonical)),
        // Which build answered — recorded, never keyed. See
        // `warn_on_program_drift` for what reads it back.
        program_digest: provider.program_digest().map(str::to_string),
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
