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

/// A passing suite that also names an `http` cache backend with no server URL,
/// so `build_cache` degrades to local disk and emits a `WARN` diagnostic. The
/// run itself still passes — the warning is a mid-flight fallback, not the
/// command outcome.
const CACHE_WARN_SUITE: &str = r#"
version: 1
suite: smoke
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"hello world\"}'"]
cache:
  backend: http
tests:
  - id: greet
    vars: {}
    assert:
      - {type: contains, value: "hello"}
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

/// The shipped example must keep validating cleanly under the strict loader
/// (deny-unknown-fields + the provider/assert flatten-gap check).
#[test]
fn shipped_example_validates() {
    let example =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/render-health");
    bin()
        .arg("validate")
        .arg(example)
        .assert()
        .success()
        .stdout(predicate::str::contains("ok:"));
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

#[test]
fn log_format_json_flag_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), PASSING_SUITE);
    bin()
        .args(["--log-format", "json", "validate"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("ok:"));
}

/// Hard invariant: results go to stdout, diagnostics to stderr. Even with a
/// cache-fallback WARN in flight, stdout carries only the semantic results
/// (PASS/FAIL) and no log-formatted lines leak into it.
#[test]
fn stdout_stays_pure_results_diagnostics_go_to_stderr() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), CACHE_WARN_SUITE);
    let output = bin()
        .arg("run")
        .current_dir(dir.path())
        .env_remove("MEASURELLM_SERVER_URL")
        .output()
        .unwrap();
    assert!(output.status.success(), "cache-fallback run should pass");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Results land on stdout.
    assert!(stdout.contains("PASS"), "stdout should carry results");
    // No log lines pollute stdout.
    assert!(
        !stdout.contains(" WARN ") && !stdout.contains(" INFO "),
        "stdout must not contain log-level tokens; got:\n{stdout}"
    );
    assert!(
        !stdout.contains("using local disk"),
        "the cache diagnostic must not appear on stdout; got:\n{stdout}"
    );
    // The diagnostic itself lands on stderr as a log line.
    assert!(
        stderr.contains("using local disk"),
        "the cache diagnostic should appear on stderr; got:\n{stderr}"
    );
}

/// With `--log-format json`, every stderr line is a JSON object and the cache
/// fallback surfaces as a structured `WARN` event. This proves the print
/// conversion end-to-end: an `eprintln!` warning could never satisfy this.
#[test]
fn json_log_format_emits_structured_warn_on_stderr() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), CACHE_WARN_SUITE);
    let output = bin()
        .args(["--log-format", "json", "run"])
        .current_dir(dir.path())
        .env_remove("MEASURELLM_SERVER_URL")
        .output()
        .unwrap();
    assert!(output.status.success(), "cache-fallback run should pass");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(!lines.is_empty(), "expected at least one stderr log line");
    let mut saw_cache_warn = false;
    for line in &lines {
        let value: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("stderr line is not JSON ({e}): {line}"));
        if value["level"] == "WARN"
            && value["message"]
                .as_str()
                .is_some_and(|m| m.contains("using local disk"))
        {
            saw_cache_warn = true;
        }
    }
    assert!(
        saw_cache_warn,
        "expected a JSON WARN line for the cache fallback; got:\n{stderr}"
    );
}

/// `RUST_LOG` replaces the default filter entirely, so `RUST_LOG=off` silences
/// the cache-fallback WARN. Converted diagnostics honor the filter — an
/// `eprintln!` would print regardless.
#[test]
fn rust_log_off_silences_cache_warning() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), CACHE_WARN_SUITE);
    let output = bin()
        .arg("run")
        .current_dir(dir.path())
        .env("RUST_LOG", "off")
        .env_remove("MEASURELLM_SERVER_URL")
        .output()
        .unwrap();
    assert!(output.status.success(), "cache-fallback run should pass");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("using local disk"),
        "RUST_LOG=off must suppress the cache WARN diagnostic; got:\n{stderr}"
    );
}
