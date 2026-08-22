//! The gate step's behavior, one test per contract clause: which exit codes
//! render which annotation, that the class breakdown and a failed upload are
//! named, and that an empty breakdown leaves no dangling punctuation. Split
//! from `action_contract.rs` (whose `run_step`/`Workspace` harness these use
//! via `super::`) to keep that file under the 1000-line cap.

use super::{eval_step_with, run_step, Workspace};

/// The contract: `1` means the model regressed and the PR is to blame, `3`
/// means the harness broke and it is not. A CI consumer sees that distinction
/// only here, so the two must not render the same.
#[test]
fn the_gate_distinguishes_a_regression_from_a_broken_harness() {
    let annotation_for = |code: i32| {
        let ws = Workspace::new();
        let observed = eval_step_with(code, &ws)
            .output("exit-code")
            .unwrap_or_default()
            .to_string();
        let ran = run_step("Gate on result", &[("CODE", &observed)], &ws);
        (ran.status, ran.log.trim().to_string())
    };

    let (regression_status, regression) = annotation_for(1);
    assert_eq!(regression_status, Some(1), "a regression fails the job");
    assert!(
        regression.contains("regressions (exit 1)"),
        "a regression must be annotated as one, got: {regression}"
    );

    let (infra_status, infra) = annotation_for(3);
    assert_eq!(infra_status, Some(3), "a broken harness fails the job too");
    assert!(
        infra.contains("infrastructure error (exit 3)"),
        "a broken harness must be annotated as one, got: {infra}"
    );

    let (suite_status, suite) = annotation_for(2);
    assert_eq!(suite_status, Some(2), "a bad suite fails the job too");
    assert!(
        suite.contains("config/usage error (exit 2)"),
        "a suite fault must be annotated as one, got: {suite}"
    );

    assert_ne!(
        regression, infra,
        "the two failures must be told apart; when they read the same the \
         response to a regression is to re-run rather than investigate"
    );
    assert_ne!(
        suite, infra,
        "a suite fault and a broken harness go to different people"
    );
}

/// The gate says *what* broke, not just that something did.
///
/// The annotation used to be a fixed string, so a job that failed because an
/// LLM judge returned malformed JSON and a job that failed because the results
/// server was unreachable produced byte-identical output. Reading the class
/// breakdown meant downloading an artifact.
#[test]
fn the_gate_names_the_error_classes_behind_a_failure() {
    let ws = Workspace::new();
    let ran = run_step(
        "Gate on result",
        &[("CODE", "3"), ("ERROR_CLASSES", "grader_failed × 2")],
        &ws,
    );
    assert_eq!(ran.status, Some(3));
    assert!(
        ran.log.contains("infrastructure error (exit 3)"),
        "got: {}",
        ran.log
    );
    assert!(
        ran.log.contains("grader_failed × 2"),
        "the annotation must name the class, got: {}",
        ran.log
    );

    // Exit 2 carries it too — that is where a suite-caused error now lands.
    let ws = Workspace::new();
    let ran = run_step(
        "Gate on result",
        &[("CODE", "2"), ("ERROR_CLASSES", "grader_missing × 1")],
        &ws,
    );
    assert_eq!(ran.status, Some(2));
    assert!(ran.log.contains("grader_missing × 1"), "got: {}", ran.log);
}

/// A failed `--share` upload drives exit 3 from outside the error-class model
/// — not an errored *case*, so the breakdown cannot name it. The gate detects
/// it structurally: sharing requested, summary ran, no run URL. Without this,
/// the annotation blamed whatever case classes existed instead of the upload.
#[test]
fn the_gate_names_a_failed_upload_separately_from_the_case_classes() {
    let ws = Workspace::new();
    let ran = run_step(
        "Gate on result",
        &[
            ("CODE", "3"),
            ("ERROR_CLASSES", "grader_missing × 1"),
            ("SERVER_URL", "https://domarinn.example"),
            ("SUMMARY_OUTCOME", "success"),
            ("RUN_URL", ""),
        ],
        &ws,
    );
    assert_eq!(ran.status, Some(3));
    assert!(
        ran.log.contains("results upload failed"),
        "got: {}",
        ran.log
    );
    assert!(
        ran.log.contains("grader_missing × 1"),
        "the case breakdown still renders beside it, got: {}",
        ran.log
    );

    // When the upload landed (run-url present), the clause stays silent.
    let ws = Workspace::new();
    let ran = run_step(
        "Gate on result",
        &[
            ("CODE", "3"),
            ("ERROR_CLASSES", "grader_failed × 2"),
            ("SERVER_URL", "https://domarinn.example"),
            ("SUMMARY_OUTCOME", "success"),
            ("RUN_URL", "https://domarinn.example/runs/abc"),
        ],
        &ws,
    );
    assert_eq!(ran.status, Some(3));
    assert!(
        !ran.log.contains("results upload failed"),
        "got: {}",
        ran.log
    );
}

/// The summary step is skipped whenever the eval step never produced a run, so
/// its outputs interpolate to the empty string and the gate has no breakdown to
/// print. It must still render its verdict, and must not trail a bare em dash.
///
/// This covers the empty value, which is what Actions actually supplies — the
/// key stays declared in the step's `env:` block either way. The `${VAR:-}`
/// default in the script guards the genuinely-unset case, which `set -u` would
/// otherwise turn into an abort before the `case` ever runs; that path is not
/// reachable from here because `run_step` seeds every registered env key.
#[test]
fn the_gate_renders_cleanly_when_no_class_breakdown_is_available() {
    let ws = Workspace::new();
    let ran = run_step("Gate on result", &[("CODE", "3")], &ws);
    assert_eq!(ran.status, Some(3));
    assert!(
        ran.log.contains("infrastructure error (exit 3)"),
        "got: {}",
        ran.log
    );
    assert!(
        !ran.log.contains('—'),
        "an empty breakdown must not leave a trailing dash, got: {}",
        ran.log
    );
}
