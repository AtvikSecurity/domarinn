//! One cached JSON exchange, for every request a grader makes.
//!
//! The provider path caches a *response*; this caches an *exchange*. They key
//! the same way — [`crate::cache_key::request_cache_key`] over the canonical
//! outgoing request — which is the whole point of 0.5.0: a judge's HTTP call, an
//! embedding, and an `exec` assert's protocol round-trip are requests like any
//! other, and there is no second rule for them. What used to be a parallel key
//! space (a grader *fingerprint* hashed alongside the graded document) is
//! history, frozen in [`crate::cache_migrate`] and read only to adopt what it
//! wrote.
//!
//! # Why an exchange rather than a verdict
//!
//! A verdict is derived from a response. Caching the response and re-deriving
//! means the hit path and the miss path run the same parser — this module takes
//! the parser as an argument precisely so that cannot drift — so a verdict can
//! never be replayed from a shape today's code would reject, and a fix to a
//! parser applies retroactively to everything already stored. It also makes the
//! stored entry legible: `raw` holds what the judge actually said, not a
//! three-field summary of it.
//!
//! # The read contract
//!
//! An entry serves in one of two ways, checked in this order:
//!
//! 1. `verdict: Some(v)` — a ≤0.4.x verdict entry, adopted forward. It has no
//!    `raw` to re-parse: the verdict *is* the value, and it stays servable
//!    forever because this branch exists.
//! 2. `raw: Some(payload)` — the response to re-parse.
//!
//! An entry with neither, or one whose payload no longer parses, is a **miss**,
//! never an error. That is what keeps the two kinds of entry able to share one
//! key space safely: a provider response (no verdict, and `raw` only when the
//! provider reported one) found under a grader key cannot be misread as an
//! answer to the grading. It cannot actually happen — the canonical requests
//! differ — but the cost of being wrong is a wrong verdict, so the read is
//! written not to depend on that. See [`serve`] for why an unparseable payload
//! belongs in the same bucket.

use std::future::Future;

use serde_json::Value as Json;

use crate::cache::{CacheBackend, CacheEntry, CacheKey, CacheMode, EntryKind, Graded};
use crate::cache_adopt::MigrationProbe;
use crate::cache_key::request_cache_key;
use crate::cache_migrate::legacy_grader_verdict_key;
use crate::errors::GraderError;
use crate::runner::runner_cache::request_to_persist;
use crate::types::{Output, TokenUsage};

/// The cache, as one run's grader sees it.
///
/// `None` on [`crate::runner::GradeCtx`] rather than a mode flag in here: a
/// grader-cache-off run must not read *or* write, and an `Option` says that
/// without a fourth [`CacheMode`] that means "off, but differently".
#[derive(Clone, Copy)]
pub(crate) struct RequestCache<'a> {
    pub backend: &'a dyn CacheBackend,
    pub mode: CacheMode,
    /// The trial index, in the key so `--repeat N` still samples the judge N
    /// times: two trials whose provider responses are byte-identical — common
    /// at temperature 0, or with a warm provider cache — would otherwise
    /// collapse to one verdict and erase exactly the variance `--repeat` exists
    /// to measure.
    pub repeat: u32,
    /// Shared with the provider side: one budget, one `--no-cache-migration`.
    /// A store either has ≤0.4.x entries in it or it does not, and spending a
    /// separate budget per key space would probe twice as long to learn the
    /// same thing.
    pub migration: &'a MigrationProbe,
}

/// The ≤0.4.x verdict key's two halves, for a call type that has history.
///
/// Supplied by the caller because deriving them means knowing what a 0.4.x
/// domarinn would have hashed for *this* assert kind — see
/// [`crate::cache_migrate::legacy_grading_fingerprint`]. `similar` supplies
/// nothing and is deliberately not adopted.
pub(crate) struct LegacyVerdict {
    pub fingerprint: Json,
    pub graded: Json,
}

/// One cached exchange, as its caller describes it.
pub(crate) struct Exchange<'a> {
    /// The canonical outgoing request — redacted, and exactly what the key
    /// hashes. Credentials live in headers and are structurally absent from the
    /// envelope shapes ([`crate::provider::http_request_preview`],
    /// [`crate::provider::exec_request_preview`]).
    pub canonical: Json,
    /// What kind of call this is.
    ///
    /// Here rather than on [`EntryMeta`] because a kind is a property of the
    /// *call*, known before the cache is consulted — where `EntryMeta` describes
    /// a fresh payload and is produced by a closure that runs only on the miss
    /// path. The legacy-adoption path needs it on a hit, so it has to be here.
    pub kind: EntryKind,
    /// The assert's own `cache_salt`, when it has one.
    pub case_salt: Option<&'a str>,
    /// The `cache_salt` of the `providers:` entry behind this call, when there
    /// is one. An embeddings provider and a judge are both declared entries, so
    /// both can carry a version pin the same way a provider call does.
    pub provider_salt: Option<&'a str>,
    /// What to probe on a miss, for a call type with a ≤0.4.x key space.
    pub legacy: Option<LegacyVerdict>,
    /// Canonical requests this call published in earlier versions, newest first.
    ///
    /// The same key function over an older document — unlike [`Self::legacy`],
    /// which reconstructs a different key function entirely. 0.8.0 populates it
    /// for the vendor-backed calls, whose canonical request used to carry a full
    /// `base_url` in a `url` member and now carries only the `path`. See
    /// [`crate::cache_adopt`].
    pub legacy_canonical: Vec<Json>,
}

/// What a fresh payload is recorded as, beyond the payload itself.
///
/// The caller derives these from its own parse: only it knows that a judge's
/// reasoning is the human-readable `output` and that an embedding's is its
/// dimension count.
pub(crate) struct EntryMeta {
    pub output: Output,
    pub usage: Option<TokenUsage>,
    pub cost_usd: Option<f64>,
    pub model: Option<String>,
}

/// What the cache served.
///
/// `Verdict` is boxed because it is the rare arm and much the larger: a
/// [`Graded`] carries a verdict, token counts and a model name, where a `Parsed`
/// is a handful of words. Adoption happens on a ≤0.4.x store and nowhere else,
/// so the common path should not pay for its size on every exchange.
pub(crate) enum Served<T> {
    /// A ≤0.4.x verdict, adopted whole. There is no payload behind it, so
    /// nothing was parsed and nothing more can be derived.
    Verdict(Box<Graded>),
    /// A parsed exchange, live or replayed.
    Parsed(Parsed<T>),
}

/// A parsed exchange.
pub(crate) struct Parsed<T> {
    pub value: T,
    /// True when the payload came from the store rather than the wire.
    pub cached: bool,
    /// The cost the entry recorded, on a hit.
    ///
    /// A fallback, not the answer: cost is re-derived from the replayed payload
    /// at the *current* rate, for the same reason
    /// [`crate::runner::runner_cache::entry_to_response`] re-prices a provider
    /// hit — a warm suite must not report last year's prices forever. This is
    /// what is left when re-pricing yields nothing (an unknown model today,
    /// priced when the call was made).
    pub stored_cost_usd: Option<f64>,
}

impl<T> Served<T> {
    /// Unwrap the parse, for a call type that supplied no legacy ingredients.
    ///
    /// [`Served::Verdict`] is reachable only when [`Exchange::legacy`] was
    /// `Some`, so this is not a fallible conversion in practice — but a caller
    /// that grows an adoption path later should get a compile error at the
    /// `match`, not silence, which is why it is a method rather than an
    /// `unwrap`.
    pub(crate) fn parsed_only(self) -> Result<Parsed<T>, GraderError> {
        match self {
            Served::Parsed(parsed) => Ok(parsed),
            Served::Verdict(_) => Err(GraderError::Internal(
                "a verdict was adopted for a call type with no legacy key space",
            )),
        }
    }
}

/// Run one cached JSON exchange.
///
/// `cache` is `None` for a run with grader caching off: the live future runs,
/// nothing is read, nothing is written. Everything else follows the read
/// contract in the module docs.
///
/// The three closures are what keep this generic without it guessing:
///
/// - `parse` turns a payload into the caller's value, and runs on **both** the
///   hit and miss paths. Taking it here rather than leaving it to the caller is
///   the mechanism that makes "a replay parses like a live call" structural.
/// - `meta` describes a *fresh* payload for storage. It sees the parsed value,
///   so an entry's `output`/`usage`/`cost` come from the same reading of the
///   response the verdict did.
/// - `strict_miss` builds the error a `--cache-only` miss deserves, in the
///   caller's own words: "which judge, missing what" is more use than a key.
///
/// `live` is a **factory** rather than a future so the call can be made more
/// than once, and `reasks` says how many extra times this particular caller may
/// use it — see [`ask_live`].
///
/// `reasks` is per call site rather than read off the error because
/// [`GraderError`] cannot tell the two apart: an `llm-rubric` judge that
/// sampled a bad object and an `exec` checker that deterministically printed
/// the wrong shape both raise `InvalidVerdict`. Only the judge is worth asking
/// again; re-running the checker would execute somebody's program three times
/// for one assertion.
pub(crate) async fn cached_exchange<T, F, Fut, P, M>(
    cache: Option<&RequestCache<'_>>,
    exchange: Exchange<'_>,
    parse: P,
    meta: M,
    strict_miss: impl FnOnce(&CacheKey) -> GraderError,
    reasks: u32,
    live: F,
) -> Result<Served<T>, GraderError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<Json, GraderError>>,
    P: Fn(&Json) -> Result<T, GraderError>,
    M: FnOnce(&T) -> EntryMeta,
{
    let Some(cache) = cache else {
        let (_, value) = ask_live(&live, &parse, reasks).await?;
        return Ok(Served::Parsed(Parsed {
            value,
            cached: false,
            stored_cost_usd: None,
        }));
    };

    let key = request_cache_key(
        &exchange.canonical,
        cache.repeat,
        // `None` for an `exec` assert's grading, whose child is named by the
        // request itself with no `providers:` entry behind it to pin. An
        // embeddings call and a judge do have one, and pass its salt. Inserted
        // only when set, so a call that has none keys exactly as before.
        exchange.provider_salt,
        exchange.case_salt,
    );

    match cache.backend.get(&key).await {
        Ok(Some(entry)) => {
            if let Some(served) = serve(&key, entry, &parse) {
                return Ok(served);
            }
        }
        Ok(None) => tracing::debug!(%key, "grader cache miss"),
        // A cache that cannot be read is not a reason to fail the run: fall
        // through to the live call, exactly as the pre-0.5.0 grader path did.
        // `--cache-only` still fails, below, because it has no live call.
        Err(e) => tracing::warn!(error = %e, "grader cache read failed; grading live"),
    }

    // An entry written under an earlier *canonical request* — same key function,
    // older document. Tried first because it is the cheaper and more likely of
    // the two: it adopts a whole payload rather than a bare verdict, and any
    // store written by 0.5.0 through 0.7.x has these.
    if let Some(entry) = adopt_legacy_canonical(cache, &exchange).await {
        if let Some(served) = serve(&key, entry.clone(), &parse) {
            if cache.mode == CacheMode::ReadWrite {
                let mut refiled = entry;
                // Re-filed under today's key with today's request, so the next
                // run hits directly and the probe budget is not spent again.
                refiled.request = Some(request_to_persist(&exchange.canonical));
                if let Err(e) = cache.backend.put(&key, &refiled).await {
                    tracing::warn!(error = %e, "adopting an earlier canonical request: write failed");
                }
            }
            return Ok(served);
        }
    }

    // Before paying for this, check whether an older domarinn already answered
    // it under the key shape that has since changed.
    if let Some(mut entry) = adopt_legacy(cache, &exchange).await {
        if cache.mode == CacheMode::ReadWrite {
            // Re-filed as it stands: `verdict` stays `Some` and `raw` stays
            // `None`, because there is no payload to invent and the read
            // contract serves a verdict-only entry forever. What it gains is the
            // request it answers — under the new key that is what an entry is
            // *about*, and re-filing without it would put a ≤0.4-era entry into
            // the new era half-formed.
            entry.request = Some(request_to_persist(&exchange.canonical));
            // ...and the kind, for the same reason. A ≤0.4.x entry's `verdict`
            // does say which grader wrote it, but only a reader that knows to
            // look; stamping it here means every path agrees on one field.
            entry.kind = Some(exchange.kind.clone());
            if let Err(e) = cache.backend.put(&key, &entry).await {
                tracing::warn!(error = %e, "adopting a legacy verdict entry: write failed");
            }
        }
        return Ok(Served::Verdict(Box::new(Graded {
            verdict: entry
                .verdict
                .expect("adopt_legacy returns only entries with a verdict"),
            usage: entry.usage,
            cost_usd: entry.cost_usd,
            model: entry.model,
            cached: true,
        })));
    }

    if cache.mode == CacheMode::ReadOnlyStrict {
        // A `--cache-only` run that reached a live judge would be lying about
        // being offline.
        return Err(strict_miss(&key));
    }

    let (payload, value) = ask_live(&live, &parse, reasks).await?;
    if cache.mode == CacheMode::ReadWrite {
        let entry = fresh_entry(
            &exchange.canonical,
            exchange.kind.clone(),
            payload,
            meta(&value),
        );
        // A cache write failure must not fail the run.
        if let Err(e) = cache.backend.put(&key, &entry).await {
            tracing::warn!(error = %e, "grader cache write failed");
        }
    }
    Ok(Served::Parsed(Parsed {
        value,
        cached: false,
        stored_cost_usd: None,
    }))
}

/// How many times a judge may be asked again after an answer that could not be
/// used.
///
/// Two, so a run survives a pair of consecutive bad samples without a
/// pathological case multiplying the grading bill by more than three.
///
/// Only the `llm-rubric` judge passes this; the `exec`-assert and embeddings
/// exchanges pass `0`. Not configurable through `runner.retries`, which governs
/// the provider path — the two are different mechanisms (see [`crate::retry`])
/// and sharing a knob would imply a backoff this has not got.
pub(crate) const MAX_REASKS: u32 = 2;

/// Make the live call and parse it, re-asking up to `reasks` more times when
/// the failure is one a second *sample* could plausibly fix.
///
/// A judge's verdict is sampled. Unlike a provider 400, "the model returned an
/// object without a usable `pass`" is not a fact about the request — the same
/// request a moment later usually produces a fine verdict. Before this existed
/// there was no retry on the grader path at all (`with_retry` reaches only the
/// provider call), so a single malformed sample errored a case permanently and
/// took the whole CI job to exit 3 with it.
///
/// Two guards, and both are needed. [`GraderError::is_retryable`] excludes the
/// faults a second ask cannot fix; `reasks` excludes the *callers* for whom no
/// fault is worth re-asking, because the error type alone cannot tell a
/// sampled reply from a deterministic program's output.
///
/// Parsing is inside the loop deliberately: the failure being retried happens
/// *after* the bytes arrive, so retrying only the call would re-parse the same
/// reply and fail identically.
///
/// Nothing is written to the cache on the failing attempts — a payload that
/// does not parse can never be stored, because the entry's metadata is derived
/// from the parsed value. That also means a discarded attempt's tokens never
/// reach the run summary, so a re-asked case under-reports its grading cost by
/// whatever the failed samples billed. Bounded by `MAX_REASKS` and only
/// reachable on an unusable verdict, so the gap is small, but it is real.
async fn ask_live<T, F, Fut, P>(live: &F, parse: &P, reasks: u32) -> Result<(Json, T), GraderError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<Json, GraderError>>,
    P: Fn(&Json) -> Result<T, GraderError>,
{
    let mut attempt = 0;
    loop {
        let outcome = match live().await {
            Ok(payload) => match parse(&payload) {
                Ok(value) => return Ok((payload, value)),
                Err(e) => e,
            },
            Err(e) => e,
        };
        if attempt >= reasks || !outcome.is_retryable() {
            return Err(outcome);
        }
        attempt += 1;
        tracing::warn!(
            error = %outcome,
            attempt,
            max = reasks,
            "grader did not return a usable verdict; asking again"
        );
    }
}

/// Apply the read contract to a found entry. `None` is a miss.
///
/// **A stored payload today's parser rejects is a miss, not an error**, and the
/// distinction matters because entries are immutable: a hard error here could
/// never be written past, so one incompatible entry would fail the same
/// assertion on every future run and the only remedy would be purging a store
/// the message never mentioned. Falling through re-asks the judge and gets a
/// real answer, which is not a silent pass — fail-closed is about never
/// *inventing* a verdict, and nothing is invented here. The cost is that the
/// stale entry keeps losing the race on every run, so it is logged at `warn`
/// rather than swallowed. Offline there is no fallthrough and `--cache-only`
/// reports its strict miss, which is the honest answer there.
fn serve<T>(
    key: &CacheKey,
    entry: CacheEntry,
    parse: &impl Fn(&Json) -> Result<T, GraderError>,
) -> Option<Served<T>> {
    if let Some(verdict) = entry.verdict {
        tracing::debug!(%key, "grader cache hit (adopted verdict)");
        return Some(Served::Verdict(Box::new(Graded {
            verdict,
            usage: entry.usage,
            cost_usd: entry.cost_usd,
            model: entry.model,
            cached: true,
        })));
    }
    let Some(payload) = entry.raw else {
        tracing::debug!(%key, "grader cache entry carries neither a verdict nor a payload");
        return None;
    };
    match parse(&payload) {
        Ok(value) => {
            tracing::debug!(%key, "grader cache hit");
            Some(Served::Parsed(Parsed {
                value,
                cached: true,
                stored_cost_usd: entry.cost_usd,
            }))
        }
        Err(e) => {
            tracing::warn!(
                error = %e, %key, written_by = %entry.domarinn_version,
                "a cached grader payload no longer parses; re-asking. The entry is \
                 immutable, so this repeats until the store is purged of it."
            );
            None
        }
    }
}

/// Look for this grading under the verdict key shape domarinn used to publish.
///
/// Returns only entries that actually carry a verdict: the legacy key space held
/// nothing else, so an entry without one is a hash collision or a corrupted
/// store, and neither is something to serve.
///
/// A read error is swallowed rather than propagated — this is a bonus lookup on
/// a key that already missed, so failing the grading over it would turn an
/// optimisation into an outage. Runs in `--cache-only` too, where it earns the
/// most: a run with no live call to fall back on would otherwise report a miss
/// over an answer the store already holds.
/// Look for this call's entry under a canonical request an earlier version
/// published.
///
/// The same key function as the live path, over an older document — so unlike
/// [`adopt_legacy`] there is nothing frozen to reconstruct, and what comes back
/// is a whole entry rather than a bare verdict. Spends the shared
/// [`crate::cache_adopt::MigrationProbe`] budget, so a store with nothing to
/// migrate pays a handful of lookups once and then stops.
async fn adopt_legacy_canonical(
    cache: &RequestCache<'_>,
    exchange: &Exchange<'_>,
) -> Option<CacheEntry> {
    if exchange.legacy_canonical.is_empty() || !cache.migration.should_probe() {
        return None;
    }
    for canonical in &exchange.legacy_canonical {
        let key = request_cache_key(
            canonical,
            cache.repeat,
            exchange.provider_salt,
            exchange.case_salt,
        );
        match cache.backend.get(&key).await {
            Ok(Some(entry)) => {
                tracing::debug!(%key, "adopted an entry under an earlier canonical request");
                cache.migration.record_adoption();
                return Some(entry);
            }
            Ok(None) => {}
            Err(e) => tracing::debug!(error = %e, %key, "legacy canonical probe failed; ignoring"),
        }
    }
    None
}

async fn adopt_legacy(cache: &RequestCache<'_>, exchange: &Exchange<'_>) -> Option<CacheEntry> {
    let legacy = exchange.legacy.as_ref()?;
    if !cache.migration.should_probe() {
        return None;
    }
    let key = legacy_grader_verdict_key(&legacy.fingerprint, &legacy.graded, cache.repeat);
    match cache.backend.get(&key).await {
        Ok(Some(entry)) if entry.verdict.is_some() => {
            cache.migration.record_adoption();
            tracing::debug!(%key, "adopted a verdict written under a previous key shape");
            Some(entry)
        }
        Ok(_) => None,
        Err(e) => {
            tracing::debug!(error = %e, %key, "legacy verdict probe failed; ignoring");
            None
        }
    }
}

/// An entry for a payload this run paid for.
///
/// `raw` carries the payload **unbounded**, unlike a provider entry's `raw`.
/// There the payload is metadata riding alongside an `output` that is stored in
/// full anyway, so a cap costs a debugging drawer; here it *is* the value, and a
/// capped entry would be one that can never serve — a permanent miss written to
/// a shared store on every run. An embedding of a large model is tens of
/// kilobytes of floats, and that is the entry working, not a pathology.
///
/// The real boundary is the store's, not this function's: a remote backend
/// rejects an oversized entry with 413 against `DOMARINN_CACHE_MAX_ENTRY_BYTES`
/// (4 MiB by default), which the write path below logs and continues past — so
/// a payload over that limit is re-paid on every run rather than failing one.
/// That is the right place for the limit, because it is the only place that
/// knows what the store will accept.
fn fresh_entry(canonical: &Json, kind: EntryKind, payload: Json, meta: EntryMeta) -> CacheEntry {
    CacheEntry {
        created_at: chrono::Utc::now(),
        kind: Some(kind),
        // The request replaces the fingerprint, exactly as on the provider side.
        provider_fingerprint: None,
        request: Some(request_to_persist(canonical)),
        output: meta.output,
        usage: meta.usage,
        cost_usd: meta.cost_usd,
        model: meta.model,
        raw: Some(payload),
        stop_reason: None,
        // Grader calls do not go through `with_retry`, so there is no attempt
        // count or in-flight measurement to replay, and `0`/`Some(1)` would both
        // be claims this path cannot make.
        attempts: None,
        provider_latency_ms: None,
        reasoning: None,
        empty_reason: None,
        // No single program to digest: a judge is answered over the network, and
        // an `exec` assert's child is named by `command` in the request itself.
        program_digest: None,
        address: None,
        // A verdict entry is what a ≤0.4.x store held; entries written from here
        // carry the payload the verdict is re-derived from instead.
        verdict: None,
        tool_calls: Vec::new(),
        domarinn_version: crate::VERSION.to_string(),
    }
}
