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
    /// The exec child failed to spawn, exited nonzero, or wrote unparseable
    /// stdout. The system under test is broken; the model never saw anything.
    pub const EXEC_FAILED: &'static str = "exec_failed";
    /// Rendering vars or the prompt raised — a template bug in the suite.
    pub const RENDER_FAILED: &'static str = "render_failed";
    /// The grader itself failed. The eval did not run; do not read the score.
    pub const GRADER_FAILED: &'static str = "grader_failed";
    /// A deferred assert with no grader configured for its kind.
    pub const GRADER_MISSING: &'static str = "grader_missing";
    /// A local assert blew up while evaluating (bad regex, exec assert nonzero).
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
    /// This is the project's existing exit-code contract expressed as a
    /// predicate: "the harness broke" (exit 3) is categorically different from
    /// "the model regressed" (exit 1), and an error-rate spike is only
    /// interesting when it is the former.
    pub fn is_infrastructure(&self) -> bool {
        self.0.starts_with("provider_")
            || self.0.starts_with("cache_")
            || self.0 == Self::EXEC_FAILED
    }
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
/// that ran and failed, which in turn outranks a local assert blowing up.
pub const PRECEDENCE: &[&str] = &[
    ErrorClass::GRADER_MISSING,
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

    /// The distinction the exit-code contract already draws: a rate limit is the
    /// harness's problem, a failing grader means the eval did not run, and
    /// neither says anything about the model.
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
