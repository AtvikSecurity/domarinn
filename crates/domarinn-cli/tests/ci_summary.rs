//! End-to-end tests for the CI-facing surface: `run --share` recording the
//! run's URL, and `ci-summary` turning a stored run into a PR comment plus
//! GitHub Actions step outputs.

mod common;

use std::thread::JoinHandle;
use std::time::Duration;

use assert_cmd::prelude::*;
use common::{bin, latest_run, meta_ok_body, run_to, stub_routes_status, stub_server, suite};
use predicates::prelude::*;

/// A `{"url": ...}` ingest response for a run that lands at `abc`.
const SHARE_OK: &str = r#"{"url":"https://domarinn.test/runs/abc"}"#;

/// The server `run --share` talks to: the ingest POST answering `status` with
/// `body`, plus the `/meta` route the upload's preflight consults.
///
/// A routing stub rather than [`stub_server`] because the share leg is on its
/// way to making more than one request; registering `/meta` here now means the
/// preflight lands without every one of these tests having to be rewritten. The
/// count is today's request total (the POST alone) so `join` still returns as
/// soon as the client is done rather than idling to the deadline — adding the
/// preflight means bumping it to 2.
fn share_stub(status: &'static str, body: &'static str) -> (String, JoinHandle<Vec<String>>) {
    stub_routes_status(
        vec![
            ("/api/v1/meta", "200 OK", meta_ok_body()),
            ("/api/v1/runs", status, body.to_string()),
        ],
        1,
        Duration::from_secs(30),
    )
}

/// `--share` must record the URL the server returned onto the persisted run,
/// so a later `ci-summary` can link to it without re-uploading or scraping
/// stdout.
#[test]
fn run_share_persists_the_returned_run_url() {
    let (url, server) = share_stub("200 OK", SHARE_OK);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();

    bin()
        .args(["run", "--share"])
        .env("DOMARINN_SERVER_URL", &url)
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "View run: https://domarinn.test/runs/abc",
        ));
    server.join().unwrap();

    assert_eq!(
        latest_run(dir.path()).share_url.as_deref(),
        Some("https://domarinn.test/runs/abc")
    );
}

/// A re-share must upload byte-identical content to the first share. Ingest is
/// idempotent on `sha256(canonical_json(run))` keyed by run id, so a document
/// carrying the URL of its *own previous upload* hashes differently and turns a
/// harmless `domarinn share` into a 409 Conflict. The recorded URL therefore
/// stays local and never travels with the run.
#[test]
fn re_sharing_a_recorded_run_does_not_upload_its_own_url() {
    let (first, first_server) = share_stub("200 OK", SHARE_OK);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();

    bin()
        .args(["run", "--share"])
        .env("DOMARINN_SERVER_URL", &first)
        .current_dir(dir.path())
        .assert()
        .success();
    first_server.join().unwrap();
    // Precondition: the URL really did land on the stored run, so the re-share
    // below is genuinely uploading a document that carries one. Without this the
    // test would still pass if `--share` silently stopped recording anything.
    assert!(latest_run(dir.path()).share_url.is_some());

    let (second, second_server) =
        stub_server("200 OK", r#"{"url":"https://domarinn.test/runs/abc"}"#);
    bin()
        .arg("share")
        .env("DOMARINN_SERVER_URL", &second)
        .current_dir(dir.path())
        .assert()
        .success();

    let request = String::from_utf8_lossy(&second_server.join().unwrap()).into_owned();
    // Guard against a vacuous pass: prove we captured the run document itself
    // before concluding anything from the absence of a substring.
    assert!(
        request.contains("schema_version"),
        "captured request is not the run document: {request}"
    );
    assert!(
        !request.contains("share_url"),
        "re-share uploaded its own previous URL, which changes the content hash \
         and makes the server answer 409 instead of 200: {request}"
    );
}

/// A run that was never shared carries no URL — the field must stay absent
/// rather than persisting an empty string a renderer would have to special-case.
#[test]
fn run_without_share_persists_no_run_url() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();

    let raw = std::fs::read_to_string(
        dir.path()
            .join(".domarinn/runs")
            .join(common::latest_id(dir.path()))
            .join("result.json"),
    )
    .unwrap();
    assert!(!raw.contains("share_url"));
}

/// A rejected upload fails the run. `run --share` in CI exists to *store* the
/// results, so exiting 0 having stored nothing reports a green job for work that
/// went nowhere — the workflow moves on, and the gap is only noticed when
/// someone goes looking for the run. The grading still stands: the run is
/// persisted locally, and simply carries no URL.
#[test]
fn run_share_failure_exits_infra_by_default() {
    let (url, server) = share_stub("500 Internal Server Error", r#"{"error":"nope"}"#);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();

    bin()
        .args(["run", "--share"])
        .env("DOMARINN_SERVER_URL", &url)
        .current_dir(dir.path())
        .assert()
        .code(3)
        .stderr(predicate::str::contains("share failed"));
    server.join().unwrap();

    // The run is still on disk and still graded — only the publish leg failed.
    assert_eq!(latest_run(dir.path()).summary.passed, 1);
    assert_eq!(latest_run(dir.path()).share_url, None);
}

/// The opt-out for a job where publishing is genuinely optional (a fork's PR
/// with no credentials): the upload still fails, and the run still exits on its
/// own merits.
#[test]
fn run_share_failure_is_tolerated_with_allow_share_failure() {
    let (url, server) = share_stub("500 Internal Server Error", r#"{"error":"nope"}"#);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();

    bin()
        .args(["run", "--share", "--allow-share-failure"])
        .env("DOMARINN_SERVER_URL", &url)
        .current_dir(dir.path())
        .assert()
        .success();
    server.join().unwrap();

    assert_eq!(latest_run(dir.path()).share_url, None);
}

/// A server that never answers is the same failure as one that says no — a
/// misconfigured URL is far likelier in CI than a rejecting server, and it must
/// not be the quieter of the two.
#[test]
fn run_share_with_unreachable_server_exits_infra() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();
    // Port 1 is reserved and unbindable without privileges, so nothing is
    // listening and the connect is refused immediately rather than timing out.
    let unreachable = "http://127.0.0.1:1";

    bin()
        .args(["run", "--share"])
        .env("DOMARINN_SERVER_URL", unreachable)
        .current_dir(dir.path())
        .assert()
        .code(3)
        .stderr(predicate::str::contains("share failed"));

    bin()
        .args(["run", "--share", "--allow-share-failure"])
        .env("DOMARINN_SERVER_URL", unreachable)
        .current_dir(dir.path())
        .assert()
        .success();
}

/// `--share` with nowhere to share to is a failure, not a no-op. This was the
/// quietest hole of the three: the missing-URL error never reached the network,
/// so a workflow that forgot `DOMARINN_SERVER_URL` passed its gate, uploaded
/// nothing, and printed one warning nobody reads.
#[test]
fn run_share_without_a_server_url_exits_infra() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();

    bin()
        .args(["run", "--share"])
        .env_remove("DOMARINN_SERVER_URL")
        .current_dir(dir.path())
        .assert()
        .code(3)
        .stderr(predicate::str::contains("no server URL"));
}

/// The opt-out only means anything alongside `--share`; on its own it reads as
/// "tolerate a failure of something I never asked for", so clap rejects it.
#[test]
fn allow_share_failure_without_share_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();

    bin()
        .args(["run", "--allow-share-failure"])
        .current_dir(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--share"));
}

/// `ci-summary` renders the headline table for the latest run, defaulting to
/// `latest` with no argument.
#[test]
fn ci_summary_defaults_to_latest_and_tables_the_metrics() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();
    bin()
        .arg("ci-summary")
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("### domarinn run — s"))
        .stdout(predicate::str::contains("| Result | ✅ 1 passed |"))
        .stdout(predicate::str::contains("cases from cache"));
}

/// It is a reporter, not a gate: a failing run still exits 0 so a workflow step
/// that summarizes cannot change the verdict `run` already returned.
#[test]
fn ci_summary_exits_zero_on_a_failing_run() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("goodbye", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().code(1);
    bin()
        .arg("ci-summary")
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("**Failing:**"))
        .stdout(predicate::str::contains(
            "| Result | ❌ 0 passed, 1 failed |",
        ));
}

#[test]
fn ci_summary_writes_markdown_to_out_path() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();
    bin()
        .args(["ci-summary", "--out", "summary.md"])
        .current_dir(dir.path())
        .assert()
        .success();
    let md = std::fs::read_to_string(dir.path().join("summary.md")).unwrap();
    assert!(md.contains("| metric | value |"));
}

/// The key=value pairs a workflow reads back as step outputs. These replace the
/// action's old regex over results.xml and its grep over the markdown.
#[test]
fn ci_summary_writes_github_output_pairs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("goodbye", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().code(1);
    bin()
        .args(["ci-summary", "--github-output", "gh.txt"])
        .current_dir(dir.path())
        .assert()
        .success();

    let out = std::fs::read_to_string(dir.path().join("gh.txt")).unwrap();
    let pairs: std::collections::HashMap<&str, &str> =
        out.lines().filter_map(|l| l.split_once('=')).collect();
    assert_eq!(pairs.get("passed"), Some(&"0"));
    assert_eq!(pairs.get("failed"), Some(&"1"));
    assert_eq!(pairs.get("errored"), Some(&"0"));
    assert_eq!(pairs.get("failed-or-errored"), Some(&"1"));
    assert_eq!(pairs.get("total"), Some(&"1"));
    assert_eq!(pairs.get("pass-rate"), Some(&"0.0"));
    assert_eq!(pairs.get("regressed"), Some(&"0"));
    // Never shared, so the URL keys are present but empty rather than missing —
    // a workflow referencing a nonexistent output would otherwise get "".
    assert_eq!(pairs.get("run-url"), Some(&""));
}

/// `$GITHUB_OUTPUT` is honoured without a flag, which is how the action uses it.
#[test]
fn ci_summary_honours_the_github_output_env_var() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();
    bin()
        .arg("ci-summary")
        .env("GITHUB_OUTPUT", dir.path().join("env-out.txt"))
        .current_dir(dir.path())
        .assert()
        .success();
    let out = std::fs::read_to_string(dir.path().join("env-out.txt")).unwrap();
    assert!(out.contains("passed=1"));
    assert!(out.contains("cache-hit-rate="));
}

/// Appends rather than truncates: GitHub reuses one `$GITHUB_OUTPUT` file for
/// every step in a job, so clobbering it would drop earlier steps' outputs.
#[test]
fn ci_summary_appends_to_an_existing_github_output_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();
    std::fs::write(dir.path().join("gh.txt"), "earlier-step=kept\n").unwrap();
    bin()
        .args(["ci-summary", "--github-output", "gh.txt"])
        .current_dir(dir.path())
        .assert()
        .success();
    let out = std::fs::read_to_string(dir.path().join("gh.txt")).unwrap();
    assert!(out.contains("earlier-step=kept"));
    assert!(out.contains("passed=1"));
}

/// A shared run links to itself; the CI run URL comes from the workflow env.
#[test]
fn ci_summary_links_to_the_shared_run_and_the_ci_run() {
    let (url, server) = share_stub("200 OK", SHARE_OK);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();
    bin()
        .args(["run", "--share"])
        .env("DOMARINN_SERVER_URL", &url)
        // Set on the RUN, not on the summary: the engine records CI metadata
        // onto the run document, and `ci-summary` prefers what the run itself
        // recorded over the ambient environment. Setting these only at summary
        // time would assert against a fallback that never fires for a run that
        // knows its own origin.
        .env("GITHUB_ACTIONS", "true")
        .env("GITHUB_SERVER_URL", "https://github.com")
        .env("GITHUB_REPOSITORY", "acme/widgets")
        .env("GITHUB_RUN_ID", "42")
        .current_dir(dir.path())
        .assert()
        .success();
    server.join().unwrap();

    bin()
        .args(["ci-summary", "--github-output", "gh.txt"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "[View run](https://domarinn.test/runs/abc)",
        ))
        .stdout(predicate::str::contains(
            "[CI run](https://github.com/acme/widgets/actions/runs/42)",
        ));
    let out = std::fs::read_to_string(dir.path().join("gh.txt")).unwrap();
    assert!(out.contains("run-url=https://domarinn.test/runs/abc"));
}

/// With neither URL there is no links line at all — no empty brackets, no
/// dangling separator.
#[test]
fn ci_summary_omits_the_links_line_when_there_is_nothing_to_link() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();
    // `bin()` already clears the CI environment for both invocations, so the
    // run genuinely has no CI metadata to link to — which is the condition this
    // test is about.
    let out = bin()
        .arg("ci-summary")
        .current_dir(dir.path())
        .output()
        .unwrap();
    let md = String::from_utf8(out.stdout).unwrap();
    assert!(!md.contains("[View run]"));
    assert!(!md.contains("[CI run]"));
    assert!(!md.contains("]()"));
}

/// `--against` appends the baseline comparison and reports the regression count
/// as a step output.
#[test]
fn ci_summary_against_a_baseline_reports_regressions() {
    let dir = tempfile::tempdir().unwrap();
    run_to(dir.path(), "base.json", &suite("hello", "hello"));
    std::fs::write(dir.path().join("domarinn.yaml"), suite("goodbye", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().code(1);
    bin()
        .args([
            "ci-summary",
            "--against",
            "base.json",
            "--github-output",
            "gh.txt",
        ])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("### domarinn comparison"))
        .stdout(predicate::str::contains("| Newly failing | 1 |"));
    let out = std::fs::read_to_string(dir.path().join("gh.txt")).unwrap();
    assert!(out.contains("regressed=1"));
}

/// With a baseline present the diff already tables the newly-failing cases, so
/// the plain failure table would print the same rows a second time.
#[test]
fn ci_summary_against_a_baseline_does_not_repeat_the_failure_table() {
    let dir = tempfile::tempdir().unwrap();
    run_to(dir.path(), "base.json", &suite("hello", "hello"));
    std::fs::write(dir.path().join("domarinn.yaml"), suite("goodbye", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().code(1);
    let out = bin()
        .args(["ci-summary", "--against", "base.json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let md = String::from_utf8(out.stdout).unwrap();
    assert!(!md.contains("**Failing:**"), "got:\n{md}");
    assert!(md.contains("**Newly failing:**"));
}

/// An unresolvable run is a usage error, same as `share`.
#[test]
fn ci_summary_unresolvable_run_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    bin()
        .args(["ci-summary", "nosuchrun"])
        .current_dir(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "could not resolve run reference 'nosuchrun'",
        ));
}

/// An unreadable baseline must not sink the summary — the run's own numbers are
/// still worth reporting, and a missing `latest` on a first PR is routine.
#[test]
fn ci_summary_survives_an_unresolvable_baseline() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();
    bin()
        .args(["ci-summary", "--against", "nosuchbaseline"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("| metric | value |"));
}
