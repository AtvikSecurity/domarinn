//! The [`Assertion`] trait — the seam for grading a provider's output.
//!
//! Deterministic asserts (`is_deterministic() == true`) run first and can
//! short-circuit the expensive LLM grader.

use async_trait::async_trait;

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
}

impl AssertOutcome {
    pub fn pass(reason: impl Into<String>) -> Self {
        AssertOutcome {
            score: 1.0,
            passed: true,
            reason: reason.into(),
            details: None,
        }
    }

    pub fn fail(reason: impl Into<String>) -> Self {
        AssertOutcome {
            score: 0.0,
            passed: false,
            reason: reason.into(),
            details: None,
        }
    }

    /// Apply a `negate` flag, flipping pass/fail and score.
    pub fn negated(self, negate: bool) -> Self {
        if !negate {
            return self;
        }
        AssertOutcome {
            score: 1.0 - self.score,
            passed: !self.passed,
            reason: format!("negated: {}", self.reason),
            details: self.details,
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
