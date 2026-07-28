//! End-to-end tests for structured error classification: the class travels with
//! the failure from the point it happened, rather than being sniffed back out of
//! a prose message later.

mod common;

use assert_cmd::prelude::*;
use common::{bin, latest_run};

fn write(dir: &std::path::Path, body: &str) {
    std::fs::write(dir.join("domarinn.yaml"), body).unwrap();
}

/// A deferred assert with no grader configured fails closed — and says which
/// kind of problem it was, so a reader can tell "the eval did not run" from
/// "the model got it wrong".
#[test]
fn a_deferred_assert_with_no_grader_is_classified_grader_missing() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        r#"
version: 1
project: p
suite: s
providers:
  - id: prov
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"hello\"}'"]
tests:
  - id: t1
    vars: {}
    assert:
      - {type: llm-rubric, value: "is it polite?"}
"#,
    );
    // An errored case is infra (exit 3), not an assertion failure.
    bin().arg("run").current_dir(dir.path()).assert().code(3);

    let run = latest_run(dir.path());
    let case = &run.cases[0];
    assert_eq!(
        case.error_class.as_ref().map(|c| c.as_str()),
        Some("grader_missing")
    );
    // The prose is untouched and still carries the detail.
    assert!(
        case.error
            .as_deref()
            .unwrap_or_default()
            .contains("llm-rubric"),
        "prose should still name the assert: {:?}",
        case.error
    );
}

/// A broken system under test is the harness's problem, not the model's.
#[test]
fn an_exec_provider_that_cannot_run_is_classified_exec_failed() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        r#"
version: 1
project: p
suite: s
providers:
  - id: prov
    type: exec
    command: ["definitely-not-a-real-binary-xyzzy"]
tests:
  - id: t1
    vars: {}
    assert:
      - {type: contains, value: "hello"}
"#,
    );
    bin().arg("run").current_dir(dir.path()).assert().code(3);

    let case = &latest_run(dir.path()).cases[0];
    assert_eq!(
        case.error_class.as_ref().map(|c| c.as_str()),
        Some("exec_failed")
    );
}

/// A template bug is the suite author's problem, and is distinguishable from
/// every kind of provider trouble.
#[test]
fn a_prompt_that_cannot_render_is_classified_render_failed() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        r#"
version: 1
project: p
suite: s
providers:
  - id: prov
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"hello\"}'"]
prompts:
  - id: main
    template: "{{ missing | nosuchfilter }}"
tests:
  - id: t1
    vars: {}
    assert:
      - {type: contains, value: "hello"}
"#,
    );
    bin().arg("run").current_dir(dir.path()).assert().code(3);

    let case = &latest_run(dir.path()).cases[0];
    assert_eq!(
        case.error_class.as_ref().map(|c| c.as_str()),
        Some("render_failed")
    );
}

/// `--cache-only` with an empty cache is a workflow problem, not a provider one.
#[test]
fn a_cache_only_miss_is_classified_cache_miss() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        r#"
version: 1
project: p
suite: s
providers:
  - id: prov
    type: exec
    cache_salt: "x"
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"hello\"}'"]
tests:
  - id: t1
    vars: {}
    assert:
      - {type: contains, value: "hello"}
"#,
    );
    bin()
        .args(["run", "--cache-only"])
        .current_dir(dir.path())
        .assert()
        .code(3);

    let case = &latest_run(dir.path()).cases[0];
    assert_eq!(
        case.error_class.as_ref().map(|c| c.as_str()),
        Some("cache_miss")
    );
}

/// A passing case has no failure to classify, and must not be given one.
#[test]
fn a_passing_case_carries_no_error_class() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        r#"
version: 1
project: p
suite: s
providers:
  - id: prov
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"hello\"}'"]
tests:
  - id: t1
    vars: {}
    assert:
      - {type: contains, value: "hello"}
"#,
    );
    bin().arg("run").current_dir(dir.path()).assert().success();

    let case = &latest_run(dir.path()).cases[0];
    assert_eq!(case.error_class, None);
    assert_eq!(case.error, None);
}
