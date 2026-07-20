//! E2E tests for the diff, view, cache, import, and gen-types commands.

use std::path::Path;
use std::process::Command;

use assert_cmd::prelude::*;
use predicates::prelude::*;

fn bin() -> Command {
    Command::cargo_bin("measurellm").unwrap()
}

/// A suite whose single test echoes a fixed output and asserts it contains
/// `needle`.
fn suite(output: &str, needle: &str) -> String {
    format!(
        r#"
version: 1
suite: s
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{{\"output\":\"{output}\"}}'"]
tests:
  - id: t1
    vars: {{}}
    assert:
      - {{type: contains, value: "{needle}"}}
"#
    )
}

fn run_to(dir: &Path, out_json: &str, suite_body: &str) {
    std::fs::write(dir.join("measurellm.yaml"), suite_body).unwrap();
    // The run itself may pass or fail (that is the point of the diff); we only
    // need it to produce the result file, so ignore the exit status.
    bin()
        .args(["run", "--format", "json", "--out", out_json, "--no-cache"])
        .current_dir(dir)
        .output()
        .expect("run command executes");
}

#[test]
fn diff_detects_regression_and_exits_one() {
    let dir = tempfile::tempdir().unwrap();
    run_to(dir.path(), "base.json", &suite("hello", "hello"));
    run_to(dir.path(), "head.json", &suite("goodbye", "hello"));
    bin()
        .args(["diff", "base.json", "head.json"])
        .current_dir(dir.path())
        .assert()
        .code(1)
        .stdout(predicate::str::contains("newly failing").or(predicate::str::contains("REGRESS")));
}

#[test]
fn diff_no_regression_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    run_to(dir.path(), "base.json", &suite("hello", "hello"));
    run_to(dir.path(), "head.json", &suite("hello there", "hello"));
    bin()
        .args(["diff", "base.json", "head.json"])
        .current_dir(dir.path())
        .assert()
        .success();
}

#[test]
fn run_against_latest_flags_regression() {
    let dir = tempfile::tempdir().unwrap();
    // First (passing) run establishes the baseline persisted under .measurellm.
    std::fs::write(dir.path().join("measurellm.yaml"), suite("hello", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();
    // Now a regressed suite compared against the latest run.
    std::fs::write(
        dir.path().join("measurellm.yaml"),
        suite("goodbye", "hello"),
    )
    .unwrap();
    bin()
        .args(["run", "--against", "latest"])
        .current_dir(dir.path())
        .assert()
        .code(1);
}

#[test]
fn view_latest_renders() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("measurellm.yaml"), suite("hello", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();
    bin()
        .args(["view", "latest"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS"));
}

#[test]
fn cache_path_and_stats() {
    let dir = tempfile::tempdir().unwrap();
    bin()
        .args(["cache", "path"])
        .current_dir(dir.path())
        .assert()
        .success();
    bin()
        .args(["cache", "stats"])
        .current_dir(dir.path())
        .assert()
        .success();
}

#[test]
fn import_promptfoo_produces_a_valid_suite() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("promptfooconfig.yaml"),
        r#"
description: imported
providers:
  - openai:gpt-4o
  - anthropic:messages:claude-3-5-sonnet
prompts:
  - "Answer: {{ q }}"
tests:
  - vars: {q: "hi"}
    assert:
      - type: contains
        value: "hello"
      - type: llm-rubric
        value: "is polite"
"#,
    )
    .unwrap();
    let output = bin()
        .args(["import", "promptfoo", "promptfooconfig.yaml"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    // The emitted YAML must itself validate as a measurellm suite.
    std::fs::write(dir.path().join("measurellm.yaml"), &output.stdout).unwrap();
    bin()
        .args(["validate", "measurellm.yaml"])
        .current_dir(dir.path())
        .assert()
        .success();
}

#[test]
fn gen_types_writes_typescript() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("types");
    bin()
        .args(["gen-types", out.to_str().unwrap()])
        .assert()
        .success();
    assert!(
        out.join("RunResult.ts").exists(),
        "RunResult.ts should exist"
    );
}
