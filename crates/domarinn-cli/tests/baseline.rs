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

    let _ = baseline_id;
    let (url, server) = stub_routes(
        vec![("/baseline/export", baseline_doc)],
        1,
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
        1,
        "one baseline-export request, served: {served:?}"
    );
    assert!(
        served[0].contains("exclude="),
        "the head run must be excluded from its own baseline: {}",
        served[0]
    );
}

/// A server that has the suite but no pinned baseline is an absence — the normal
/// state before anyone pins one — so the run still passes. The absence rides on
/// the 404's machine `code`, not its status: a bare 404 means an old server and
/// is fatal (see the legacy-fallback and too-old tests below).
#[test]
fn server_baseline_unpinned_is_an_absence_not_a_failure() {
    let body = r#"{"error":"no baseline pinned for p/s","code":"baseline_unpinned"}"#;
    let (url, server) = common::stub_routes_status(
        vec![("/baseline/export", "404 Not Found", body.to_string())],
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

/// A suite with two tests, `t2` asserting `t2_needle` — flipping the needle to
/// something absent regresses exactly `t2`.
fn suite_two(project: &str, suite: &str, t2_needle: &str) -> String {
    format!(
        r#"
version: 1
project: {project}
suite: {suite}
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{{\"output\":\"hello\"}}'"]
tests:
  - id: t1
    vars: {{}}
    assert:
      - {{type: contains, value: "hello"}}
  - id: t2
    vars: {{}}
    assert:
      - {{type: contains, value: "{t2_needle}"}}
"#
    )
}

/// The local store records `git.branch` only for runs inside a real repository,
/// so the branch-reference tests run in one.
fn git_seed(dir: &std::path::Path) {
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    git(&["init", "-q"]);
    git(&[
        "-c",
        "user.email=t@example.invalid",
        "-c",
        "user.name=t",
        "commit",
        "--allow-empty",
        "-q",
        "-m",
        "seed",
    ]);
}

/// `--against branch:<name>` merges the newest local runs on that branch per
/// case: a filtered newest run must not shrink the gate's coverage. Here the
/// newest `main` run carried only `t1`, so a naive newest-run baseline would
/// see head's failing `t2` as merely *added* — the discriminating assertion is
/// the "Regressions detected" table, which only the merge can produce.
#[test]
fn a_local_branch_reference_merges_the_latest_runs_on_that_branch() {
    let dir = tempfile::tempdir().unwrap();
    git_seed(dir.path());
    let yaml = dir.path().join("domarinn.yaml");

    // Full run on main: t1 and t2 both pass.
    std::fs::write(&yaml, suite_two("p", "s", "hello")).unwrap();
    bin()
        .arg("run")
        .env("DOMARINN_BRANCH", "main")
        .current_dir(dir.path())
        .assert()
        .success();

    // Newest main run is partial: only t1.
    std::fs::write(&yaml, suite_named("p", "s", "hello", "hello")).unwrap();
    bin()
        .arg("run")
        .env("DOMARINN_BRANCH", "main")
        .current_dir(dir.path())
        .assert()
        .success();

    // A feature branch regresses t2.
    std::fs::write(&yaml, suite_two("p", "s", "absent")).unwrap();
    bin()
        .args(["run", "--against", "branch:main", "--no-cache"])
        .env("DOMARINN_BRANCH", "feature/x")
        .current_dir(dir.path())
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "### domarinn comparison — ❌ Regressions detected",
        ))
        .stderr(predicate::str::contains("| Newly failing | 1 |"));
}

/// A branch reference only reads its own branch: runs on other branches are
/// not candidates, and none on the named branch is an absence, not a failure.
#[test]
fn a_local_branch_reference_ignores_runs_on_other_branches() {
    let dir = tempfile::tempdir().unwrap();
    git_seed(dir.path());
    std::fs::write(
        dir.path().join("domarinn.yaml"),
        suite_named("p", "s", "hello", "hello"),
    )
    .unwrap();

    bin()
        .arg("run")
        .env("DOMARINN_BRANCH", "other")
        .current_dir(dir.path())
        .assert()
        .success();

    bin()
        .args(["run", "--against", "branch:main", "--no-cache"])
        .env("DOMARINN_BRANCH", "main")
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("domarinn comparison").not());
}

/// The run being gated persists to the store *before* comparison, so on its
/// own branch it is always the newest candidate — and must be excluded, or
/// every gate self-compares to a permanent all-clear. Only the exclusion makes
/// this regression visible as a regression rather than "no changes".
#[test]
fn the_composite_excludes_the_run_being_gated() {
    let dir = tempfile::tempdir().unwrap();
    git_seed(dir.path());
    let yaml = dir.path().join("domarinn.yaml");

    std::fs::write(&yaml, suite_named("p", "s", "hello", "hello")).unwrap();
    bin()
        .arg("run")
        .env("DOMARINN_BRANCH", "main")
        .current_dir(dir.path())
        .assert()
        .success();

    // Regress on the same branch the baseline reads.
    std::fs::write(&yaml, suite_named("p", "s", "goodbye", "hello")).unwrap();
    bin()
        .args(["run", "--against", "branch:main", "--no-cache"])
        .env("DOMARINN_BRANCH", "main")
        .current_dir(dir.path())
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "### domarinn comparison — ❌ Regressions detected",
        ));
}

/// A branch nobody has run on yet is the bootstrap state — an absence.
#[test]
fn a_local_branch_with_no_runs_is_an_absence_not_a_failure() {
    let dir = tempfile::tempdir().unwrap();
    git_seed(dir.path());
    std::fs::write(
        dir.path().join("domarinn.yaml"),
        suite_named("p", "s", "hello", "hello"),
    )
    .unwrap();

    bin()
        .args(["run", "--against", "branch:main"])
        .env("DOMARINN_BRANCH", "feature/x")
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("domarinn comparison").not());
}

/// `server:branch:<name>` needs no pin at all: the workflow file names the
/// branch and the server merges its newest runs.
#[test]
fn a_server_branch_reference_resolves_without_a_pin() {
    let seed = tempfile::tempdir().unwrap();
    std::fs::write(
        seed.path().join("domarinn.yaml"),
        suite_named("p", "s", "hello", "hello"),
    )
    .unwrap();
    bin().arg("run").current_dir(seed.path()).assert().success();
    let baseline_doc = stored_run(seed.path(), &latest_id(seed.path()));

    let (url, server) = stub_routes(
        vec![("/baseline/export", baseline_doc)],
        1,
        std::time::Duration::from_secs(30),
    );

    let fresh = tempfile::tempdir().unwrap();
    std::fs::write(
        fresh.path().join("domarinn.yaml"),
        suite_named("p", "s", "goodbye", "hello"),
    )
    .unwrap();
    bin()
        .args(["run", "--against", "server:branch:main"])
        .env("DOMARINN_SERVER_URL", &url)
        .current_dir(fresh.path())
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "### domarinn comparison — ❌ Regressions detected",
        ));

    let served = server.join().unwrap();
    assert!(
        served[0].contains("branch=main") && served[0].contains("exclude="),
        "the reference names the branch and excludes the head: {}",
        served[0]
    );
}

/// A branch with no runs on the server is an absence — the coded 404, unlike
/// the bare one below.
#[test]
fn a_server_branch_with_no_runs_is_an_absence_not_a_failure() {
    let body = r#"{"error":"no runs of p/s on branch main","code":"no_runs_on_branch"}"#;
    let (url, server) = common::stub_routes_status(
        vec![("/baseline/export", "404 Not Found", body.to_string())],
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
        .args(["run", "--against", "server:branch:main"])
        .env("DOMARINN_SERVER_URL", &url)
        .current_dir(dir.path())
        .assert()
        .success();

    assert_eq!(server.join().unwrap().len(), 1);
}

/// A server predating the export route answers a *bare* 404. For
/// `server:baseline` that must not skip the gate — the pin may exist and be
/// readable the old way — so resolution falls back to the legacy two-GET path.
#[test]
fn an_old_server_without_the_export_route_falls_back_to_the_legacy_path() {
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
    // Route order matters: the request line for the new endpoint contains both
    // "/suites" and "/export", so the bare-404 route must be first.
    let (url, server) = common::stub_routes_status(
        vec![
            (
                "/baseline/export",
                "404 Not Found",
                r#"{"error":"not found"}"#.to_string(),
            ),
            ("/suites", "200 OK", suites),
            ("/export", "200 OK", baseline_doc),
        ],
        3,
        std::time::Duration::from_secs(30),
    );

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
        ));

    assert_eq!(server.join().unwrap().len(), 3);
}

/// The legacy path cannot express a branch reference, so against an old server
/// `server:branch:<name>` is fatal — silently skipping it would be the exact
/// gate-that-never-fires bug this module exists to prevent.
#[test]
fn a_server_branch_reference_against_an_old_server_is_a_usage_error() {
    let (url, server) = common::stub_routes_status(
        vec![(
            "/baseline/export",
            "404 Not Found",
            r#"{"error":"not found"}"#.to_string(),
        )],
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
        .args(["run", "--against", "server:branch:main"])
        .env("DOMARINN_SERVER_URL", &url)
        .current_dir(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("does not support branch"));

    assert_eq!(server.join().unwrap().len(), 1);
}

/// A one-test suite that also declares a default baseline branch.
fn suite_with_baseline(project: &str, suite: &str, output: &str, needle: &str) -> String {
    format!(
        r#"
version: 1
project: {project}
suite: {suite}
baseline:
  branch: main
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

/// `baseline: {{ branch: main }}` in the suite makes the comparison the
/// default: no `--against` on the command line, and the gate still fires.
#[test]
fn the_suite_baseline_key_supplies_the_default_comparison() {
    let dir = tempfile::tempdir().unwrap();
    git_seed(dir.path());
    let yaml = dir.path().join("domarinn.yaml");

    std::fs::write(&yaml, suite_with_baseline("p", "s", "hello", "hello")).unwrap();
    bin()
        .arg("run")
        .env("DOMARINN_BRANCH", "main")
        .env_remove("DOMARINN_SERVER_URL")
        .current_dir(dir.path())
        .assert()
        .success();

    std::fs::write(&yaml, suite_with_baseline("p", "s", "goodbye", "hello")).unwrap();
    bin()
        .args(["run", "--no-cache"])
        .env("DOMARINN_BRANCH", "main")
        .env_remove("DOMARINN_SERVER_URL")
        .current_dir(dir.path())
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "### domarinn comparison — ❌ Regressions detected",
        ));
}

/// The flag always wins over the suite key — `--against none` turns the
/// default comparison off entirely.
#[test]
fn an_explicit_against_overrides_the_suite_baseline_key() {
    let dir = tempfile::tempdir().unwrap();
    git_seed(dir.path());
    let yaml = dir.path().join("domarinn.yaml");

    std::fs::write(&yaml, suite_with_baseline("p", "s", "hello", "hello")).unwrap();
    bin()
        .arg("run")
        .env("DOMARINN_BRANCH", "main")
        .env_remove("DOMARINN_SERVER_URL")
        .current_dir(dir.path())
        .assert()
        .success();

    bin()
        .args(["run", "--against", "none", "--no-cache"])
        .env("DOMARINN_BRANCH", "main")
        .env_remove("DOMARINN_SERVER_URL")
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("domarinn comparison").not());
}

/// With a server configured, the suite default aims at the server — a fresh CI
/// checkout has no local store, and the whole point of the default is that the
/// workflow file stays empty.
#[test]
fn the_suite_default_prefers_the_server_when_one_is_configured() {
    let seed = tempfile::tempdir().unwrap();
    std::fs::write(
        seed.path().join("domarinn.yaml"),
        suite_named("p", "s", "hello", "hello"),
    )
    .unwrap();
    bin().arg("run").current_dir(seed.path()).assert().success();
    let baseline_doc = stored_run(seed.path(), &latest_id(seed.path()));

    let (url, server) = stub_routes(
        vec![("/baseline/export", baseline_doc)],
        1,
        std::time::Duration::from_secs(30),
    );

    let fresh = tempfile::tempdir().unwrap();
    std::fs::write(
        fresh.path().join("domarinn.yaml"),
        suite_with_baseline("p", "s", "goodbye", "hello"),
    )
    .unwrap();
    bin()
        .arg("run")
        .env("DOMARINN_SERVER_URL", &url)
        .current_dir(fresh.path())
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "### domarinn comparison — ❌ Regressions detected",
        ));

    let served = server.join().unwrap();
    assert!(
        served[0].contains("branch=main"),
        "the suite default must resolve through the server branch reference: {}",
        served[0]
    );
}

/// `--against none` is the explicit opt-out (the suite config can make a
/// comparison the default; the flag must be able to turn it off). No
/// comparison runs even though a baseline exists.
#[test]
fn against_none_disables_the_comparison() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("domarinn.yaml"),
        suite_named("p", "s", "hello", "hello"),
    )
    .unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();

    bin()
        .args(["run", "--against", "none", "--no-cache"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("domarinn comparison").not());
}
