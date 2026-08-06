//! Walking a provider's `fallback:` chain when the configured one will not
//! answer.
//!
//! A cell names one provider. When that provider refuses, or cannot be reached,
//! this hands the *same* request to the next link and reports whoever answered.
//! `ProviderRequest` carries no provider identity, which is what makes handing
//! it on verbatim safe.
//!
//! Four invariants hold the whole thing up. Each exists because breaking it
//! produces a run that looks fine and is not:
//!
//! 1. **Never walk offline.** Under `--cache-only` there is no live answer to go
//!    and get, so a handoff can only replace a usable (if refused) replay with a
//!    `cache_miss` error. Structural, rather than a property of the trigger
//!    list — which is why it is a mode check and not an `ErrorClass` exclusion.
//! 2. **Never walk for a latency-asserted cell.** Such a cell is forced to
//!    `CacheMode::Disabled`, *not* `ReadOnlyStrict`, so invariant 1 does not
//!    cover it — and `latency_ms` is the *answering* link's in-flight time.
//!    Since `provider_timeout` is a trigger, primary-times-out →
//!    fallback-answers-fast would make `{latency, max: 2000}` pass. "Never
//!    worse" needs a matching "never falsely better".
//! 3. **Report the primary when nothing improved.** If every link is a trigger,
//!    the chain settles on the *primary's* outcome, not the last link's.
//!    Otherwise `provider_digest` churns for zero gain and the run document
//!    diverges from a no-fallback run — which breaks the server's
//!    `(id, content_hash)` re-upload idempotency.
//! 4. **Check the abort flag between links.** The run's "bounded at roughly N
//!    in-flight calls" promise becomes N × chain-length without it.

use serde_json::Value as Json;

use crate::cache::CacheMode;
use crate::empty::EmptyReason;
use crate::empty_policy::EmptyPolicy;
use crate::error_class::ErrorClass;
use crate::provider::{CallCtx, Provider, ProviderRequest};
use crate::result::FallbackAttempt;
use crate::retry::RetryPolicy;

use super::runner_cache::{call_with_cache, CacheCall, CallFailure, CallOutcome};
use super::AbortFlag;

/// The failure classes a fallback answers.
///
/// An explicit list, deliberately **not** [`ErrorClass::is_infrastructure`]:
/// that predicate is prefix-based and also matches `cache_miss` /
/// `cache_unavailable`, and handing off on those is the one thing this must
/// never do — it would turn a `--cache-only` run into a live call.
///
/// `provider_request` is absent too. A 400 for a malformed body is the suite's
/// bug, and the next provider will reject it in exactly the same way; spending
/// a second call to learn that is not resilience, it is noise.
const FALLBACK_ERROR_CLASSES: &[&str] = &[
    ErrorClass::PROVIDER_AUTH,
    ErrorClass::PROVIDER_UNAVAILABLE,
    ErrorClass::PROVIDER_TIMEOUT,
    ErrorClass::PROVIDER_PROTOCOL,
    ErrorClass::PROVIDER_RATE_LIMIT,
    ErrorClass::EXEC_FAILED,
];

/// The empty reasons that make a provider hand off, when the suite says nothing.
///
/// The two that mean "this provider will not answer this", as opposed to "this
/// provider answered badly" — which is a result, and belongs to the grader.
const DEFAULT_FALLBACK_ON_EMPTY_REASON: &[&str] =
    &[EmptyReason::REFUSAL, EmptyReason::CONTENT_FILTER];

/// When a chain hands off, per this run.
#[derive(Debug, Clone)]
pub(super) struct FallbackPolicy {
    on_empty_reason: Vec<String>,
    /// `--no-fallback` / `DOMARINN_NO_FALLBACK`. A run that wants to know its
    /// primary is broken rather than have it papered over.
    enabled: bool,
}

impl Default for FallbackPolicy {
    /// The built-in triggers, enabled. What a suite that says nothing gets.
    fn default() -> Self {
        FallbackPolicy {
            on_empty_reason: DEFAULT_FALLBACK_ON_EMPTY_REASON
                .iter()
                .map(|s| s.to_string())
                .collect(),
            enabled: true,
        }
    }
}

impl FallbackPolicy {
    /// Read the policy off a suite, with the run's kill switch applied.
    ///
    /// `override_reasons` is the CLI flag and its environment variable, which
    /// win over the suite; the suite wins over the built-in default.
    pub(super) fn resolve(
        suite: &crate::config::Suite,
        enabled: bool,
        override_reasons: Option<Vec<String>>,
    ) -> Self {
        let on_empty_reason = override_reasons
            .or_else(|| {
                suite
                    .runner
                    .as_ref()
                    .and_then(|r| r.fallback_on_empty_reason.clone())
            })
            .unwrap_or_else(|| {
                DEFAULT_FALLBACK_ON_EMPTY_REASON
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            });
        FallbackPolicy {
            on_empty_reason,
            enabled,
        }
    }

    /// Whether this outcome is a reason to try the next link, and what to record
    /// about it if so.
    ///
    /// Compared as plain strings on both axes, because `EmptyReason` and
    /// `ErrorClass` are both open by design and this must not become the place
    /// that closes either.
    fn hand_off(
        &self,
        provider: &dyn Provider,
        outcome: &Result<CallOutcome, CallFailure>,
        policy: &EmptyPolicy,
    ) -> Option<FallbackAttempt> {
        match outcome {
            Ok(o) => {
                let reason = policy.effective_reason(provider, &o.response)?;
                self.on_empty_reason
                    .iter()
                    .any(|r| r == reason.as_str())
                    .then(|| FallbackAttempt {
                        provider_id: provider.id().to_string(),
                        empty_reason: Some(reason),
                        error_class: None,
                    })
            }
            Err(f) => FALLBACK_ERROR_CLASSES
                .contains(&f.class.as_str())
                .then(|| FallbackAttempt {
                    provider_id: provider.id().to_string(),
                    empty_reason: None,
                    error_class: Some(f.class.clone()),
                }),
        }
    }
}

/// One link's identity, recomputed per attempt because each names a different
/// model at a different address.
struct Attempt<'a> {
    provider: &'a dyn Provider,
    provider_digest: Option<String>,
    request: Option<Json>,
    outcome: Result<CallOutcome, CallFailure>,
}

/// How a chain settled.
pub(super) struct ChainOutcome<'a> {
    /// Whoever actually answered. Compare against the cell's configured
    /// provider to decide whether this was a fallback at all.
    pub answered_by: &'a dyn Provider,
    pub provider_digest: Option<String>,
    pub request: Option<Json>,
    pub outcome: Result<CallOutcome, CallFailure>,
    /// Links tried and passed over, in configured order. Empty for the
    /// overwhelming majority of cells.
    pub attempted: Vec<FallbackAttempt>,
}

/// Everything a chain walk needs that is not the request itself.
#[derive(Clone, Copy)]
pub(super) struct ChainCtx<'a> {
    pub cache: CacheCall<'a>,
    pub retry_cfg: &'a RetryPolicy,
    pub empty_policy: &'a EmptyPolicy,
    pub fallback: &'a FallbackPolicy,
    pub include_raw: bool,
    pub aborted: &'a AbortFlag,
    /// True when this cell carries a `latency` assertion. See invariant 2.
    pub has_latency_assert: bool,
}

/// Call the primary, then its fallbacks, until one of them answers.
pub(super) async fn call_chain<'a>(
    primary: &'a dyn Provider,
    fallbacks: &[&'a dyn Provider],
    req: &ProviderRequest,
    ctx: &CallCtx,
    chain: ChainCtx<'a>,
) -> ChainOutcome<'a> {
    // Invariants 1 and 2, applied once rather than per link, so "this cell never
    // walks" is a single decision a reader can find.
    let walk = chain.fallback.enabled
        && !fallbacks.is_empty()
        && chain.cache.mode != CacheMode::ReadOnlyStrict
        && !chain.has_latency_assert;

    let links: Vec<&'a dyn Provider> = if walk {
        std::iter::once(primary)
            .chain(fallbacks.iter().copied())
            .collect()
    } else {
        vec![primary]
    };

    let mut attempted: Vec<FallbackAttempt> = Vec::new();
    // Invariant 3: the primary's own outcome, kept so a chain that never
    // improved on it reports it rather than whichever link happened to be last.
    let mut primary_attempt: Option<Attempt<'a>> = None;

    let last = links.len() - 1;
    for (i, provider) in links.iter().copied().enumerate() {
        // Invariant 4. Checked before the call, not after: the point is not to
        // spend the call.
        if i > 0 && chain.aborted.reason().is_some() {
            break;
        }

        let provider_digest = Some(crate::digests::provider_digest(&provider.fingerprint()));
        let request = chain
            .include_raw
            .then(|| provider.request_preview(req))
            .flatten();
        // Only the primary spends the run-global legacy-adoption probe budget;
        // see `CacheCall::probe_legacy` for why this is a flag rather than a
        // second run state.
        let mut cache = chain.cache;
        cache.probe_legacy = i == 0;
        let outcome = call_with_cache(provider, req, ctx, cache, chain.retry_cfg).await;

        let attempt = Attempt {
            provider,
            provider_digest,
            request,
            outcome,
        };

        if i < last {
            if let Some(record) =
                chain
                    .fallback
                    .hand_off(provider, &attempt.outcome, chain.empty_policy)
            {
                tracing::warn!(
                    from = %provider.id(),
                    to = %links[i + 1].id(),
                    reason = %record
                        .empty_reason
                        .as_ref()
                        .map(|r| r.as_str().to_string())
                        .or_else(|| record.error_class.as_ref().map(|c| c.as_str().to_string()))
                        .unwrap_or_default(),
                    "provider did not answer; handing off to its fallback"
                );
                attempted.push(record);
                if i == 0 {
                    primary_attempt = Some(attempt);
                }
                continue;
            }
        }

        // The chain settled here. Invariant 3: if it settled without ever
        // improving on the primary, report the primary. A configured fallback
        // must never make a case worse, or different, than no fallback at all.
        //
        // "Did not improve" is broader than "re-triggered", on two axes.
        //
        // A link can settle on an error that is *not* a trigger —
        // `cache_unavailable` from a failed cache read, or `provider_request`
        // for a 400, deliberately excluded because the next provider would
        // reject the same body identically. Reporting either would turn a
        // gradeable refusal into `CaseStatus::Error`.
        //
        // And it can do that at a *middle* link, not just the last one: with
        // `fallback: [b, c]`, a 400 from `b` settles the chain at `i == 1`.
        // Gating this on `i == last` therefore left the exact regression the
        // invariant exists to rule out reachable through the middle of any
        // chain longer than two. The only condition that matters is that we are
        // past the primary — `i > 0` — because at a middle link `hand_off`
        // returning `Some` is what `continue`d, so reaching here means it did
        // not.
        let never_improved = i > 0
            && (attempt.outcome.is_err()
                || chain
                    .fallback
                    .hand_off(provider, &attempt.outcome, chain.empty_policy)
                    .is_some());
        if never_improved {
            if let Some(p) = primary_attempt {
                tracing::warn!(
                    provider = %p.provider.id(),
                    links = links.len(),
                    "no fallback improved on the configured provider; reporting its own outcome"
                );
                return ChainOutcome {
                    answered_by: p.provider,
                    provider_digest: p.provider_digest,
                    request: p.request,
                    outcome: p.outcome,
                    // Empty, not `skip(1)`. The primary is the reported answer,
                    // so by this type's own definition nothing was passed over —
                    // and `skip(1)` would leave the *middle* links of a chain of
                    // three behind, making the case claim a handoff that the run
                    // summary (keyed on `answered_by_provider_id`) does not
                    // count, and moving the document's content hash away from
                    // what a no-fallback run of the same suite writes.
                    attempted: Vec::new(),
                };
            }
        }
        return ChainOutcome {
            answered_by: attempt.provider,
            provider_digest: attempt.provider_digest,
            request: attempt.request,
            outcome: attempt.outcome,
            attempted,
        };
    }

    // Only reachable when the abort flag was raised mid-chain. The primary's
    // outcome is the honest answer: it is the one thing that did happen.
    match primary_attempt {
        Some(p) => ChainOutcome {
            answered_by: p.provider,
            provider_digest: p.provider_digest,
            request: p.request,
            outcome: p.outcome,
            // Empty for the same reason as the invariant-3 path above.
            attempted: Vec::new(),
        },
        None => unreachable!("the chain always contains the primary, and link 0 never breaks"),
    }
}

/// The chain this cell may walk: configured order, minus anything the test
/// excludes and anything that failed to build.
///
/// Resolved per `(provider, test)` rather than per provider, because
/// `only_providers` / `skip_providers` is a property of the *test* — a test that
/// skips `slow-model` must not reach it through another provider's back door.
///
/// A repeated id is dropped rather than rejected: trying the same provider twice
/// in a row is a no-op, and `validate` already warns about it.
pub(super) fn resolve_chain<'a>(
    ids: &[String],
    by_id: &std::collections::HashMap<&str, &'a dyn Provider>,
    filter: &crate::filter::Filter,
    test: &crate::config::TestCase,
) -> Vec<&'a dyn Provider> {
    let mut seen = std::collections::HashSet::new();
    ids.iter()
        .filter(|id| seen.insert(id.as_str()))
        .filter(|id| filter.test_allows_provider(id, test))
        .filter_map(|id| by_id.get(id.as_str()).copied())
        .collect()
}
