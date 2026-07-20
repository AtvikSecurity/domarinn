//! End-to-end CLI tests driving the built `measurellm` binary.

use std::process::Command;

use assert_cmd::prelude::*;
use predicates::prelude::*;

fn bin() -> Command {
    Command::cargo_bin("measurellm").unwrap()
}

fn write_suite(dir: &std::path::Path, body: &str) {
    std::fs::write(dir.join("measurellm.yaml"), body).unwrap();
}

/// A self-contained exec provider that echoes a fixed output.
const PASSING_SUITE: &str = r#"
version: 1
project: cli-test
suite: smoke
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"hello world\"}'"]
tests:
  - id: greet
    vars: {}
    assert:
      - {type: contains, value: "hello"}
"#;

const FAILING_SUITE: &str = r#"
version: 1
suite: smoke
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"hello\"}'"]
tests:
  - id: greet
    vars: {}
    assert:
      - {type: contains, value: "goodbye"}
"#;

#[test]
fn validate_ok() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), PASSING_SUITE);
    bin()
        .arg("validate")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("ok:"));
}

#[test]
fn validate_flags_missing_providers() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), "version: 1\nproviders: []\n");
    bin()
        .arg("validate")
        .arg(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("provider"));
}

#[test]
fn schema_config_emits_json_schema() {
    bin()
        .args(["schema", "config"])
        .assert()
        .success()
        .stdout(predicate::str::contains("$schema"));
}

#[test]
fn run_passing_suite_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), PASSING_SUITE);
    bin()
        .arg("run")
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS"));
}

#[test]
fn run_failing_suite_exits_one() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), FAILING_SUITE);
    bin()
        .arg("run")
        .current_dir(dir.path())
        .assert()
        .code(1)
        .stdout(predicate::str::contains("FAIL"));
}

#[test]
fn run_json_format_is_valid_json() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), PASSING_SUITE);
    let output = bin()
        .args(["run", "--format", "json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["summary"]["passed"], 1);
}

#[test]
fn run_llm_rubric_without_grader_is_infra_error() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(
        dir.path(),
        r#"
version: 1
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"x\"}'"]
tests:
  - id: t
    vars: {}
    assert:
      - {type: llm-rubric, value: "is good"}
"#,
    );
    // Fail closed: a deferred assert with no grader is an infra error (exit 3).
    bin().arg("run").current_dir(dir.path()).assert().code(3);
}

#[test]
fn run_persists_latest_pointer() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), PASSING_SUITE);
    bin().arg("run").current_dir(dir.path()).assert().success();
    let latest = dir.path().join(".measurellm").join("runs").join("latest");
    assert!(latest.exists(), "latest pointer should be written");
}
