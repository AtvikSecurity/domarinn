//! The [`Assertion`] trait — the seam for grading a provider's output.
//!
//! Deterministic asserts (`is_deterministic() == true`) run first and can
//! short-circuit the expensive LLM grader.

use async_trait::async_trait;

use crate::empty::EmptyReason;
use crate::template::TemplateEngine;
use crate::types::Output;

/// Context handed to an assertion when it grades a cell's output.
pub struct AssertCtx<'a> {
    pub output: &'a Output,
    pub provider_id: &'a str,
    pub test_id: &'a str,
    pub tags: &'a [String],
    pub vars: &'a serde_json::Value,
    pub engine: &'a TemplateEngine,
}

/// The outcome of evaluating a single assertion.
#[derive(Debug, Clone)]
pub struct AssertOutcome {
    /// 0.0..=1.0.
    pub score: f64,
    pub passed: bool,
    pub reason: String,
    pub details: Option<serde_json::Value>,
    /// The assertion could not be *evaluated* — a schema that will not compile,
    /// a regex that will not parse. Distinct from "evaluated, and the output did
    /// not satisfy it", because [`Self::negated`] must not turn one into a pass.
    ///
    /// See [`Self::unevaluable`].
    pub unevaluable: bool,
}

impl AssertOutcome {
    pub fn pass(reason: impl Into<String>) -> Self {
        AssertOutcome {
            score: 1.0,
            passed: true,
            reason: reason.into(),
            details: None,
            unevaluable: false,
        }
    }

    /// Attach structured detail to an outcome. Chained rather than a fourth
    /// constructor argument: almost every assertion has nothing structured to
    /// say and should stay a one-liner.
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn fail(reason: impl Into<String>) -> Self {
        AssertOutcome {
            score: 0.0,
            passed: false,
            reason: reason.into(),
            details: None,
            unevaluable: false,
        }
    }

    /// The assertion is broken, not unsatisfied — its schema will not compile,
    /// its regex will not parse, its expression will not evaluate.
    ///
    /// Scores zero like a failure, but is immune to `negate`. Without that
    /// distinction `not-contains-json` with an uncompilable `schema:` scored a
    /// full 1.0: the compile error failed, `negated` flipped it, and the suite
    /// reported a green check for an assertion that never ran. A guard that
    /// fails *open* under negation is worse than no guard.
    pub fn unevaluable(reason: impl Into<String>) -> Self {
        AssertOutcome {
            unevaluable: true,
            ..AssertOutcome::fail(reason)
        }
    }

    /// Apply a `negate` flag, flipping pass/fail and score.
    ///
    /// An [unevaluable](Self::unevaluable) outcome passes through untouched:
    /// "this assertion is broken" has no opposite.
    pub fn negated(self, negate: bool) -> Self {
        if !negate || self.unevaluable {
            return self;
        }
        AssertOutcome {
            score: 1.0 - self.score,
            passed: !self.passed,
            reason: format!("negated: {}", self.reason),
            details: self.details,
            unevaluable: false,
        }
    }

    /// Refuse a pass a negated assertion earned only because the output was
    /// empty. "The forbidden content is absent" is not evidence of compliance
    /// when nothing was produced at all — a refusal must not pass `not-*`
    /// asserts vacuously. Fails (score 0), never errors: this is a judgement
    /// about the output, so the case lands in Fail, not Error (contrast
    /// [`Self::unevaluable`], which is about a broken assertion).
    pub fn deny_vacuous_negated_pass(
        self,
        negate: bool,
        empty_reason: Option<&EmptyReason>,
    ) -> Self {
        let Some(reason) = empty_reason else {
            return self;
        };
        if !negate || !self.passed || self.unevaluable {
            return self;
        }
        AssertOutcome {
            score: 0.0,
            passed: false,
            reason: format!(
                "output was empty ({}): a negated assertion cannot pass vacuously — \
                 nothing was produced for the forbidden content to be absent from",
                reason.as_str()
            ),
            details: self.details,
            unevaluable: false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("assertion error: {0}")]
pub struct AssertError(#[from] pub anyhow::Error);

#[async_trait]
pub trait Assertion: Send + Sync {
    fn kind(&self) -> &'static str;
    /// Deterministic asserts run first and gate the grader.
    fn is_deterministic(&self) -> bool;
    async fn check(&self, ctx: &AssertCtx<'_>) -> Result<AssertOutcome, AssertError>;
}
