//! E2E tests for the diff, view, import, and gen-types commands.
//!
//! The `cache` subcommands live in `cache_cmd.rs`: they carry their own
//! two-tier fixtures, and this file was within a few lines of the repo's
//! per-file ratchet (`domarinn-core/tests/file_length.rs`).

mod common;

use assert_cmd::prelude::*;
use common::{bin, latest_id, run_to, suite};
use domarinn_core::diff::diff_runs;
use domarinn_core::result::RunResult;
use predicates::prelude::*;

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
    // First (passing) run establishes the baseline persisted under .domarinn.
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();
    // Now a regressed suite compared against the latest run.
    std::fs::write(dir.path().join("domarinn.yaml"), suite("goodbye", "hello")).unwrap();
    bin()
        .args(["run", "--against", "latest"])
        .current_dir(dir.path())
        .assert()
        .code(1);
}

/// A suite carrying a named prompt, so two variants that differ only in the
/// prompt template drive a config-digest change while keeping the same case_key
/// (the prompt id is unchanged).
fn prompt_suite(template: &str) -> String {
    format!(
        r#"
version: 1
suite: s
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{{\"output\":\"hello\"}}'"]
prompts:
  - id: greeting
    template: "{template}"
tests:
  - id: t1
    vars: {{}}
    assert:
      - {{type: contains, value: "hello"}}
"#
    )
}

/// A `{"output": "..."}` JSON payload whose output is 70 lines each prefixed with
/// `prefix` (e.g. `a0..a69`). Written to a file the exec provider `cat`s, so the
/// resulting output text carries real newlines (a big multi-line diff).
fn multiline_json(prefix: char) -> String {
    let lines: Vec<String> = (0..70).map(|i| format!("{prefix}{i}")).collect();
    format!("{{\"output\":\"{}\"}}", lines.join("\\n"))
}

/// The `diff` table renders a real unified output diff (base line removed, head
/// line added), a score delta, and preserves the regression exit code. Colored
/// only under `--color always`; plain when piped.
#[test]
fn diff_table_shows_unified_output_diff_and_score() {
    let dir = tempfile::tempdir().unwrap();
    run_to(dir.path(), "base.json", &suite("hello", "hello"));
    run_to(dir.path(), "head.json", &suite("goodbye", "hello"));

    let plain = bin()
        .args(["diff", "base.json", "head.json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(plain.status.code(), Some(1), "regression still exits 1");
    let stdout = String::from_utf8_lossy(&plain.stdout);
    assert!(
        stdout.contains("-hello"),
        "base output line marked removed; got:\n{stdout}"
    );
    assert!(
        stdout.contains("+goodbye"),
        "head output line marked added; got:\n{stdout}"
    );
    assert!(
        stdout.contains("score 1.00 → 0.00"),
        "score arrow present; got:\n{stdout}"
    );
    assert!(
        !stdout.contains('\u{1b}'),
        "piped default carries no escapes; got:\n{stdout}"
    );

    let colored = bin()
        .args(["diff", "base.json", "head.json", "--color", "always"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        colored.stdout.contains(&0x1b),
        "colored output carries ANSI escapes"
    );
    let ctext = String::from_utf8_lossy(&colored.stdout);
    assert!(
        ctext.contains("-hello") && ctext.contains("+goodbye"),
        "diff line bytes survive inside the escapes; got:\n{ctext}"
    );
}

/// Two suites differing only in a prompt template produce a `config changed:`
/// line plus a prompts-section diff.
#[test]
fn diff_reports_config_and_prompt_drift() {
    let dir = tempfile::tempdir().unwrap();
    run_to(dir.path(), "base.json", &prompt_suite("Hi there"));
    run_to(dir.path(), "head.json", &prompt_suite("Hello there"));
    let out = bin()
        .args(["diff", "base.json", "head.json", "--color", "never"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("config changed:"),
        "digest drift noted; got:\n{stdout}"
    );
    assert!(
        stdout.contains("Hello there"),
        "the prompts diff surfaces the new template; got:\n{stdout}"
    );
}

/// `--diffs none` keeps the regression marker but suppresses the output hunks.
#[test]
fn diff_diffs_none_suppresses_output_hunks() {
    let dir = tempfile::tempdir().unwrap();
    run_to(dir.path(), "base.json", &suite("hello", "hello"));
    run_to(dir.path(), "head.json", &suite("goodbye", "hello"));
    let out = bin()
        .args(["diff", "base.json", "head.json", "--diffs", "none"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("REGRESS"),
        "the transition marker is still shown; got:\n{stdout}"
    );
    assert!(
        !stdout.contains("+goodbye"),
        "no output hunks under --diffs none; got:\n{stdout}"
    );
}

/// The per-case render cap truncates a large diff with a `--full` hint; `--full`
/// removes the cap and shows every line.
#[test]
fn diff_full_uncaps_a_large_output_diff() {
    let dir = tempfile::tempdir().unwrap();
    let payload = dir.path().join("payload.txt");
    let payload_str = payload.to_str().unwrap();
    let suite_body = format!(
        r#"
version: 1
suite: s
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; cat {payload_str}"]
tests:
  - id: t1
    vars: {{}}
    assert:
      - {{type: contains, value: "a0"}}
"#
    );
    std::fs::write(dir.path().join("domarinn.yaml"), &suite_body).unwrap();

    std::fs::write(&payload, multiline_json('a')).unwrap();
    bin()
        .args([
            "run",
            "--format",
            "json",
            "--out",
            "base.json",
            "--no-cache",
        ])
        .current_dir(dir.path())
        .output()
        .expect("base run executes");
    std::fs::write(&payload, multiline_json('b')).unwrap();
    bin()
        .args([
            "run",
            "--format",
            "json",
            "--out",
            "head.json",
            "--no-cache",
        ])
        .current_dir(dir.path())
        .output()
        .expect("head run executes");

    let capped = bin()
        .args(["diff", "base.json", "head.json", "--color", "never"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let capped = String::from_utf8_lossy(&capped.stdout);
    assert!(
        capped.contains("more diff lines (--full to show)"),
        "capped diff shows the hint; got tail:\n{}",
        capped.lines().rev().take(4).collect::<Vec<_>>().join("\n")
    );
    assert!(
        !capped.contains("+b69"),
        "the last changed line is hidden when capped"
    );

    let full = bin()
        .args([
            "diff",
            "base.json",
            "head.json",
            "--color",
            "never",
            "--full",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let full = String::from_utf8_lossy(&full.stdout);
    assert!(
        !full.contains("more diff lines"),
        "no cap hint under --full"
    );
    assert!(
        full.contains("+b69"),
        "every changed line is shown under --full"
    );
}

/// `diff --format md` carries a ```diff fenced block and the newly-failing score
/// column.
#[test]
fn diff_format_md_has_diff_fence_and_score_column() {
    let dir = tempfile::tempdir().unwrap();
    run_to(dir.path(), "base.json", &suite("hello", "hello"));
    run_to(dir.path(), "head.json", &suite("goodbye", "hello"));
    bin()
        .args(["diff", "base.json", "head.json", "--format", "md"])
        .current_dir(dir.path())
        .assert()
        .code(1)
        .stdout(predicate::str::contains("```diff"))
        .stdout(predicate::str::contains("| test | score |"));
}

/// The JSON golden: `diff --format json` must be byte-identical to a raw
/// `to_string_pretty(&diff_runs(base, head))`, guarding the machine wire type
/// against accidental enrichment by the new table/markdown features.
#[test]
fn diff_json_is_byte_identical_to_direct_diff_runs() {
    let dir = tempfile::tempdir().unwrap();
    run_to(dir.path(), "base.json", &suite("hello", "hello"));
    run_to(dir.path(), "head.json", &suite("goodbye", "hello"));

    let base: RunResult =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("base.json")).unwrap())
            .unwrap();
    let head: RunResult =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("head.json")).unwrap())
            .unwrap();
    let golden = serde_json::to_string_pretty(&diff_runs(&base, &head)).unwrap();

    let out = bin()
        .args(["diff", "base.json", "head.json", "--format", "json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        stdout.trim_end_matches('\n'),
        golden,
        "diff --format json drifted from the raw diff_runs serialization"
    );
}

#[test]
fn view_latest_renders() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();
    bin()
        .args(["view", "latest"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS"));
}

/// A suite with one passing and one failing test over the same provider output.
const MIXED_SUITE: &str = r#"
version: 1
suite: mixed
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"hello world\"}'"]
tests:
  - id: goodcase
    vars: {}
    assert:
      - {type: contains, value: "hello"}
  - id: badcase
    vars: {}
    assert:
      - {type: contains, value: "goodbye"}
"#;

/// `view --failed` renders only the failing/errored cases, but the footer still
/// summarizes the whole run and a `showing N of M` line records the filter.
#[test]
fn view_failed_shows_only_failing_cases_and_count() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), MIXED_SUITE).unwrap();
    // The mixed run exits non-zero (one failure); we only need it persisted.
    bin()
        .arg("run")
        .current_dir(dir.path())
        .output()
        .expect("run executes");
    let output = bin()
        .args(["view", "latest", "--failed"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("FAIL"),
        "failing case shown; got:\n{stdout}"
    );
    assert!(
        stdout.contains("badcase"),
        "the failing test name is shown; got:\n{stdout}"
    );
    assert!(
        !stdout.contains("goodcase"),
        "the passing case must be filtered out; got:\n{stdout}"
    );
    // Footer describes the whole run; the showing line records the filter.
    assert!(stdout.contains("1 passed, 1 failed"));
    assert!(
        stdout.contains("showing 1 failed/errored of 2 cases"),
        "expected the showing line; got:\n{stdout}"
    );
}

/// `view --format md` emits the markdown run summary header.
#[test]
fn view_format_md_has_markdown_header() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();
    bin()
        .args(["view", "latest", "--format", "md"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("### domarinn run — s"))
        .stdout(predicate::str::contains("| metric | value |"))
        .stdout(predicate::str::contains("| Result | ✅ 1 passed |"))
        .stdout(predicate::str::contains("| Pass rate | 100.0%"))
        .stdout(predicate::str::contains("cases from cache"));
}

/// The junit machine format is byte-identical whether or not `--color always` is
/// passed. Rendered from one persisted run so latency is fixed across the two
/// invocations.
#[test]
fn view_junit_identical_with_and_without_color() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();
    let plain = bin()
        .args(["view", "latest", "--format", "junit"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let colored = bin()
        .args(["view", "latest", "--format", "junit", "--color", "always"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(plain.status.success() && colored.status.success());
    assert_eq!(
        plain.stdout, colored.stdout,
        "junit output must be identical regardless of --color"
    );
    assert!(!colored.stdout.contains(&0x1b));
}

/// Persist the mixed run (one pass, one fail) and return its temp dir.
fn mixed_run() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), MIXED_SUITE).unwrap();
    // The mixed run exits non-zero (one failing case); we only need it persisted.
    bin()
        .arg("run")
        .current_dir(dir.path())
        .output()
        .expect("run executes");
    dir
}

/// `view --case <test-id>` resolves the case and dumps its full detail: the
/// identity line, the assert kind, and the (untruncated) output.
#[test]
fn view_case_by_test_id_shows_asserts_and_output() {
    let dir = mixed_run();
    let output = bin()
        .args(["view", "latest", "--case", "goodcase", "--color", "never"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("PASS goodcase"), "header; got:\n{stdout}");
    assert!(
        stdout.contains("provider p · test goodcase"),
        "identity line; got:\n{stdout}"
    );
    assert!(
        stdout.contains("contains"),
        "the assert kind is shown; got:\n{stdout}"
    );
    assert!(
        stdout.contains("hello world"),
        "the full output is shown; got:\n{stdout}"
    );
}

/// A test id shared by both providers fans out to a cross-provider view. Here the
/// two tests differ, but a bogus selector still exits 2 with suggestions.
#[test]
fn view_case_bogus_selector_exits_two_with_suggestions() {
    let dir = mixed_run();
    let output = bin()
        .args(["view", "latest", "--case", "nope-not-here"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "usage exit on no match");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no case matches"),
        "error names the miss; got:\n{stderr}"
    );
    assert!(
        stderr.contains("closest cases:"),
        "suggestions offered; got:\n{stderr}"
    );
    // The real case names are proposed as candidates.
    assert!(
        stderr.contains("goodcase") || stderr.contains("badcase"),
        "a real case is suggested; got:\n{stderr}"
    );
}

/// `--case ... --format json` is always a JSON array, even for a single match, so
/// `jq` consumers need no special-casing.
#[test]
fn view_case_json_is_always_an_array() {
    let dir = mixed_run();
    let output = bin()
        .args(["view", "latest", "--case", "goodcase", "--format", "json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let arr = v.as_array().expect("top level is an array");
    assert_eq!(arr.len(), 1, "one matched case");
    assert_eq!(arr[0]["cell"]["test_id"], serde_json::json!("goodcase"));
    // Machine format carries no escapes even if a TTY were assumed.
    assert!(!output.stdout.contains(&0x1b));
}

/// `--case` intersects with `--failed`: a passing case is filtered out (a note is
/// shown), while a failing one still renders.
#[test]
fn view_case_intersects_failed_filter() {
    let dir = mixed_run();
    // goodcase passes, so --failed leaves nothing.
    let passing = bin()
        .args([
            "view", "latest", "--case", "goodcase", "--failed", "--color", "never",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(passing.status.success());
    let out = String::from_utf8_lossy(&passing.stdout);
    assert!(
        out.contains("no matching failed/errored cases"),
        "passing case filtered out; got:\n{out}"
    );
    // badcase fails, so --failed keeps it.
    let failing = bin()
        .args([
            "view", "latest", "--case", "badcase", "--failed", "--color", "never",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(failing.status.success());
    let out = String::from_utf8_lossy(&failing.stdout);
    assert!(
        out.contains("FAIL badcase"),
        "failing case kept; got:\n{out}"
    );
}

/// `--raw` on a run whose cases carry no stored raw metadata (the exec provider
/// records none) prints the explicit `not recorded` note rather than nothing.
#[test]
fn view_case_raw_on_v1_run_notes_not_recorded() {
    let dir = mixed_run();
    let output = bin()
        .args([
            "view", "latest", "--case", "goodcase", "--raw", "--color", "never",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("raw: not recorded"),
        "raw absence is explicit; got:\n{stdout}"
    );
}

/// `--case` with `--format junit` is nonsensical and exits 2.
#[test]
fn view_case_junit_format_is_rejected() {
    let dir = mixed_run();
    bin()
        .args(["view", "latest", "--case", "goodcase", "--format", "junit"])
        .current_dir(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("does not support --format junit"));
}

#[test]
fn runs_lists_stored_runs_newest_first_with_latest_marker() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();
    let first_id = latest_id(dir.path());
    // A second run mints a distinct id and becomes the newest + `latest`.
    bin().arg("run").current_dir(dir.path()).assert().success();
    let second_id = latest_id(dir.path());
    assert_ne!(first_id, second_id, "each run mints a distinct id");

    let output = bin()
        .args(["runs", "--color", "never"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let pos_second = stdout
        .find(&second_id)
        .unwrap_or_else(|| panic!("second id listed; got:\n{stdout}"));
    let pos_first = stdout
        .find(&first_id)
        .unwrap_or_else(|| panic!("first id listed; got:\n{stdout}"));
    assert!(
        pos_second < pos_first,
        "newest run listed first; got:\n{stdout}"
    );
    assert!(
        stdout.contains(&format!("* {second_id}")),
        "latest marked with *; got:\n{stdout}"
    );
    assert!(
        stdout.contains("2 runs in .domarinn/runs"),
        "footer counts both runs; got:\n{stdout}"
    );
}

#[test]
fn runs_json_reports_path_and_latest_flag() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();
    bin().arg("run").current_dir(dir.path()).assert().success();
    let second_id = latest_id(dir.path());

    let output = bin()
        .args(["runs", "--json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let arr = v.as_array().expect("json array");
    assert_eq!(arr.len(), 2, "both runs present");
    // Newest first, and it is the `latest` pointer target.
    assert_eq!(arr[0]["run_id"].as_str().unwrap(), second_id);
    assert_eq!(arr[0]["latest"], serde_json::json!(true));
    assert_eq!(arr[1]["latest"], serde_json::json!(false));
    // `path` points at the stored result.json.
    let path0 = arr[0]["path"].as_str().unwrap();
    assert!(path0.contains("result.json"), "path present; got {path0}");
    assert!(
        path0.contains(&second_id),
        "path names the run; got {path0}"
    );
    // `summary` is embedded verbatim.
    assert!(arr[0]["summary"]["total"].is_number());
}

/// `share` accepts a bare run id from `.domarinn/runs`, like `view` and `diff`.
/// With no server configured, resolution still succeeds and the missing server
/// surfaces as the best-effort upload warning (exit 0) — proving the id
/// resolved to the stored run rather than being read as a file path.
#[test]
fn share_accepts_bare_run_id() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();
    let id = latest_id(dir.path());
    bin()
        .args(["share", &id])
        .env_remove("DOMARINN_SERVER_URL")
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("no server URL"));
}

/// An unresolvable reference reports the reference itself, not a file error.
#[test]
fn share_unresolvable_reference_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    bin()
        .args(["share", "nosuchrun"])
        .current_dir(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "could not resolve run reference 'nosuchrun'",
        ));
}

/// With no argument, `share` still targets the latest stored run.
#[test]
fn share_no_args_shares_the_latest_run() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();
    bin()
        .arg("share")
        .env_remove("DOMARINN_SERVER_URL")
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("no server URL"));
}

/// An explicit result.json path is still accepted (the pre-run-id contract).
#[test]
fn share_result_json_path_still_accepted() {
    let dir = tempfile::tempdir().unwrap();
    run_to(dir.path(), "out.json", &suite("hello", "hello"));
    bin()
        .args(["share", "out.json"])
        .env_remove("DOMARINN_SERVER_URL")
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("no server URL"));
}

#[test]
fn runs_limit_one_shows_a_single_row() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();
    bin().arg("run").current_dir(dir.path()).assert().success();
    let output = bin()
        .args(["runs", "-n", "1", "--json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 1, "limit caps the rows");
}

#[test]
fn runs_empty_dir_is_friendly_and_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    bin()
        .args(["runs"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("no runs"));
}

#[test]
fn runs_remote_without_server_exits_usage() {
    let dir = tempfile::tempdir().unwrap();
    bin()
        .args(["runs", "--remote"])
        .env_remove("DOMARINN_SERVER_URL")
        .current_dir(dir.path())
        .assert()
        .code(2);
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
    // The emitted YAML must itself validate as a domarinn suite.
    std::fs::write(dir.path().join("domarinn.yaml"), &output.stdout).unwrap();
    bin()
        .args(["validate", "domarinn.yaml"])
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
    // The server DTO layer (response + request-body TS derives) must be
    // exported alongside core's result/diff types into the same directory.
    assert!(
        out.join("RunListResponse.ts").exists(),
        "RunListResponse.ts (a server response DTO) should exist"
    );
    assert!(
        out.join("CreateUserBody.ts").exists(),
        "CreateUserBody.ts (a server request body) should exist"
    );
}
