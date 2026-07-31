//! Tests for run provenance — who ran a suite, where, and on what commit.
//!
//! The behaviour that changed: provenance is collected by the *engine*, so it
//! reaches the locally persisted `result.json`. Previously `runner.rs` hardcoded
//! `git: None, ci: None` and only `domarinn share` attached them, to a clone, on
//! the way out — so every stored run on disk claimed to know nothing about its
//! own origin.

mod common;

use assert_cmd::prelude::*;
use common::{bin, latest_run, suite};

fn write_suite(dir: &std::path::Path, body: String) {
    std::fs::write(dir.join("domarinn.yaml"), body).unwrap();
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A repo with one commit, and `.domarinn/` ignored so the run's own output
/// cannot make the worktree read as dirty.
fn init_repo(dir: &std::path::Path) {
    git(dir, &["init", "-b", "main"]);
    git(dir, &["config", "user.email", "test@example.invalid"]);
    git(dir, &["config", "user.name", "Test"]);
    std::fs::write(dir.join(".gitignore"), ".domarinn/\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-m", "initial"]);
}

#[test]
fn a_local_run_records_who_ran_it_and_on_what_build() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), suite("hello", "hello"));
    bin().arg("run").current_dir(dir.path()).assert().success();

    let origin = latest_run(dir.path())
        .origin
        .expect("a run records its origin");
    assert!(origin.actor.is_some(), "actor should be the OS username");
    assert!(origin.host.is_some(), "host should be the hostname");
    assert_eq!(origin.version.as_deref(), Some(env!("CARGO_PKG_VERSION")));
    assert_eq!(origin.redacted, None);
}

/// The core regression: git metadata now reaches the stored run. Before this,
/// `result.json` always had `git: null` unless the run was uploaded.
#[test]
fn a_local_run_records_git_metadata_without_sharing() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    write_suite(dir.path(), suite("hello", "hello"));
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-m", "add suite"]);

    bin().arg("run").current_dir(dir.path()).assert().success();

    let git_meta = latest_run(dir.path())
        .git
        .expect("a run inside a repo records git metadata");
    assert_eq!(git_meta.branch.as_deref(), Some("main"));
    assert_eq!(
        git_meta.commit.as_ref().map(|c| c.len()),
        Some(40),
        "commit should be the full sha"
    );
    assert!(!git_meta.dirty, "a committed, ignored-output tree is clean");
}

/// The CI regression: `actions/checkout` leaves a detached HEAD, so git alone
/// answers the literal `HEAD` for every run on a runner and that string was
/// being stored as the branch. The pull-request source branch is what a reviewer
/// means by "the branch", and only the environment knows it.
#[test]
fn a_github_pull_request_run_records_the_source_branch_not_the_merge_ref() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    write_suite(dir.path(), suite("hello", "hello"));
    // What a runner looks like mid-`pull_request`: HEAD is not on a branch.
    git(dir.path(), &["checkout", "--detach", "HEAD"]);

    bin()
        .arg("run")
        .env("GITHUB_ACTIONS", "true")
        .env("GITHUB_HEAD_REF", "feat/ci-branch")
        .env("GITHUB_REF", "refs/pull/42/merge")
        .current_dir(dir.path())
        .assert()
        .success();

    let git_meta = latest_run(dir.path()).git.expect("git metadata");
    assert_eq!(git_meta.branch.as_deref(), Some("feat/ci-branch"));
    assert!(git_meta.commit.is_some(), "the commit still comes from git");
}

/// A push build carries no head ref, and the branch is behind `refs/heads/`.
#[test]
fn a_github_push_run_records_the_ref_it_was_pushed_to() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    write_suite(dir.path(), suite("hello", "hello"));

    bin()
        .arg("run")
        .env("GITHUB_ACTIONS", "true")
        .env("GITHUB_HEAD_REF", "")
        .env("GITHUB_REF", "refs/heads/release/2026-07")
        .current_dir(dir.path())
        .assert()
        .success();

    assert_eq!(
        latest_run(dir.path()).git.and_then(|g| g.branch).as_deref(),
        Some("release/2026-07")
    );
}

/// Detached with nothing in the environment to ask: no branch beats a branch
/// named `HEAD`, which is what every filter and the retention key would store.
#[test]
fn a_detached_checkout_records_no_branch_rather_than_the_word_head() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    write_suite(dir.path(), suite("hello", "hello"));
    git(dir.path(), &["checkout", "--detach", "HEAD"]);

    bin().arg("run").current_dir(dir.path()).assert().success();

    let git_meta = latest_run(dir.path())
        .git
        .expect("a detached checkout is still a repo");
    assert_eq!(git_meta.branch, None);
    assert!(git_meta.commit.is_some());
}

/// An uncommitted change is recorded, because "this result came from a tree that
/// is not reproducible" is exactly the trust signal a shared board needs.
#[test]
fn an_uncommitted_change_marks_the_run_dirty() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    write_suite(dir.path(), suite("hello", "hello"));

    bin().arg("run").current_dir(dir.path()).assert().success();

    assert!(latest_run(dir.path()).git.expect("git metadata").dirty);
}

/// Outside a repo there is nothing to record, and nothing may be invented.
#[test]
fn a_run_outside_a_repository_records_no_git_metadata() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), suite("hello", "hello"));
    bin().arg("run").current_dir(dir.path()).assert().success();

    assert!(latest_run(dir.path()).git.is_none());
}

/// `DOMARINN_PROVENANCE=off` must leave no trace at all — not an empty object,
/// no key. Asserted on the raw JSON because an absent key and a present-but-null
/// one deserialize identically but hash differently.
#[test]
fn provenance_off_writes_no_origin_key_at_all() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    write_suite(dir.path(), suite("hello", "hello"));

    bin()
        .arg("run")
        .env("DOMARINN_PROVENANCE", "off")
        .current_dir(dir.path())
        .assert()
        .success();

    let raw = std::fs::read_to_string(
        dir.path()
            .join(".domarinn/runs")
            .join(common::latest_id(dir.path()))
            .join("result.json"),
    )
    .unwrap();
    assert!(!raw.contains("\"origin\""));
    assert!(!raw.contains("\"git\""));
    assert!(!raw.contains("\"ci\""));
}

/// `--no-provenance` drops identity only, and says so. Keeping git and the
/// version is the point: they are not personal, and they are what makes a run
/// reproducible.
#[test]
fn no_provenance_drops_identity_but_keeps_git_and_says_it_was_redacted() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    write_suite(dir.path(), suite("hello", "hello"));

    bin()
        .args(["run", "--no-provenance"])
        .current_dir(dir.path())
        .assert()
        .success();

    let run = latest_run(dir.path());
    let origin = run.origin.expect("still records an origin");
    assert_eq!(origin.actor, None);
    assert_eq!(origin.host, None);
    assert_eq!(origin.redacted, Some(true));
    assert!(origin.version.is_some());
    assert!(run.git.is_some(), "git is not identity and is kept");
}

/// The environment sets policy; the flag may only tighten it. A user cannot
/// re-enable identity that the image or machine turned off.
#[test]
fn the_flag_cannot_widen_what_the_environment_suppressed() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), suite("hello", "hello"));

    bin()
        .args(["run", "--no-provenance"])
        .env("DOMARINN_PROVENANCE", "off")
        .current_dir(dir.path())
        .assert()
        .success();

    assert!(latest_run(dir.path()).origin.is_none());
}

#[test]
fn note_is_recorded_on_the_run() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), suite("hello", "hello"));

    bin()
        .args(["run", "--note", "trying temperature 0.3"])
        .current_dir(dir.path())
        .assert()
        .success();

    assert_eq!(
        latest_run(dir.path())
            .origin
            .and_then(|o| o.note)
            .as_deref(),
        Some("trying temperature 0.3")
    );
}

/// With no `--note`, the suite's `description` fills the slot — a field that was
/// parsed into `config_snapshot` and read by nothing.
#[test]
fn the_suite_description_becomes_the_note_when_no_flag_is_given() {
    let dir = tempfile::tempdir().unwrap();
    let body = suite("hello", "hello").replace(
        "version: 1",
        "version: 1\ndescription: nightly smoke coverage",
    );
    write_suite(dir.path(), body);

    bin().arg("run").current_dir(dir.path()).assert().success();

    assert_eq!(
        latest_run(dir.path())
            .origin
            .and_then(|o| o.note)
            .as_deref(),
        Some("nightly smoke coverage")
    );
}
