//! The centralized error taxonomy.
//!
//! domarinn's failures were previously described three different ways: strongly
//! typed per-stage enums (`ExecError`, `ResolveError`, `LoadError`, …), a
//! two-variant `ProviderError`, and — in the grader, the one place users hit
//! most — bare `String`s. The strings were the worst of it: "no grader is
//! configured for this suite" and "the grader returned a truncated verdict" are
//! different problems with different owners and different fixes, and a `String`
//! makes them indistinguishable to every caller.
//!
//! This module gives the whole surface one shape:
//!
//! - [`GraderError`] replaces the grader's stringly-typed failures with one
//!   variant per distinct occurrence.
//! - [`Classify`] maps *every* error type in the crate to an
//!   [`ErrorClass`], so the mapping lives in one readable table rather than
//!   being re-derived at each call site.
//!
//! The classes themselves stay in [`crate::error_class`] because they cross the
//! wire — they are persisted on `CaseResult` and read by the server and the web
//! UI. This module is about the *internal* errors that produce them.

use crate::error_class::ErrorClass;

/// Give any error its [`ErrorClass`].
///
/// One trait rather than an inherent method per type, so the taxonomy is a
/// single searchable list: every `impl Classify` below is one row of the table
/// mapping "what went wrong" to "who fixes it".
pub trait Classify {
    fn class(&self) -> ErrorClass;
}

/// A grading failure, by kind.
///
/// Grading fails closed — any variant here becomes an errored assert rather
/// than a failed one, because a grader that did not run has not judged
/// anything. The variants exist to distinguish *whose* problem it is:
/// `Unconfigured` is the suite author's, `Transport` is the provider's, and
/// `TruncatedVerdict` is a settings problem with a specific fix.
#[derive(Debug, thiserror::Error)]
pub enum GraderError {
    /// The suite has no grader for this assert kind. Fix: add a `grader:` block.
    ///
    /// Distinct from every other variant because nothing ran at all — and
    /// because it is the one a first-time user hits, where "grader failed" sent
    /// them looking for a transient fault that does not exist.
    #[error("no grader is configured for '{kind}' assertions in this suite")]
    Unconfigured { kind: &'static str },

    /// A grader provider type that cannot serve this assert kind.
    #[error("grader provider type {provider} does not support {kind}")]
    Unsupported {
        provider: String,
        kind: &'static str,
    },

    /// The grader call itself failed — network, timeout, non-2xx.
    ///
    /// Strictly the *network*. This was once the catch-all for anything that
    /// was not a verdict problem, which put an `exec` checker exiting non-zero
    /// and a `--cache-only` miss under "transport" and then under whatever
    /// class transport happened to carry. Both now have their own variant, so
    /// this one means what it says.
    #[error("grader transport: {0}")]
    Transport(String),

    /// An `exec` assertion's checker program failed — did not spawn, exited
    /// non-zero, or wrote stdout the protocol could not read.
    ///
    /// Its own variant because it is not a grader call at all: no judge was
    /// contacted, so classing it with the grader made a broken checker look
    /// like a broken judge. The checker is the suite author's own program, so
    /// its class is `checker_failed` — a suite fault (exit `2`), like the
    /// unevaluable local assert it is the exec analogue of. It used to share
    /// `exec_failed` with the exec *provider*, which routed a typo'd checker
    /// path to the harness bucket and paged an operator for a suite fix.
    #[error("exec assert failed: {0}")]
    ExecFailed(String),

    /// A `--cache-only` run needed a live call to answer honestly.
    ///
    /// Not a failure of anything: the caller asked for offline replay and the
    /// entry is not there. It is separated from `Transport` so it lands in the
    /// `cache_miss` class a reader can act on — the fix is to warm the cache or
    /// drop `--cache-only`, never to look at the network.
    #[error("{0}")]
    CacheMiss(String),

    /// The grader's credential was rejected (401/403).
    ///
    /// Its own variant because it is the one grader failure that will happen to
    /// *every* remaining case: a rejected credential does not become valid on
    /// the next call. Collapsed into `Transport` it read as a transient fault,
    /// so a whole suite would error one case at a time and exit 3 — an
    /// infrastructure fault, after burning the run's entire provider spend.
    /// The runner short-circuits on this; see `runner::AbortFlag`.
    #[error("grader credential rejected (HTTP {status})")]
    AuthRejected { status: u16 },

    /// The verdict was cut off before it was complete. Fail closed: a truncated
    /// verdict must never be read as a pass. Fix: raise the grader's
    /// `max_tokens`.
    #[error("verdict truncated ({signal}); raise the grader's max_tokens")]
    TruncatedVerdict { signal: &'static str },

    /// A response arrived but carried no usable verdict.
    #[error("grader returned no usable verdict: {0}")]
    InvalidVerdict(String),

    /// The grader is configured in a way that cannot work — e.g. extended
    /// thinking enabled alongside forced tool use.
    #[error("grader misconfigured: {0}")]
    Misconfigured(String),

    /// A local assert reached the grader. A bug, not a user error.
    #[error("internal: {0}")]
    Internal(&'static str),
}

impl GraderError {
    /// Whether asking *the same question again, immediately* could plausibly
    /// produce a different answer.
    ///
    /// This is a narrow question, and the narrowness is the point. It means
    /// "was this answer **sampled**?" — not "might this succeed on a retry in
    /// general". Only a model's own reply qualifies: an `llm-rubric` judge that
    /// returned an object without a usable `pass` was not making a statement
    /// about the request, and a second sample usually parses fine.
    ///
    /// **`Transport` is deliberately excluded.** It looks retryable and is the
    /// obvious thing to include, but the caller re-asks with no delay and no
    /// `Retry-After` handling, so a judge answering `429` would receive three
    /// back-to-back requests per graded assertion instead of one — multiplied
    /// by runner concurrency, that adds load to a service already shedding it.
    /// A transport fault needs backoff, which lives in [`crate::retry`] and
    /// covers the provider path only; wiring the grader through it is the
    /// honest fix and is not this.
    ///
    /// The other `false` arms: `AuthRejected` will fail identically for every
    /// remaining case and already poisons the run; `Unconfigured` /
    /// `Unsupported` / `Misconfigured` are suite bugs no attempt count fixes;
    /// `ExecFailed` is a program that will behave the same way twice;
    /// `CacheMiss` has nothing to re-ask; `Internal` is our own bug.
    ///
    /// `TruncatedVerdict` is included with a caveat: a re-ask at the same
    /// `max_tokens` may well truncate again, and the real fix stays the one the
    /// message names. It is here because truncation can also be a long-reasoning
    /// blip on one sample.
    ///
    /// **A `true` here is necessary but not sufficient.** The budget is set per
    /// call site — see `request_cache::cached_exchange` — because this type
    /// cannot tell an LLM's sampled reply from a deterministic `exec` checker
    /// printing the wrong shape. Both raise `InvalidVerdict`; only one is worth
    /// asking twice.
    pub fn is_retryable(&self) -> bool {
        match self {
            GraderError::InvalidVerdict(_) | GraderError::TruncatedVerdict { .. } => true,
            GraderError::Transport(_)
            | GraderError::ExecFailed(_)
            | GraderError::CacheMiss(_)
            | GraderError::Unconfigured { .. }
            | GraderError::Unsupported { .. }
            | GraderError::Misconfigured(_)
            | GraderError::AuthRejected { .. }
            | GraderError::Internal(_) => false,
        }
    }
}

impl Classify for GraderError {
    fn class(&self) -> ErrorClass {
        ErrorClass::new(match self {
            // The suite is wrong, not the run: a distinct class so it can be
            // filtered out of "the provider is flaky" dashboards.
            GraderError::Unconfigured { .. }
            | GraderError::Unsupported { .. }
            | GraderError::Misconfigured(_) => ErrorClass::GRADER_MISSING,
            // The credential, not the grader — same distinction the provider
            // path draws, so a rejected key does not land in a flakiness graph.
            GraderError::AuthRejected { .. } => ErrorClass::PROVIDER_AUTH,
            // A grader that could not be reached is its own class: it is fixed
            // by waiting or by looking at the network, where the three below
            // are fixed by looking at what the judge actually returned.
            GraderError::Transport(_) => ErrorClass::GRADER_UNAVAILABLE,
            // Not grader classes at all. The checker is the suite's own
            // script, so it collapses with the suite-side faults (exit 2) —
            // distinct from `exec_failed`, the exec *provider* child, which is
            // the harness's problem. A cache miss says an offline run had
            // nothing to replay.
            GraderError::ExecFailed(_) => ErrorClass::CHECKER_FAILED,
            GraderError::CacheMiss(_) => ErrorClass::CACHE_MISS,
            GraderError::TruncatedVerdict { .. }
            | GraderError::InvalidVerdict(_)
            | GraderError::Internal(_) => ErrorClass::GRADER_FAILED,
        })
    }
}

impl Classify for crate::provider::ProviderError {
    /// Already carried on the error itself, set where the evidence existed —
    /// see [`crate::provider::ProviderError`].
    fn class(&self) -> ErrorClass {
        self.class().clone()
    }
}

impl Classify for crate::exec::ExecError {
    /// Every exec failure is the system under test being broken; the model
    /// never saw anything.
    fn class(&self) -> ErrorClass {
        ErrorClass::new(ErrorClass::EXEC_FAILED)
    }
}

impl Classify for crate::cache::CacheError {
    fn class(&self) -> ErrorClass {
        ErrorClass::new(ErrorClass::CACHE_UNAVAILABLE)
    }
}

impl Classify for crate::resolve::ResolveError {
    /// Expanding tests is suite-authoring work, so a failure here is a template
    /// or config fault rather than anything the provider did.
    fn class(&self) -> ErrorClass {
        ErrorClass::new(ErrorClass::RENDER_FAILED)
    }
}

impl Classify for crate::runner::RunError {
    fn class(&self) -> ErrorClass {
        match self {
            crate::runner::RunError::Factory(_) => ErrorClass::new(ErrorClass::PROVIDER_REQUEST),
            crate::runner::RunError::Resolve(e) => e.class(),
            crate::runner::RunError::Generate(_) => ErrorClass::new(ErrorClass::EXEC_FAILED),
            // The suite resolved to nothing. Nothing rendered, nothing was
            // requested, nothing executed — the config is what is wrong.
            crate::runner::RunError::NothingToRun(_) => ErrorClass::new(ErrorClass::RENDER_FAILED),
            crate::runner::RunError::Credentials(_) => ErrorClass::new(ErrorClass::PROVIDER_AUTH),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The distinction that motivated the type: these are different problems
    /// with different owners, and a `String` could not tell them apart.
    ///
    /// `InvalidVerdict` stands in for "the grader failed" here. It used to be
    /// `Transport`, which now carries its own class — see
    /// `an_unreachable_grader_is_classed_apart_from_one_that_answered_badly`.
    #[test]
    fn an_unconfigured_grader_is_not_a_failed_one() {
        let missing = GraderError::Unconfigured { kind: "llm-rubric" };
        let failed = GraderError::InvalidVerdict("verdict missing `pass`".into());
        assert_eq!(missing.class().as_str(), ErrorClass::GRADER_MISSING);
        assert_eq!(failed.class().as_str(), ErrorClass::GRADER_FAILED);
        assert_ne!(missing.class(), failed.class());
    }

    /// A truncated verdict is a grader fault, never a pass — the fail-closed
    /// rule expressed in the type.
    #[test]
    fn a_truncated_verdict_classifies_as_a_grader_failure() {
        let e = GraderError::TruncatedVerdict {
            signal: "finish_reason=length",
        };
        assert_eq!(e.class().as_str(), ErrorClass::GRADER_FAILED);
        assert!(e.to_string().contains("max_tokens"), "{e}");
    }

    /// Grader problems are the suite's or the harness's, never the provider's —
    /// they must not inflate a provider error-rate spike. This holds for the
    /// transport variant too, even though it *is* a machine failing: the run
    /// still learned nothing about the model, which is what the predicate
    /// answers. Its exit code is a separate question — see
    /// `a_grader_transport_fault_still_gates_as_a_broken_harness`.
    #[test]
    fn grader_failures_are_not_infrastructure() {
        for e in [
            GraderError::Unconfigured { kind: "similar" },
            GraderError::Transport("boom".into()),
            GraderError::InvalidVerdict("no content".into()),
        ] {
            assert!(!e.class().is_infrastructure(), "{e}");
        }
    }

    /// The transport variant earns its own class because it routes elsewhere:
    /// waiting or checking the network, rather than reading the judge's reply.
    #[test]
    fn an_unreachable_grader_is_classed_apart_from_one_that_answered_badly() {
        assert_eq!(
            GraderError::Transport("boom".into()).class().as_str(),
            ErrorClass::GRADER_UNAVAILABLE
        );
        assert_eq!(
            GraderError::InvalidVerdict("no content".into())
                .class()
                .as_str(),
            ErrorClass::GRADER_FAILED
        );
    }

    /// Both grader classes still fail the job at the infra code; the split is
    /// about who reads the message, not about letting a run through.
    #[test]
    fn a_grader_transport_fault_still_gates_as_a_broken_harness() {
        use domarinn_types::error_class::GateFault;
        for e in [
            GraderError::Transport("boom".into()),
            GraderError::InvalidVerdict("no content".into()),
        ] {
            assert_eq!(e.class().gate_fault(), GateFault::Harness, "{e}");
        }
        assert_eq!(
            GraderError::Unconfigured { kind: "similar" }
                .class()
                .gate_fault(),
            GateFault::Suite
        );
    }

    /// The axis the retry loop reads. Pinned in both directions because a
    /// wrong `true` here re-asks a judge that will never answer, and a wrong
    /// `false` puts a sampling blip back into the CI-failing bucket this whole
    /// change exists to empty.
    #[test]
    fn only_faults_a_second_ask_could_fix_are_retryable() {
        for e in [
            GraderError::InvalidVerdict("verdict missing `pass`".into()),
            GraderError::TruncatedVerdict {
                signal: "stop_reason=max_tokens",
            },
        ] {
            assert!(e.is_retryable(), "{e} should be retryable");
        }
        for e in [
            GraderError::ExecFailed("exited with 7".into()),
            GraderError::CacheMiss("cache-only: miss".into()),
            GraderError::Unconfigured { kind: "similar" },
            GraderError::Unsupported {
                provider: "p".into(),
                kind: "similar",
            },
            GraderError::Misconfigured("thinking + forced tools".into()),
            GraderError::AuthRejected { status: 401 },
            GraderError::Internal("local assert reached the grader"),
        ] {
            assert!(!e.is_retryable(), "{e} should not be retryable");
        }
    }

    /// A transport fault is the obvious thing to retry and is deliberately not
    /// retried here: this layer re-asks immediately, so a judge shedding load
    /// with `429` would get three back-to-back requests per assertion instead
    /// of one. Backoff lives in `retry`, which the grader path does not use.
    #[test]
    fn a_transport_fault_is_not_re_asked_because_this_layer_has_no_backoff() {
        assert!(!GraderError::Transport("connection reset".into()).is_retryable());
    }

    #[test]
    fn the_message_names_the_assert_kind() {
        let e = GraderError::Unconfigured { kind: "llm-rubric" };
        assert!(e.to_string().contains("llm-rubric"), "{e}");
    }
}
