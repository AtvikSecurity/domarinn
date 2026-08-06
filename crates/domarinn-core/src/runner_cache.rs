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

use crate::cache::{CacheBackend, CacheEntry, CacheKey, CacheMode, EntryKind};
use crate::cache_adopt::MigrationProbe;
use crate::cache_key::request_cache_key;
use crate::cache_migrate::legacy_provider_key;
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
    address_warned: Mutex<HashSet<String>>,
    no_store_warned: Mutex<HashSet<String>>,
}

impl CacheRunState {
    pub(super) fn new(migration: MigrationProbe) -> Self {
        CacheRunState {
            migration,
            drift_warned: Mutex::new(HashSet::new()),
            address_warned: Mutex::new(HashSet::new()),
            no_store_warned: Mutex::new(HashSet::new()),
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

    /// The same, for the endpoint-drift warning.
    ///
    /// A separate set rather than a shared one: a provider can have both a
    /// rebuilt program and a moved endpoint, and the two say different things.
    fn claim_address_warning(&self, provider_id: &str) -> bool {
        self.address_warned
            .lock()
            .map(|mut seen| seen.insert(provider_id.to_string()))
            .unwrap_or(false)
    }

    /// The same, for replaying an entry this suite would no longer write.
    ///
    /// Its own set for the same reason the other two are separate: a provider
    /// can be rebuilt, moved, *and* serving a stale refusal, and collapsing them
    /// would mean the first warning silenced the other two.
    fn claim_no_store_warning(&self, provider_id: &str) -> bool {
        self.no_store_warned
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
    /// The key this call was addressed by, hit or miss.
    ///
    /// `None` when the call had no key at all — `--no-cache`, a provider that
    /// declines caching, or a request with no stable canonical form. Recorded
    /// on the case so an entry can name the runs that used it.
    pub cache_key: Option<CacheKey>,
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
    /// The key this call was addressed by, when it had one.
    ///
    /// Set on the `--cache-only` miss path in particular: that failure names a
    /// key in its own message, and it is exactly the case someone wants to look
    /// up. Recording it makes the errored case link to the entry that is
    /// missing.
    pub cache_key: Option<CacheKey>,
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
            cache_key: None,
        }
    }

    /// The same, for a failure that knows which key it was looking for.
    pub(super) fn before_any_attempt_for(class: &str, message: String, key: &CacheKey) -> Self {
        CallFailure {
            cache_key: Some(key.clone()),
            ..CallFailure::before_any_attempt(class, message)
        }
    }
}

/// Everything the cache side of one provider call needs.
///
/// Grouped rather than passed loose because they travel together and mean
/// nothing apart: which store, how to use it, which trial, the run-scoped state
/// that makes probing and warning happen once instead of per case, and what
/// this run considers worth storing.
#[derive(Clone, Copy)]
pub(super) struct CacheCall<'a> {
    pub backend: &'a dyn CacheBackend,
    pub mode: CacheMode,
    pub repeat: u32,
    pub state: &'a CacheRunState,
    /// Read on three paths: the write gate below, the two legacy re-files, and
    /// the replay warning in [`hit`]. A reference rather than a value so
    /// `CacheCall` stays `Copy` — it is passed by value to every cell.
    pub policy: &'a crate::empty_policy::EmptyPolicy,
    /// Whether this call may spend the run's legacy-adoption probe budget.
    ///
    /// `false` for every non-primary link of a `fallback:` chain. The budget is
    /// run-global (see [`crate::cache_adopt`]), so a walked chain would spend
    /// two or three probes per case instead of one — draining it in a handful
    /// of cells and silently stranding every legacy entry a store still holds.
    ///
    /// A flag rather than a probe-disabled `CacheRunState`: the state also owns
    /// the once-per-provider warning sets, and a second one would turn "warn
    /// once per run" into "warn once per cell" for exactly the fallback links
    /// most likely to be replaying something worth one line.
    pub probe_legacy: bool,
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
        policy,
        probe_legacy,
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
                return Ok(hit(provider, entry, state, key, policy));
            }
            Ok(None) => {
                tracing::debug!(%key, "cache miss");
                // An entry written under an earlier *canonical request* — same
                // key function, older document. Tried first: every store written
                // between 0.5.0 and 0.7.x has these, where only a ≤0.4.x store
                // has the fingerprint-keyed kind below.
                if let Some((_, mut entry)) =
                    adopt_legacy_canonical(provider, req, repeat, cache, state, probe_legacy).await
                {
                    // A re-file is a brand-new entry under today's key, so it
                    // answers to the same policy a fresh write does. Without
                    // this, an adopted refusal is re-filed into the current key
                    // space and replays forever — the exact thing the write gate
                    // prevents, arriving through a side door. The entry is still
                    // *served*; only the re-file is skipped.
                    if mode == CacheMode::ReadWrite && !policy.stored_entry_is_declined(&entry) {
                        // Re-filed under today's key with today's request, so the
                        // next run hits directly and the probe budget is not
                        // spent again.
                        entry.request = Some(request_to_persist(canonical));
                        if let Err(e) = cache.put(key, &entry).await {
                            tracing::warn!(
                                error = %e,
                                "adopting an entry under an earlier canonical request: write failed"
                            );
                        }
                    }
                    return Ok(hit(provider, entry, state, key, policy));
                }
                // Before paying for this, check whether an older domarinn already
                // answered it under the key shape that has since changed.
                if let Some(mut entry) =
                    adopt_legacy(provider, req, repeat, cache, state, probe_legacy).await
                {
                    // Re-file it under the current key so the next run finds it
                    // directly and the probe budget is not spent again. A write
                    // failure is not fatal: the entry was still served.
                    // Same policy gate as the canonical-adopt path above, for
                    // the same reason: re-filing mints a new immutable entry.
                    if mode == CacheMode::ReadWrite && !policy.stored_entry_is_declined(&entry) {
                        // An adopted entry predates the request being recorded,
                        // so give it one: under the new key the request is what
                        // the entry is *about*, and re-filing without it would
                        // put a ≤0.4-era entry into the new era half-formed.
                        // `provider_fingerprint` is left as written — it is the
                        // provenance of the call that actually happened.
                        entry.request = Some(request_to_persist(canonical));
                        // Same argument as the request: this path knows what it
                        // called, and re-filing is the only chance a ≤0.4-era
                        // entry gets to say so — the store is immutable, so a
                        // later write under this key is a no-op.
                        entry.kind = Some(EntryKind::new(EntryKind::PROVIDER));
                        if let Err(e) = cache.put(key, &entry).await {
                            tracing::warn!(error = %e, "adopting a legacy cache entry: write failed");
                        }
                    }
                    return Ok(hit(provider, entry, state, key, policy));
                }
                if mode == CacheMode::ReadOnlyStrict {
                    return Err(CallFailure::before_any_attempt_for(
                        ErrorClass::CACHE_MISS,
                        format!("cache-only: miss for key {key}"),
                        key,
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
                    // An empty output is a *successful* call, so it arrives here
                    // in the `Ok` arm — which is why the "errors are never
                    // cached" guard, which is structural rather than a
                    // condition, never covered it. Against an immutable store
                    // one transient empty reply from a flaky gateway is then
                    // replayed forever, for everyone sharing the cache.
                    //
                    // Classified here as well as in `runner_cell` deliberately:
                    // both go through the same `EmptyPolicy`, which is a pure
                    // function of the same response, so the two answers cannot
                    // disagree. Threading a classification out through
                    // `CallOutcome` for one caller would be the version that can.
                    //
                    // Grader and embedding writes (`request_cache`) are *not*
                    // gated: a grader response is parsed for a verdict, never
                    // classified for emptiness, and one that no longer parses is
                    // already treated as a miss — so it self-heals.
                    let reason = policy.effective_reason(provider, &response);
                    if policy.should_store(reason.as_ref()) {
                        let entry = response_to_entry(provider, &response, stats, canonical);
                        // A cache write failure must not fail the run.
                        if let Err(e) = cache.put(key, &entry).await {
                            tracing::warn!(error = %e, "cache write failed");
                        }
                    } else {
                        tracing::debug!(
                            %key,
                            reason = %clamp_for_log(reason.as_ref().map(|r| r.as_str()).unwrap_or("")),
                            "not storing: no gradeable output, and `cache.store_empty_outputs` \
                             does not keep this reason"
                        );
                    }
                }
            }
            Ok(CallOutcome {
                response,
                cached: false,
                attempts: Some(stats.attempts),
                provider_latency_ms: Some(stats.in_flight.as_millis() as u64),
                // A miss carries the key too: it is a property of the request,
                // so the entry this call just wrote is the same one a later hit
                // will read. Recording only on hits would make "which runs used
                // this entry" mean "which runs were cache hits".
                cache_key: key,
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
                // A provider error still had a key, and recording it means the
                // failing case links to the entry a later successful run wrote.
                cache_key: key,
            })
        }
    }
}

/// Turn a found entry into an outcome, reporting anything about it that the key
/// deliberately does not cover.
fn hit(
    provider: &dyn Provider,
    entry: CacheEntry,
    state: &CacheRunState,
    key: &CacheKey,
    policy: &crate::empty_policy::EmptyPolicy,
) -> CallOutcome {
    warn_on_program_drift(provider, &entry, state);
    warn_on_address_drift(provider, &entry, state);
    warn_on_declined_replay(provider, &entry, state, key, policy);
    let attempts = entry.attempts;
    let provider_latency_ms = entry.provider_latency_ms;
    CallOutcome {
        response: entry_to_response(entry, provider),
        cached: true,
        attempts,
        provider_latency_ms,
        cache_key: Some(key.clone()),
    }
}

/// Say so, once per provider, when a replayed entry is one this suite would no
/// longer write.
///
/// The write gate stops *new* empties being stored; it cannot touch what is
/// already there, because entries are immutable. So a cache poisoned before the
/// upgrade — or before `cache.store_empty_outputs` was tightened — keeps
/// serving the same empty answer, and nothing in a green-looking run says why.
/// This is the only place that says it, and it names the cure.
fn warn_on_declined_replay(
    provider: &dyn Provider,
    entry: &CacheEntry,
    state: &CacheRunState,
    key: &CacheKey,
    policy: &crate::empty_policy::EmptyPolicy,
) {
    if !policy.stored_entry_is_declined(entry) {
        return;
    }
    let Some(reason) = entry.empty_reason.as_ref() else {
        return;
    };
    if !state.claim_no_store_warning(provider.id()) {
        return;
    }
    // Clamped before it reaches a log line: `EmptyReason` is an open string an
    // `exec` child writes verbatim, so an unclamped one is a newline away from
    // forging log records.
    let reason = clamp_for_log(reason.as_str());
    tracing::warn!(
        provider = %provider.id(),
        %reason,
        %key,
        "replaying a cached empty answer that this suite would no longer store. \
         `cache.store_empty_outputs` only gates new writes, and entries are \
         immutable — so this one replays until it is removed: \
         `domarinn cache gc --empty-reason <reason> --older-than 0s` for a local \
         cache, or `DELETE /api/v1/cache/entries/<key>` on a server."
    );
}

/// Cap a callee-controlled string and strip anything that could forge a log
/// line. Applied to every `empty_reason` that reaches a diagnostic.
fn clamp_for_log(s: &str) -> String {
    const MAX: usize = 64;
    s.chars()
        .filter(|c| !c.is_control())
        .take(MAX)
        .collect::<String>()
}

/// Say so, once per provider, when these answers came from a different endpoint
/// than the one now configured.
///
/// The compensation for keeping `base_url` out of the cache key. Pointing a
/// suite at a gateway must not re-run it, and pointing it at a local stub must
/// not silently replay a vendor's answers; keying the host would fix the second
/// by breaking the first. Recording and reporting fixes the second and keeps the
/// first, which is the same bargain `warn_on_program_drift` strikes for an
/// `exec` provider's bytes.
///
/// Nothing is invalidated. Deciding that a different endpoint answers a
/// different question is the suite's call, and `cache_salt` is how it says so.
fn warn_on_address_drift(provider: &dyn Provider, entry: &CacheEntry, state: &CacheRunState) {
    let (Some(live), Some(stored)) = (provider.address(), entry.address.as_deref()) else {
        // An entry written before the field existed, or a provider with no fixed
        // address. Absence is not evidence of a change.
        return;
    };
    if live == stored || !state.claim_address_warning(provider.id()) {
        return;
    }
    tracing::warn!(
        provider = %provider.id(),
        stored = %stored,
        configured = %live,
        "replaying cached answers that came from a different endpoint. `base_url` is \
         not part of the cache key, so a gateway and a direct connection share \
         entries — which is usually what you want. If these two endpoints answer \
         differently (a local stub standing in for a vendor, say), give them \
         different `cache_salt` values so they stop sharing."
    );
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
/// Look for this request under a *canonical request* an earlier version
/// published.
///
/// The same key function as the live path over an older document, where
/// [`adopt_legacy`] reconstructs a different key function entirely. Tried first
/// because it is the more likely of the two: every store written between 0.5.0
/// and 0.7.x has these, where only a ≤0.4.x store has the other.
async fn adopt_legacy_canonical(
    provider: &dyn Provider,
    req: &ProviderRequest,
    repeat: u32,
    cache: &dyn CacheBackend,
    state: &CacheRunState,
    probe_legacy: bool,
) -> Option<(CacheKey, CacheEntry)> {
    let shapes = provider.legacy_canonical_requests(req);
    if shapes.is_empty() || !probe_legacy || !state.migration.should_probe() {
        return None;
    }
    for canonical in shapes {
        let key = crate::cache_key::request_cache_key(
            &canonical,
            repeat,
            provider.cache_salt(),
            req.case_salt.as_deref(),
        );
        match cache.get(&key).await {
            Ok(Some(entry)) => {
                state.migration.record_adoption();
                tracing::debug!(%key, "adopted an entry under an earlier canonical request");
                return Some((key, entry));
            }
            Ok(None) => continue,
            Err(e) => {
                tracing::debug!(error = %e, %key, "legacy canonical probe failed; ignoring");
                continue;
            }
        }
    }
    None
}

async fn adopt_legacy(
    provider: &dyn Provider,
    req: &ProviderRequest,
    repeat: u32,
    cache: &dyn CacheBackend,
    state: &CacheRunState,
    probe_legacy: bool,
) -> Option<CacheEntry> {
    let legacy = provider.legacy_fingerprints();
    if legacy.is_empty() || !probe_legacy || !state.migration.should_probe() {
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
/// is `body`, `headers_digest` and `output_expr`, for `anthropic` the `headers`
/// member carrying `anthropic-version`, for `exec` the `stdin` and
/// `env_digest`. A slim entry therefore no longer re-hashes to its own key. That
/// is the trade: the key is already stored (it is where the entry lives), so
/// what is lost is offline re-derivation for the rare huge request, and what is
/// kept is every entry staying under the cap.
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
        kind: Some(EntryKind::new(EntryKind::PROVIDER)),
        // The request replaces the fingerprint rather than joining it: the key
        // is derived from the request now, so the fingerprint would be
        // provenance for a key nothing computes. Entries in the wild still carry
        // it, which is why the field survives as `Option`.
        provider_fingerprint: None,
        request: Some(request_to_persist(canonical)),
        // Which build answered — recorded, never keyed. See
        // `warn_on_program_drift` for what reads it back.
        program_digest: provider.program_digest().map(str::to_string),
        // Where it came from — recorded, never keyed. See `warn_on_address_drift`.
        address: provider.address(),
        output: response.output.clone(),
        usage: response.usage.clone(),
        cost_usd: response.cost_usd,
        stop_reason: response.stop_reason.clone(),
        attempts: Some(stats.attempts),
        provider_latency_ms: Some(stats.in_flight.as_millis() as u64),
        reasoning: response.reasoning.clone(),
        // The *classified* reason, not the raw one the provider happened to
        // set. `classify_empty` folds in the `classify_blank` fallback, which is
        // where a blank answer from an `http` provider (which never sets
        // `empty_reason`) or an `exec` child with no `stop_reason` gets its
        // diagnosis. Storing the raw field left those entries with a NULL
        // reason, so `cache gc --empty-reason blank` could not find them, the
        // server's column and facet never saw them, and the once-per-provider
        // replay warning returned early — the operator got neither the
        // diagnostic nor the cure this change exists to provide.
        //
        // Deliberately *not* `EmptyPolicy::effective_reason`: that additionally
        // consults `runner.refusal_patterns`, which is suite configuration.
        // Freezing a config decision into an immutable entry would mean editing
        // a pattern no longer reclassifies what is already stored.
        empty_reason: provider.classify_empty(response),
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
