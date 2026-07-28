//! Tests for [`super::classify_change`] — the axis that separates "you changed
//! the prompt" from "the model regressed" from "your grader is flaky".

use super::{classify_change, CaseChange, ChangeInputs};

/// Everything known and identical on both sides; callers override one axis.
fn same<'a>() -> ChangeInputs<'a> {
    ChangeInputs {
        base_prompt: Some("blake3:p"),
        head_prompt: Some("blake3:p"),
        base_provider: Some("blake3:m"),
        head_provider: Some("blake3:m"),
        base_asserts: Some("blake3:a"),
        head_asserts: Some("blake3:a"),
        base_grader: Some("blake3:g"),
        head_grader: Some("blake3:g"),
        output_changed: false,
        verdict_changed: false,
    }
}

#[test]
fn nothing_moved_is_stable() {
    assert_eq!(classify_change(&same()), CaseChange::Stable);
}

#[test]
fn a_prompt_edit_is_named_as_such() {
    let i = ChangeInputs {
        head_prompt: Some("blake3:p2"),
        output_changed: true,
        verdict_changed: true,
        ..same()
    };
    assert_eq!(classify_change(&i), CaseChange::PromptChanged);
}

#[test]
fn a_model_bump_is_not_reported_as_a_prompt_change() {
    let i = ChangeInputs {
        head_provider: Some("blake3:m2"),
        output_changed: true,
        ..same()
    };
    assert_eq!(classify_change(&i), CaseChange::ProviderChanged);
}

/// The precedence that matters: an edited prompt explains everything
/// downstream, so it is reported instead of the model bump beside it.
#[test]
fn a_prompt_change_outranks_a_provider_change() {
    let i = ChangeInputs {
        head_prompt: Some("blake3:p2"),
        head_provider: Some("blake3:m2"),
        ..same()
    };
    assert_eq!(classify_change(&i), CaseChange::PromptChanged);
}

#[test]
fn moving_the_goalposts_is_named_as_such() {
    let i = ChangeInputs {
        head_asserts: Some("blake3:a2"),
        verdict_changed: true,
        ..same()
    };
    assert_eq!(classify_change(&i), CaseChange::AssertsChanged);
}

#[test]
fn same_input_new_output_and_a_flipped_verdict_is_model_drift() {
    let i = ChangeInputs {
        output_changed: true,
        verdict_changed: true,
        ..same()
    };
    assert_eq!(classify_change(&i), CaseChange::ModelDrift);
}

#[test]
fn same_input_new_output_but_a_held_verdict_is_tolerated_drift() {
    let i = ChangeInputs {
        output_changed: true,
        ..same()
    };
    assert_eq!(classify_change(&i), CaseChange::OutputDrift);
}

/// The payoff. Identical request, identical output, identical grading
/// definition — and the verdict still flipped. Nothing but the grader is left.
#[test]
fn identical_everything_with_a_flipped_verdict_indicts_the_grader() {
    let i = ChangeInputs {
        verdict_changed: true,
        ..same()
    };
    assert_eq!(classify_change(&i), CaseChange::UnstableGrader);
}

/// Swapping the grader model is a deliberate act, not flakiness.
///
/// Before the grader axis existed this landed in `UnstableGrader` — the
/// classifier accusing the grader of being unstable for a change the author
/// made on purpose, which is the one diagnosis the whole feature exists to
/// make trustworthy.
#[test]
fn replacing_the_grader_is_not_an_unstable_grader() {
    let i = ChangeInputs {
        head_grader: Some("blake3:g2"),
        verdict_changed: true,
        ..same()
    };
    assert_eq!(classify_change(&i), CaseChange::GraderChanged);
}

/// A grader digest missing on one side means the claim "nothing but the grader
/// is left" cannot be made at all.
#[test]
fn an_unknown_grader_blocks_the_unstable_grader_verdict() {
    let i = ChangeInputs {
        base_grader: None,
        verdict_changed: true,
        ..same()
    };
    assert_eq!(classify_change(&i), CaseChange::Unknown);
}

/// The authored grading definition outranks the grader that applied it: if the
/// rubric changed, that explains the flip without reaching for the judge.
#[test]
fn an_assert_change_outranks_a_grader_change() {
    let i = ChangeInputs {
        head_asserts: Some("blake3:a2"),
        head_grader: Some("blake3:g2"),
        ..same()
    };
    assert_eq!(classify_change(&i), CaseChange::AssertsChanged);
}

/// `provider_digest` can never be backfilled, so half of every comparison
/// against a historical run is missing it. That must read as unknown, not as
/// "the provider held" — which would let a real model bump be reported as an
/// unstable grader.
#[test]
fn a_missing_digest_is_unknown_not_unchanged() {
    let i = ChangeInputs {
        base_provider: None,
        verdict_changed: true,
        ..same()
    };
    assert_eq!(classify_change(&i), CaseChange::Unknown);
}

#[test]
fn a_run_with_no_digests_at_all_is_unknown() {
    let i = ChangeInputs {
        verdict_changed: true,
        ..Default::default()
    };
    assert_eq!(classify_change(&i), CaseChange::Unknown);
}

/// A definite change on one axis is a complete explanation, so it wins over an
/// unknown on another — reporting `Unknown` there would discard a real finding.
#[test]
fn a_known_change_outranks_an_unknown_axis() {
    let i = ChangeInputs {
        head_prompt: Some("blake3:p2"),
        base_provider: None,
        head_provider: None,
        ..same()
    };
    assert_eq!(classify_change(&i), CaseChange::PromptChanged);
}
