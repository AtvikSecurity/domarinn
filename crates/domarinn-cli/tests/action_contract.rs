//! The `domarinn-eval` composite action keeps the CLI's exit-code contract.
//!
//! # Why this guard exists
//!
//! Nothing in this repository runs `.github/actions/domarinn-eval` — it is
//! published for downstream workflows — so its shell was never executed by CI
//! and a break in it is invisible here. One shipped that way: the eval step
//! carried the comment "No `set -e`: we must capture the CLI exit code and keep
//! going", but GitHub expands `shell: bash` to
//! `bash --noprofile --norc -e -o pipefail {0}`. The `-e` comes from the
//! runner, not the script, so a non-zero exit aborted the step before
//! `$GITHUB_OUTPUT` was written. The gate step then read an empty `CODE`, fell
//! through to its `${CODE:-3}` default, and annotated *every* failure as an
//! infrastructure error — telling a reviewer looking at a real regression to
//! re-run rather than investigate, which is precisely the confusion the two
//! exit codes exist to prevent. The artifact upload, gated on `results-path`,
//! was skipped at the same time.
//!
//! So the scripts are run here rather than read. Both tests execute the real
//! `run:` text lifted out of `action.yml`, under the interpreter GitHub uses,
//! against a stub binary that exits on demand. [`the_gate_distinguishes_a_regression_from_a_broken_harness`]
//! is the contract itself; [`the_eval_step_writes_its_outputs_for_every_exit_code`]
//! is the mechanism it rests on, checked separately so a failure says which
//! half moved.
//!
//! [`the_shape_this_test_assumes_still_holds`] guards the assumption underneath
//! both: if the step stops declaring `shell: bash`, or grows an input, the
//! interpreter and environment reproduced here have silently stopped matching
//! the runner's and the other two tests would be asserting about nothing.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const ACTION: &str = ".github/actions/domarinn-eval/action.yml";

/// The interpreter GitHub runs a `shell: bash` step under, verbatim from its
/// documented default: `bash --noprofile --norc -e -o pipefail {0}`.
///
/// Reproduced rather than simplified. Under a plain `bash script.sh` the bug
/// this file exists for does not happen at all, so a test that took the
/// convenient shell would pass against the broken action.
const GITHUB_BASH: &[&str] = &["--noprofile", "--norc", "-e", "-o", "pipefail"];

/// The variables the eval step declares, and the values a caller who set no
/// optional inputs would produce.
///
/// The empty ones are the default path, and the one most likely to break: every
/// optional argument is appended by a `[ -n "$X" ] && args+=(…)` line, and an
/// unset input is what makes those tests fail.
const EVAL_ENV: &[(&str, &str)] = &[
    ("DOMARINN_SERVER_URL", ""),
    ("DOMARINN_TOKEN", ""),
    ("DOMARINN_BRANCH", ""),
    ("INPUT_CONFIG", "suite.yaml"),
    ("INPUT_AGAINST", ""),
    ("INPUT_ALLOW_EMPTY", "false"),
    ("INPUT_CACHE_DIR", ""),
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root resolves from crates/domarinn-cli")
}

fn action() -> serde_yaml_ng::Value {
    let path = repo_root().join(ACTION);
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {ACTION}: {e}"));
    serde_yaml_ng::from_str(&raw).unwrap_or_else(|e| panic!("parsing {ACTION}: {e}"))
}

/// The step whose `field` is `value` — `id` for the steps that have one, `name`
/// for the rest.
fn step(field: &str, value: &str) -> serde_yaml_ng::Value {
    action()["runs"]["steps"]
        .as_sequence()
        .expect("runs.steps is a list")
        .iter()
        .find(|s| s[field].as_str() == Some(value))
        .unwrap_or_else(|| panic!("{ACTION} has no step with {field}: {value}"))
        .clone()
}

fn script_of(field: &str, value: &str) -> String {
    step(field, value)["run"]
        .as_str()
        .unwrap_or_else(|| panic!("the {value} step has a `run:` script"))
        .to_string()
}

/// Run a step's `run:` text the way the runner would, in `dir`.
fn run_step(script: &str, env: &[(&str, &str)], dir: &Path) -> Output {
    let path = dir.join("step.sh");
    std::fs::write(&path, script).unwrap();
    Command::new("bash")
        .args(GITHUB_BASH)
        .arg(&path)
        .current_dir(dir)
        .envs(env.iter().copied())
        .output()
        .expect("bash is on PATH")
}

/// A stand-in for the CLI that writes the report it was asked for and then
/// exits with `code`, so the step sees a real non-zero exit rather than a
/// missing binary.
fn stub_exiting(code: i32, dir: &Path) -> String {
    let path = dir.join("stub-domarinn");
    std::fs::write(
        &path,
        format!("#!/usr/bin/env bash\nprintf '<testsuites/>' > results.xml\nexit {code}\n"),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.to_str().unwrap().to_string()
}

/// The `key=value` lines a step appended to `$GITHUB_OUTPUT`.
fn outputs_written(dir: &Path) -> BTreeMap<String, String> {
    let raw = std::fs::read_to_string(dir.join("github_output")).unwrap_or_default();
    raw.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Run the eval step against a stub exiting `code`, and return what it wrote.
fn eval_step_with(code: i32, dir: &Path) -> (Output, BTreeMap<String, String>) {
    let mut env: Vec<(&str, &str)> = EVAL_ENV.to_vec();
    let stub = stub_exiting(code, dir);
    let output_file = dir.join("github_output");
    std::fs::write(&output_file, "").unwrap();
    env.push(("DOMARINN_BIN", &stub));
    env.push(("GITHUB_OUTPUT", output_file.to_str().unwrap()));

    let out = run_step(&script_of("id", "eval"), &env, dir);
    (out, outputs_written(dir))
}

/// Every exit code the CLI documents must survive as a step output.
///
/// `results-path` matters as much as `exit-code`: the artifact upload is gated
/// on it, so losing it drops the JUnit report exactly when a failing run makes
/// it worth having.
#[test]
fn the_eval_step_writes_its_outputs_for_every_exit_code() {
    for code in [0, 1, 2, 3] {
        let dir = tempfile::tempdir().unwrap();
        let (out, written) = eval_step_with(code, dir.path());
        let log = String::from_utf8_lossy(&out.stdout);

        assert_eq!(
            written.get("exit-code").map(String::as_str),
            Some(code.to_string().as_str()),
            "exit {code} must reach the gate step; the eval step logged:\n{log}\
             {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            written.get("results-path").map(String::as_str),
            Some("results.xml"),
            "exit {code} must still publish the report for the upload step"
        );
        assert!(
            log.contains(&format!("domarinn exited with {code}")),
            "the step log must name the code a reader will see annotated: {log}"
        );
    }
}

/// The contract: `1` means the model regressed and the PR is to blame, `3`
/// means the harness broke and it is not. A CI consumer sees that distinction
/// only here, so the two must not render the same.
#[test]
fn the_gate_distinguishes_a_regression_from_a_broken_harness() {
    let gate = script_of("name", "Gate on result");
    let annotation_for = |code: i32| {
        let dir = tempfile::tempdir().unwrap();
        let (_, written) = eval_step_with(code, dir.path());
        let observed = written.get("exit-code").cloned().unwrap_or_default();
        let out = run_step(
            &gate,
            &[("CODE", observed.as_str()), ("FAIL_ON_REGRESSION", "true")],
            dir.path(),
        );
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        )
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

    assert_ne!(
        regression, infra,
        "the two failures must be told apart; when they read the same the \
         response to a regression is to re-run rather than investigate"
    );
}

/// What the two tests above assume about the step they execute.
///
/// Both reproduce the runner rather than call it: [`GITHUB_BASH`] is the
/// expansion of the literal `shell: bash`, and [`EVAL_ENV`] is hand-written. If
/// either drifts from `action.yml` the tests keep passing while asserting about
/// a step the runner no longer runs.
#[test]
fn the_shape_this_test_assumes_still_holds() {
    let eval = step("id", "eval");
    assert_eq!(
        eval["shell"].as_str(),
        Some("bash"),
        "GITHUB_BASH is the expansion of exactly this string; changing the \
         shell means changing it here too"
    );

    let declared: Vec<&str> = eval["env"]
        .as_mapping()
        .expect("the eval step declares an `env` block")
        .keys()
        .filter_map(|k| k.as_str())
        // Supplied per-run rather than as a constant: one points at the stub,
        // the other at the temp file the outputs are read back from.
        .filter(|k| !matches!(*k, "DOMARINN_BIN" | "GITHUB_OUTPUT"))
        .collect();
    let covered: Vec<&str> = EVAL_ENV.iter().map(|(k, _)| *k).collect();

    let mut declared_sorted = declared.clone();
    declared_sorted.sort_unstable();
    let mut covered_sorted = covered.clone();
    covered_sorted.sort_unstable();
    assert_eq!(
        declared_sorted, covered_sorted,
        "an input the step reads but this test never sets is an input whose \
         effect on the exit-code contract is untested"
    );
}
