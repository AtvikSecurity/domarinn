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
    #[error("grader transport: {0}")]
    Transport(String),

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
            GraderError::Transport(_)
            | GraderError::TruncatedVerdict { .. }
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
    #[test]
    fn an_unconfigured_grader_is_not_a_failed_one() {
        let missing = GraderError::Unconfigured { kind: "llm-rubric" };
        let failed = GraderError::Transport("connection reset".into());
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
    /// they must not inflate a provider error-rate spike.
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

    #[test]
    fn the_message_names_the_assert_kind() {
        let e = GraderError::Unconfigured { kind: "llm-rubric" };
        assert!(e.to_string().contains("llm-rubric"), "{e}");
    }
}
