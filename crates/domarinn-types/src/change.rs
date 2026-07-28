//! What moved between two runs of one case.
//!
//! A wire value — it is carried on every compare row and rendered by the web UI
//! — so it lives here rather than beside the diffing engine that produces it.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// *What* moved between two runs of one case.
///
/// Orthogonal to `Delta` (in `domarinn_core::diff`), which says whether the **verdict** moved. That
/// distinction is the whole point: "you changed the prompt" and "the model
/// regressed" both surface as `NewlyFailing` today, and they call for opposite
/// responses.
///
/// A closed enum, unlike [`crate::empty::EmptyReason`] and
/// [`crate::error_class::ErrorClass`]: those carry values invented by vendors
/// or by an out-of-tree `exec` child, whereas these are derived by domarinn from
/// digest equality and no third party can add one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CaseChange {
    /// A digest is absent on one side or the other — one of the runs predates
    /// the field, and `provider_digest` in particular can never be backfilled.
    /// Deliberately its own variant: "unknown" and "unchanged" are different
    /// answers, and collapsing them would invent a finding.
    Unknown,
    /// The request changed — prompt, vars or params. A different output is
    /// expected here, not a finding.
    PromptChanged,
    /// The model or its settings changed.
    ProviderChanged,
    /// The grading definition changed. The goalposts moved, not the system.
    AssertsChanged,
    /// The suite's grader itself changed — a different model or settings doing
    /// the judging.
    ///
    /// Its own variant rather than folded into `AssertsChanged`, because the
    /// fix is different: one means you rewrote a rubric, the other means you
    /// swapped the judge. And it must exist at all, because without it a
    /// deliberate grader bump landed in `UnstableGrader` — the classifier
    /// accusing the grader of flakiness for a change the author made on
    /// purpose.
    GraderChanged,
    /// Same request and grading, different output, and the verdict flipped.
    ModelDrift,
    /// Same request and grading, different output, verdict held —
    /// nondeterminism within tolerance.
    OutputDrift,
    /// Same request, same output, same grading — and the verdict flipped
    /// anyway. Only one thing is left that could have moved: the grader.
    ///
    /// This is the diagnosis the digests exist to make possible. Grader
    /// verdicts are never cached (see [`crate::runner::AssertGrader`]), so an
    /// `llm-rubric` suite re-grades on every run even at a 100% provider cache
    /// hit rate — and without this, a flaky grader is indistinguishable from a
    /// real regression.
    UnstableGrader,
    /// Nothing moved.
    Stable,
}

/// The digests and outcomes of one case on both sides of a comparison.
#[derive(Debug, Clone, Copy, Default)]
pub struct ChangeInputs<'a> {
    pub base_prompt: Option<&'a str>,
    pub head_prompt: Option<&'a str>,
    pub base_provider: Option<&'a str>,
    pub head_provider: Option<&'a str>,
    pub base_asserts: Option<&'a str>,
    pub head_asserts: Option<&'a str>,
    /// The suite's grader digest. Run-level rather than per-case — the grader
    /// is configured once for the suite — but it belongs on this axis because
    /// it decides verdicts, and `UnstableGrader`'s claim that "nothing but the
    /// grader is left" is only true if the grader itself is known to have held.
    pub base_grader: Option<&'a str>,
    pub head_grader: Option<&'a str>,
    pub output_changed: bool,
    pub verdict_changed: bool,
}

/// `Some(true)`/`Some(false)` when both sides are known, `None` when either is
/// missing. Three states, because two would force a guess.
fn moved(base: Option<&str>, head: Option<&str>) -> Option<bool> {
    match (base, head) {
        (Some(b), Some(h)) => Some(b != h),
        _ => None,
    }
}

/// Classify what moved. See [`CaseChange`].
///
/// A definite change on any axis wins over an unknown on another: if the prompt
/// demonstrably changed, it does not matter that the provider digest is
/// unavailable — the finding is already explained.
pub fn classify_change(i: &ChangeInputs) -> CaseChange {
    let prompt = moved(i.base_prompt, i.head_prompt);
    let provider = moved(i.base_provider, i.head_provider);
    let asserts = moved(i.base_asserts, i.head_asserts);
    let grader = moved(i.base_grader, i.head_grader);

    if prompt == Some(true) {
        return CaseChange::PromptChanged;
    }
    if provider == Some(true) {
        return CaseChange::ProviderChanged;
    }
    if asserts == Some(true) {
        return CaseChange::AssertsChanged;
    }
    if grader == Some(true) {
        return CaseChange::GraderChanged;
    }
    // Nothing is known to have changed. Only claim the input held if it is
    // actually known to have held on every axis — including the grader, or
    // `UnstableGrader` below would indict a grader that was simply replaced.
    if prompt.is_none() || provider.is_none() || asserts.is_none() || grader.is_none() {
        return CaseChange::Unknown;
    }
    match (i.output_changed, i.verdict_changed) {
        (true, true) => CaseChange::ModelDrift,
        (true, false) => CaseChange::OutputDrift,
        (false, true) => CaseChange::UnstableGrader,
        (false, false) => CaseChange::Stable,
    }
}

#[cfg(test)]
#[path = "change_tests.rs"]
mod change_tests;
