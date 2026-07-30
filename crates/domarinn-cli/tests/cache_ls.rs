//! E2E tests for `cache ls` and `cache show`.
//!
//! Both are driven through a real run rather than hand-written files, so the
//! fixtures are entries the engine actually wrote — which is the point of
//! having a second consumer of the projection at all: a shape only the web UI
//! reads is a shape that can quietly stop matching what the engine produces.

mod common;

use assert_cmd::prelude::*;
use common::{bin, suite};
use predicates::prelude::*;

/// Run a suite in `dir` so its cache has exactly one entry, and return the key.
fn seeded(dir: &std::path::Path) -> String {
    std::fs::write(dir.join("domarinn.yaml"), suite("hello", "hello")).unwrap();
    bin().arg("run").current_dir(dir).assert().success();

    let out = bin()
        .args(["cache", "ls", "--json"])
        .current_dir(dir)
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let first = stdout.lines().next().expect("one entry after a run");
    let row: serde_json::Value = serde_json::from_str(first).unwrap();
    row["key"].as_str().unwrap().to_string()
}

#[test]
fn cache_ls_lists_what_a_run_just_wrote() {
    let dir = tempfile::tempdir().unwrap();
    let key = seeded(dir.path());
    assert!(key.starts_with("sha256:"), "{key}");

    bin()
        .args(["cache", "ls"])
        .current_dir(dir.path())
        .assert()
        .success()
        // The key is abbreviated in the table; the full value is what --json is
        // for, and what `show` takes.
        .stdout(predicate::str::contains(&key[7..13]));
}

#[test]
fn cache_ls_reports_no_entries_rather_than_nothing() {
    let dir = tempfile::tempdir().unwrap();
    bin()
        .args(["cache", "ls"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("no entries"));
}

/// `--json` is what makes the local cache scriptable, so it emits one object
/// per line rather than an array — composable with `jq`, `head` and `grep`.
#[test]
fn cache_ls_json_is_one_object_per_line() {
    let dir = tempfile::tempdir().unwrap();
    seeded(dir.path());
    let out = bin()
        .args(["cache", "ls", "--json"])
        .current_dir(dir.path())
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    for line in stdout.lines().filter(|l| !l.is_empty()) {
        serde_json::from_str::<serde_json::Value>(line).expect("each line parses alone");
    }
}

#[test]
fn cache_show_prints_the_entry_a_run_wrote() {
    let dir = tempfile::tempdir().unwrap();
    let key = seeded(dir.path());
    bin()
        .args(["cache", "show", &key])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("--- output ---"))
        .stdout(predicate::str::contains("hello"));
}

/// Raw provider metadata is the largest member and the least often wanted, so
/// it is opt-in here exactly as it is on the HTTP detail route.
#[test]
fn cache_show_withholds_raw_metadata_unless_asked() {
    let dir = tempfile::tempdir().unwrap();
    let key = seeded(dir.path());
    bin()
        .args(["cache", "show", &key])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("--- provider metadata ---").not());
}

#[test]
fn cache_show_rejects_a_malformed_key_as_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    bin()
        .args(["cache", "show", "not-a-key"])
        .current_dir(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("expected sha256:"));
}

/// A well-formed key that is simply not here is an infrastructure failure, not
/// a usage error: the caller asked a sensible question and the answer is no.
#[test]
fn cache_show_reports_a_missing_entry_distinctly_from_a_bad_key() {
    let dir = tempfile::tempdir().unwrap();
    let absent = format!("sha256:{}", "ab".repeat(32));
    bin()
        .args(["cache", "show", &absent])
        .current_dir(dir.path())
        .assert()
        .code(3)
        .stderr(predicate::str::contains("no entry for"));
}
