//! Tests for `--against` baseline resolution — the CI regression gate.
//!
//! The behaviour under test is narrow and load-bearing: a gate that cannot find
//! its baseline must not report green. `--against latest` used to resolve
//! through a cwd-relative `.domarinn/runs/latest` pointer, so on a fresh CI
//! checkout it found nothing, logged a warning, and let the job exit 0 on a real
//! regression.

mod common;

use assert_cmd::prelude::*;
use common::{bin, latest_id, stub_routes};
use predicates::prelude::*;

/// A suite that names both a project and a suite (required to address a
/// server-pinned baseline, whose columns are NOT NULL) and asserts `needle`
/// appears in a fixed `output`.
fn suite_named(project: &str, suite: &str, output: &str, needle: &str) -> String {
    format!(
        r#"
version: 1
project: {project}
suite: {suite}
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

fn stored_run(dir: &std::path::Path, id: &str) -> String {
    std::fs::read_to_string(dir.join(".domarinn/runs").join(id).join("result.json")).unwrap()
}

/// The regression this whole phase exists for. A named baseline that cannot be
/// resolved is a usage error, never a silent pass — otherwise a gate reports
/// green having compared nothing.
///
/// Fully discriminating: the run's own assertions pass, so without the fix the
/// exit code would be 0.
#[test]
fn an_unresolvable_baseline_fails_the_run_rather_than_passing_it() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("domarinn.yaml"),
        suite_named("p", "s", "hello", "hello"),
    )
    .unwrap();

    bin()
        .args(["run", "--against", "nosuchrun"])
        .current_dir(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--against"));
}

/// A suite's first run has nothing to compare against. That is an absence, not a
/// failure, and must not fail the job.
#[test]
fn a_first_run_with_no_baseline_yet_still_passes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("domarinn.yaml"),
        suite_named("p", "s", "hello", "hello"),
    )
    .unwrap();

    bin()
        .args(["run", "--against", "latest"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("domarinn comparison").not());
}

/// `latest` means the newest run *of this suite*. The `latest` pointer file
/// records the last run of any suite, so resolving through it silently diffed
/// one suite against another — and because `case_key` carries no suite, the
/// result looked plausible rather than empty.
#[test]
fn latest_does_not_compare_across_suites() {
    let dir = tempfile::tempdir().unwrap();

    // Suite "a" runs first and becomes the `latest` pointer's target.
    std::fs::write(
        dir.path().join("domarinn.yaml"),
        suite_named("p", "a", "hello", "hello"),
    )
    .unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();

    // Suite "b" then asks for a baseline. Its only candidate is a run of a
    // different suite, so there must be no comparison at all.
    std::fs::write(
        dir.path().join("domarinn.yaml"),
        suite_named("p", "b", "hello", "hello"),
    )
    .unwrap();
    bin()
        .args(["run", "--against", "latest"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("domarinn comparison").not());
}

/// Two runs of the *same* suite do compare — the guard above must not have made
/// `latest` useless.
#[test]
fn latest_compares_two_runs_of_the_same_suite() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("domarinn.yaml"),
        suite_named("p", "s", "hello", "hello"),
    )
    .unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();

    bin()
        .args(["run", "--against", "latest", "--no-cache"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "### domarinn comparison — ✅ No regressions",
        ));
}

/// Naming an explicit run of another suite is rejected outright rather than
/// producing a confident diff of unrelated cases.
#[test]
fn an_explicit_baseline_from_another_suite_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("domarinn.yaml"),
        suite_named("p", "a", "hello", "hello"),
    )
    .unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();
    let other = latest_id(dir.path());

    std::fs::write(
        dir.path().join("domarinn.yaml"),
        suite_named("p", "b", "hello", "hello"),
    )
    .unwrap();
    bin()
        .args(["run", "--against", &other])
        .current_dir(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("comparing different suites"));
}

/// `server:baseline` without a server is a usage error, not a silent skip.
#[test]
fn server_baseline_without_a_server_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("domarinn.yaml"),
        suite_named("p", "s", "hello", "hello"),
    )
    .unwrap();

    bin()
        .args(["run", "--against", "server:baseline"])
        .env_remove("DOMARINN_SERVER_URL")
        .current_dir(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("needs a server"));
}

/// A suite with no `project:`/`suite:` cannot address a server-pinned baseline,
/// because the server keys them on both. Say so instead of guessing a default.
#[test]
fn server_baseline_needs_a_project_and_suite() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("domarinn.yaml"),
        common::suite("hello", "hello"),
    )
    .unwrap();

    bin()
        .args(["run", "--against", "server:baseline"])
        .env("DOMARINN_SERVER_URL", "http://127.0.0.1:1")
        .current_dir(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "needs both `project:` and `suite:`",
        ));
}

/// The end-to-end CI shape: a fresh checkout with no local run store resolves the
/// server's pinned baseline and catches a regression.
///
/// The exit code alone would not prove this — a run whose own assertions fail
/// exits 1 regardless. The comparison table on stderr and the two served
/// endpoints are what show the gate actually ran.
#[test]
fn server_baseline_catches_a_regression_in_a_fresh_checkout() {
    // Produce a passing baseline run in one directory...
    let seed = tempfile::tempdir().unwrap();
    std::fs::write(
        seed.path().join("domarinn.yaml"),
        suite_named("p", "s", "hello", "hello"),
    )
    .unwrap();
    bin().arg("run").current_dir(seed.path()).assert().success();
    let baseline_id = latest_id(seed.path());
    let baseline_doc = stored_run(seed.path(), &baseline_id);

    let suites = format!(
        r#"{{"project":"p","suites":[{{"suite":"s","run_count":1,"last_run_at":null,"baseline_run_id":"{baseline_id}","series":[]}}]}}"#
    );
    let (url, server) = stub_routes(
        vec![("/suites", suites), ("/export", baseline_doc)],
        2,
        std::time::Duration::from_secs(30),
    );

    // ...then regress it from a directory with no `.domarinn/` at all, which is
    // exactly what CI sees. `--against latest` finds nothing here.
    let fresh = tempfile::tempdir().unwrap();
    std::fs::write(
        fresh.path().join("domarinn.yaml"),
        suite_named("p", "s", "goodbye", "hello"),
    )
    .unwrap();
    bin()
        .args(["run", "--against", "server:baseline"])
        .env("DOMARINN_SERVER_URL", &url)
        .current_dir(fresh.path())
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "### domarinn comparison — ❌ Regressions detected",
        ))
        .stderr(predicate::str::contains("| Newly failing | 1 |"));

    let served = server.join().unwrap();
    assert_eq!(
        served.len(),
        2,
        "expected the suite listing then the baseline export, served: {served:?}"
    );
}

/// A server that has the suite but no pinned baseline is an absence — the normal
/// state before anyone pins one — so the run still passes.
#[test]
fn server_baseline_unpinned_is_an_absence_not_a_failure() {
    let suites = r#"{"project":"p","suites":[{"suite":"s","run_count":1,"last_run_at":null,"baseline_run_id":null,"series":[]}]}"#;
    let (url, server) = stub_routes(
        vec![("/suites", suites.to_string())],
        1,
        std::time::Duration::from_secs(30),
    );

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("domarinn.yaml"),
        suite_named("p", "s", "hello", "hello"),
    )
    .unwrap();
    bin()
        .args(["run", "--against", "server:baseline"])
        .env("DOMARINN_SERVER_URL", &url)
        .current_dir(dir.path())
        .assert()
        .success();

    assert_eq!(server.join().unwrap().len(), 1);
}
