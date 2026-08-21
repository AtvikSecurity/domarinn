//! What kind of failure a case hit.
//!
//! `CaseResult.error` is prose — the exact message a human needs, and useless
//! for aggregation. Without a class, a run reporting "14 errors" makes you open
//! fourteen cases to learn that none of them were about the model.
//!
//! ## Open by construction
//!
//! An open string newtype rather than an enum, for the same reasons as
//! [`crate::empty::EmptyReason`], and one of its own. A class crosses three
//! boundaries where an unknown value must not be a hard failure: the stored
//! `CaseResult` blob, the server's ingest, and — decisively — the exec protocol,
//! where `ProtocolError` is written by a child process domarinn did not compile.
//! A closed enum would reject a value from a newer child. `#[serde(other)]` is
//! not an escape hatch either: it would round-trip an unknown value to the
//! catch-all variant and break byte-stable re-serialization.
//!
//! ## What earns a constant
//!
//! One test: **does it route to a different owner, or a different fix?**
//! `provider_rate_limit` and `provider_unavailable` are both "the provider had a
//! bad day", but one is fixed by lowering concurrency and the other by waiting,
//! so they are separate. A 404 and a 400 are both "fix your suite", so they are
//! one class.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// What kind of failure produced a case's error. Open — see the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(transparent)]
#[ts(type = "string")]
pub struct ErrorClass(pub String);

impl ErrorClass {
    /// A 4xx that is not auth or rate limiting — bad model id, bad params,
    /// malformed request. Fix the suite.
    pub const PROVIDER_REQUEST: &'static str = "provider_request";
    /// 401/403, or a missing API-key environment variable. Fix the secret.
    pub const PROVIDER_AUTH: &'static str = "provider_auth";
    /// 429, still failing after the retry budget. Lower concurrency or raise
    /// `runner.retries`. Neither the model's fault nor the suite's.
    pub const PROVIDER_RATE_LIMIT: &'static str = "provider_rate_limit";
    /// 5xx after retries — the provider is having a bad day.
    pub const PROVIDER_UNAVAILABLE: &'static str = "provider_unavailable";
    /// Timeout or connection failure after retries.
    pub const PROVIDER_TIMEOUT: &'static str = "provider_timeout";
    /// A 2xx whose body could not be parsed into a response.
    pub const PROVIDER_PROTOCOL: &'static str = "provider_protocol";
    /// An exec child failed to spawn, exited nonzero, or wrote unparseable
    /// stdout.
    ///
    /// Covers both exec children: an `exec` **provider**, where the system
    /// under test is broken and the model never saw anything, and an `exec`
    /// **assertion**, where the checker program broke and nothing was graded.
    /// Either way the case errored rather than failed — there is no verdict to
    /// read, only a process that did not work.
    pub const EXEC_FAILED: &'static str = "exec_failed";
    /// Rendering vars or the prompt raised — a template bug in the suite.
    pub const RENDER_FAILED: &'static str = "render_failed";
    /// The grader itself failed. The eval did not run; do not read the score.
    pub const GRADER_FAILED: &'static str = "grader_failed";
    /// The grader could not be reached at all — a transport or network fault.
    ///
    /// Split from [`Self::GRADER_FAILED`] on this module's own test: both mean
    /// the eval did not run, but one is fixed by waiting or by looking at the
    /// network and the other by looking at the judge's output. Merged, a
    /// dashboard cannot tell a grader outage from a judge that answered badly.
    pub const GRADER_UNAVAILABLE: &'static str = "grader_unavailable";
    /// A deferred assert with no grader configured for its kind.
    pub const GRADER_MISSING: &'static str = "grader_missing";
    /// A local assert blew up while evaluating — a bad regex, an uncompilable
    /// schema. The suite's bug.
    ///
    /// **Not** an `exec` assertion whose checker exited non-zero: that is
    /// [`Self::EXEC_FAILED`], because the child broke rather than the
    /// assertion being unevaluable. This doc used to claim otherwise and never
    /// matched what the runner stored.
    pub const ASSERT_FAILED: &'static str = "assert_failed";
    /// `--cache-only` and the entry was not there. A workflow problem.
    pub const CACHE_MISS: &'static str = "cache_miss";
    /// The cache backend errored on read. An infrastructure problem.
    pub const CACHE_UNAVAILABLE: &'static str = "cache_unavailable";

    pub fn new(value: impl Into<String>) -> Self {
        ErrorClass(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is an infrastructure fault rather than a judgement about
    /// the system under test.
    ///
    /// **This is not the exit-code contract** — see [`Self::gate_fault`] for
    /// that. The docstring here used to claim it was, which was wrong in both
    /// directions: `grader_failed` is excluded here but exits `3`, and the
    /// prefix match covers `cache_miss`, which is a workflow problem rather
    /// than a broken machine. What this answers is narrower and is what an
    /// error-rate dashboard wants: "was anything about the model or the suite
    /// implicated, or did a machine simply fail?"
    ///
    /// Deliberately excludes every `grader_*` class. A grader fault means the
    /// eval did not run and the scores beside it are not evidence — that is a
    /// judgement-shaped problem for a reader even when its cause is a network
    /// blip, which is why the UI tones them like failures rather than like
    /// retries. [`crate::error_class`] callers wanting the CI gate's view of
    /// the same class must use [`Self::gate_fault`].
    pub fn is_infrastructure(&self) -> bool {
        self.0.starts_with("provider_")
            || self.0.starts_with("cache_")
            || self.0 == Self::EXEC_FAILED
    }

    /// Which exit code an errored case of this class drives — **the** CI
    /// exit-code contract, expressed once.
    ///
    /// Before this existed the ladder in `run` treated every errored cell as
    /// exit `3`, so "the grader returned malformed JSON" and "your suite names
    /// a grader that does not exist" reached CI as the same sentence. They
    /// route to different people, so they get different codes.
    ///
    /// Both variants still fail the job — [`GateFault::Suite`] maps to exit
    /// `2`, which the shipped action gates on unconditionally. This split makes
    /// a red run *legible*; it never makes one green.
    ///
    /// An unrecognized class is [`GateFault::Harness`]. The type is open by
    /// construction (a newer `exec` child can name a class this build has never
    /// heard of), and failing closed keeps an unknown fault at the code it has
    /// always had rather than quietly downgrading it.
    pub fn gate_fault(&self) -> GateFault {
        // Enumerated on the `Suite` side only. Everything else — every
        // `provider_*`, every `cache_*`, `exec_failed`, both grader faults, and
        // any class from a newer child — is `Harness`, which is also the
        // fail-closed default described above. Listing the `Harness` members
        // explicitly would be three arms returning the same value and would
        // quietly stop covering a class the day someone adds one.
        match self.0.as_str() {
            Self::GRADER_MISSING | Self::RENDER_FAILED | Self::ASSERT_FAILED => GateFault::Suite,
            _ => GateFault::Harness,
        }
    }
}

/// Who has to act on an errored case, and therefore which exit code it drives.
///
/// `Ord` is the tie-break for a run carrying several errored cases: a run that
/// broke *and* has a bad suite reports as broken, because the suite's verdict
/// cannot be trusted until the harness is working. [`GateFault::Harness`] is
/// therefore the greater value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GateFault {
    /// The suite is wrong — a missing grader, a template that will not render,
    /// an assert that cannot be evaluated. Exit `2`. Fix the config.
    Suite,
    /// The harness broke — a provider, the cache, an `exec` child, or the
    /// grader. Exit `3`. Retry or page an operator; not the PR's fault.
    Harness,
}

impl From<&str> for ErrorClass {
    fn from(value: &str) -> Self {
        ErrorClass(value.to_string())
    }
}

impl std::fmt::Display for ErrorClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Most specific first, for the one site that has several candidates: a case
/// whose asserts errored for different reasons.
///
/// A missing grader explains every downstream failure, so it outranks a grader
/// that could not be reached, which outranks a grader that answered
/// unusably, which in turn outranks a local assert blowing up.
pub const PRECEDENCE: &[&str] = &[
    ErrorClass::GRADER_MISSING,
    ErrorClass::GRADER_UNAVAILABLE,
    ErrorClass::GRADER_FAILED,
    ErrorClass::ASSERT_FAILED,
];

/// Pick the most specific of several candidate classes.
pub fn most_specific(candidates: &[ErrorClass]) -> Option<ErrorClass> {
    PRECEDENCE
        .iter()
        .find_map(|want| candidates.iter().find(|c| c.as_str() == *want).cloned())
        // An unrecognized (future) class still beats reporting nothing.
        .or_else(|| candidates.first().cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The open newtype's whole purpose: a value from a newer client, or from an
    /// `exec` child domarinn did not compile, must survive a round trip rather
    /// than failing ingest.
    #[test]
    fn an_unknown_class_round_trips_unchanged() {
        let json = "\"something_invented_next_year\"";
        let parsed: ErrorClass = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.as_str(), "something_invented_next_year");
        assert_eq!(serde_json::to_string(&parsed).unwrap(), json);
    }

    #[test]
    fn it_serializes_as_a_bare_string_not_an_object() {
        let c = ErrorClass::new(ErrorClass::PROVIDER_TIMEOUT);
        assert_eq!(serde_json::to_string(&c).unwrap(), "\"provider_timeout\"");
    }

    /// A dashboard's question: did a machine fail, or was the model or the
    /// suite implicated? Every `grader_*` class sits on the judgement side even
    /// when its cause is a network blip, because the scores beside it are not
    /// evidence either way — the CI gate takes the other view of the same
    /// class, which is what [`ErrorClass::gate_fault`] is for.
    #[test]
    fn infrastructure_classes_are_separable_from_judgement_ones() {
        for infra in [
            ErrorClass::PROVIDER_RATE_LIMIT,
            ErrorClass::PROVIDER_TIMEOUT,
            ErrorClass::CACHE_MISS,
            ErrorClass::EXEC_FAILED,
        ] {
            assert!(
                ErrorClass::new(infra).is_infrastructure(),
                "{infra} should be infrastructure"
            );
        }
        for other in [
            ErrorClass::GRADER_FAILED,
            ErrorClass::GRADER_UNAVAILABLE,
            ErrorClass::GRADER_MISSING,
            ErrorClass::RENDER_FAILED,
            ErrorClass::ASSERT_FAILED,
        ] {
            assert!(
                !ErrorClass::new(other).is_infrastructure(),
                "{other} should not be infrastructure"
            );
        }
    }

    /// The exit-code contract, both directions. `Suite` is exit `2` and
    /// `Harness` is exit `3`; the point of the split is that the two reach
    /// different people, not that either one lets a run through.
    #[test]
    fn the_gate_separates_a_broken_harness_from_a_broken_suite() {
        for harness in [
            ErrorClass::PROVIDER_RATE_LIMIT,
            ErrorClass::PROVIDER_AUTH,
            ErrorClass::PROVIDER_TIMEOUT,
            ErrorClass::PROVIDER_UNAVAILABLE,
            ErrorClass::PROVIDER_PROTOCOL,
            ErrorClass::PROVIDER_REQUEST,
            ErrorClass::CACHE_MISS,
            ErrorClass::CACHE_UNAVAILABLE,
            ErrorClass::EXEC_FAILED,
            ErrorClass::GRADER_FAILED,
            ErrorClass::GRADER_UNAVAILABLE,
        ] {
            assert_eq!(
                ErrorClass::new(harness).gate_fault(),
                GateFault::Harness,
                "{harness} should be a harness fault (exit 3)"
            );
        }
        for suite in [
            ErrorClass::GRADER_MISSING,
            ErrorClass::RENDER_FAILED,
            ErrorClass::ASSERT_FAILED,
        ] {
            assert_eq!(
                ErrorClass::new(suite).gate_fault(),
                GateFault::Suite,
                "{suite} should be a suite fault (exit 2)"
            );
        }
    }

    /// The type is open, so a class from a newer `exec` child reaches this
    /// build as a string it has never seen. It must keep the exit code such a
    /// case has always had rather than be downgraded to the suite's bucket.
    #[test]
    fn an_unknown_class_fails_closed_to_the_harness_bucket() {
        assert_eq!(
            ErrorClass::new("something_invented_next_year").gate_fault(),
            GateFault::Harness
        );
    }

    /// Pinned because the ladder in `run` takes the max across a run's errored
    /// cases: a run that is both broken and misconfigured must report as
    /// broken, since the suite's verdict means nothing until the harness works.
    #[test]
    fn a_harness_fault_outranks_a_suite_fault() {
        assert!(GateFault::Harness > GateFault::Suite);
        assert_eq!(
            [GateFault::Suite, GateFault::Harness, GateFault::Suite]
                .into_iter()
                .max(),
            Some(GateFault::Harness)
        );
    }

    /// The two predicates answer different questions about the same class, and
    /// conflating them is the bug this split exists to prevent: a grader fault
    /// is not infrastructure, and still exits 3.
    #[test]
    fn a_grader_fault_is_a_harness_gate_fault_without_being_infrastructure() {
        for grader in [ErrorClass::GRADER_FAILED, ErrorClass::GRADER_UNAVAILABLE] {
            let c = ErrorClass::new(grader);
            assert!(!c.is_infrastructure(), "{grader}");
            assert_eq!(c.gate_fault(), GateFault::Harness, "{grader}");
        }
    }

    #[test]
    fn precedence_prefers_the_explanation_over_the_symptom() {
        let candidates = [
            ErrorClass::new(ErrorClass::ASSERT_FAILED),
            ErrorClass::new(ErrorClass::GRADER_MISSING),
            ErrorClass::new(ErrorClass::GRADER_FAILED),
        ];
        assert_eq!(
            most_specific(&candidates).unwrap().as_str(),
            ErrorClass::GRADER_MISSING
        );
    }

    /// A class nobody has ranked is still better than silence.
    #[test]
    fn an_unranked_class_is_reported_rather_than_dropped() {
        let candidates = [ErrorClass::new("from_a_newer_exec_child")];
        assert_eq!(
            most_specific(&candidates).unwrap().as_str(),
            "from_a_newer_exec_child"
        );
    }

    #[test]
    fn no_candidates_yields_nothing() {
        assert_eq!(most_specific(&[]), None);
    }
}
