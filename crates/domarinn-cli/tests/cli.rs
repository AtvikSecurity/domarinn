//! End-to-end CLI tests driving the built `domarinn` binary.

use std::process::Command;

use assert_cmd::prelude::*;
use predicates::prelude::*;

fn bin() -> Command {
    Command::cargo_bin("domarinn").unwrap()
}

fn write_suite(dir: &std::path::Path, body: &str) {
    std::fs::write(dir.join("domarinn.yaml"), body).unwrap();
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
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/12-render-health");
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

/// A run whose every case was skipped graded nothing, and a green gate there
/// says the suite *ran*, not that it passed. The sibling guard
/// (`RunError::NothingToRun`) covers the empty-cells end of the same hole; this
/// is the end where cells existed and every response matched a
/// `skip_on_empty_reason`, which is what a model regression that empties every
/// answer looks like.
#[test]
fn a_run_where_every_case_was_skipped_does_not_exit_zero() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(
        dir.path(),
        r#"
version: 1
suite: all-skipped
runner:
  skip_on_empty_reason: [tool_use_only]
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"\",\"empty_reason\":\"tool_use_only\"}'"]
tests:
  - id: a
    vars: {}
    assert: [{type: contains, value: "anything"}]
  - id: b
    vars: {}
    assert: [{type: contains, value: "anything"}]
"#,
    );
    bin()
        .arg("run")
        .current_dir(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("nothing was graded"));
}

/// …and one graded case is enough to make the run mean something again, so the
/// guard does not fire on the ordinary mixed suite `skip` exists to serve.
#[test]
fn a_partially_skipped_run_still_exits_on_its_verdicts() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(
        dir.path(),
        r#"
version: 1
suite: some-skipped
runner:
  skip_on_empty_reason: [tool_use_only]
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"hello\"}'"]
  - id: q
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"\",\"empty_reason\":\"tool_use_only\"}'"]
tests:
  - id: a
    vars: {}
    assert: [{type: contains, value: "hello"}]
"#,
    );
    bin().arg("run").current_dir(dir.path()).assert().success();
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
    assert_eq!(value["schema_version"], 2);
    assert_eq!(value["summary"]["passed"], 1);
}

/// `--color always` forces ANSI even when stdout is a pipe (assert_cmd is never
/// a TTY), so the human table carries escape sequences.
#[test]
fn run_color_always_emits_ansi_when_piped() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), PASSING_SUITE);
    let output = bin()
        .args(["run", "--color", "always"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains('\x1b'),
        "--color always should emit ANSI escapes; got:\n{stdout}"
    );
    // The status token survives byte-for-byte inside the escapes.
    assert!(stdout.contains("PASS"));
}

/// The default over a pipe is no color: assert_cmd is not a TTY and neither
/// NO_COLOR nor CLICOLOR_FORCE is set.
#[test]
fn run_default_piped_has_no_ansi() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), PASSING_SUITE);
    let output = bin()
        .arg("run")
        .current_dir(dir.path())
        .env_remove("CLICOLOR_FORCE")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains('\x1b'),
        "piped default output must be plain; got:\n{stdout}"
    );
}

/// `NO_COLOR` disables color under `--color auto` even if a TTY were present.
#[test]
fn run_no_color_env_disables_ansi() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), PASSING_SUITE);
    let output = bin()
        .args(["run", "--color", "auto"])
        .current_dir(dir.path())
        .env("NO_COLOR", "1")
        .env_remove("CLICOLOR_FORCE")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains('\x1b'),
        "NO_COLOR must suppress ANSI; got:\n{stdout}"
    );
}

/// The structural guarantee: even with `--color always`, the JSON machine format
/// carries no escapes and still parses.
#[test]
fn run_color_always_json_is_pure_json() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), PASSING_SUITE);
    let output = bin()
        .args(["run", "--color", "always", "--format", "json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        !output.stdout.contains(&0x1b),
        "machine JSON must never contain ANSI escapes even with --color always"
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["summary"]["passed"], 1);
}

/// File output forces color off regardless of `--color always`: a file must
/// never carry terminal escapes.
#[test]
fn run_out_file_color_always_has_no_ansi() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), PASSING_SUITE);
    let out = dir.path().join("table.txt");
    bin()
        .args(["run", "--color", "always", "--out"])
        .arg(&out)
        .current_dir(dir.path())
        .assert()
        .success();
    let bytes = std::fs::read(&out).unwrap();
    assert!(
        !bytes.contains(&0x1b),
        "--out file must be plain even with --color always"
    );
    assert!(String::from_utf8_lossy(&bytes).contains("PASS"));
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
    let latest = dir.path().join(".domarinn").join("runs").join("latest");
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
        .env_remove("DOMARINN_SERVER_URL")
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

/// A deprecation nobody sees is a deprecation that never happened: both knobs
/// this release supersedes have to say so on the run that used them, and the run
/// itself still has to pass — a warning is advice, not a failure.
#[test]
fn deprecated_cache_knobs_warn_on_stderr_without_failing_the_run() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(
        dir.path(),
        r#"
version: 1
suite: smoke
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"hello world\"}'"]
cache:
  backend: http
  grader: true
tests:
  - id: greet
    vars: {}
    assert:
      - {type: contains, value: "hello"}
"#,
    );
    let output = bin()
        .arg("run")
        .current_dir(dir.path())
        .env_remove("DOMARINN_SERVER_URL")
        .output()
        .unwrap();
    assert!(output.status.success(), "a deprecation must not fail a run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`backend: http` is a deprecated alias for `layered`"),
        "the alias warning should appear on stderr; got:\n{stderr}"
    );
    assert!(
        stderr.contains("cache.grader is deprecated; use --no-grader-cache"),
        "the cache.grader warning should appear on stderr; got:\n{stderr}"
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
        .env_remove("DOMARINN_SERVER_URL")
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
        .env_remove("DOMARINN_SERVER_URL")
        .output()
        .unwrap();
    assert!(output.status.success(), "cache-fallback run should pass");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("using local disk"),
        "RUST_LOG=off must suppress the cache WARN diagnostic; got:\n{stderr}"
    );
}

/// Regression: `domarinn server --data-dir` is bound to `DOMARINN_DATA_DIR`
/// (documented in server.md / cli.md), so the env var selects the state
/// directory. We point the env at an *unopenable* path (a DB under a regular
/// file) and assert the server fails fast referencing THAT path — proving it
/// honored the env var instead of silently using the compiled-in `/data`
/// default. Without the `env = "DOMARINN_DATA_DIR"` binding on the arg, the
/// server ignores the env and this test fails.
#[test]
fn server_honors_data_dir_env_var() {
    let dir = tempfile::tempdir().unwrap();
    // A regular file where a directory is expected: opening a SQLite DB beneath
    // it fails immediately (ENOTDIR), so the server exits before it ever binds.
    let blocker = dir.path().join("not-a-dir");
    std::fs::write(&blocker, b"x").unwrap();
    let data_dir = blocker.join("state");
    let output = bin()
        .arg("server")
        .env("DOMARINN_DATA_DIR", &data_dir)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "server should fail fast on an unopenable data dir"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let marker = data_dir.to_string_lossy();
    assert!(
        stderr.contains(marker.as_ref()),
        "error should reference the env-provided data dir ({marker}); got:\n{stderr}"
    );
    assert!(
        !stderr.contains("/data/domarinn.db"),
        "server must not fall back to the /data default when DOMARINN_DATA_DIR is set; got:\n{stderr}"
    );
}

/// Over a pipe (assert_cmd is not a TTY) the live progress bar must be fully
/// hidden: no carriage returns and no ANSI escapes leak onto stderr, so CI logs
/// and captured output stay clean. This is the non-TTY silence guarantee.
#[test]
fn run_progress_bar_is_hidden_over_a_pipe() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), PASSING_SUITE);
    let output = bin().arg("run").current_dir(dir.path()).output().unwrap();
    assert!(output.status.success());
    let stderr = &output.stderr;
    assert!(
        !stderr.contains(&b'\r'),
        "a hidden bar must not emit carriage returns; got stderr:\n{}",
        String::from_utf8_lossy(stderr)
    );
    assert!(
        !stderr.contains(&0x1b),
        "a hidden bar must not emit ANSI escapes; got stderr:\n{}",
        String::from_utf8_lossy(stderr)
    );
}

/// `--no-progress` is accepted and is equally silent over a pipe (the flag is an
/// opt-out that must never make output noisier).
#[test]
fn run_no_progress_flag_is_accepted_and_silent() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), PASSING_SUITE);
    let output = bin()
        .args(["run", "--no-progress"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        !output.stderr.contains(&b'\r') && !output.stderr.contains(&0x1b),
        "--no-progress stderr must carry no bar artifacts; got:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// stdout purity is independent of `--no-progress`: the primary (table) output
/// is byte-identical with and without the flag, so the progress bar can never
/// perturb the machine-facing stream.
///
/// Both runs pass `--no-cache`. Without it the second run is a cache hit and
/// its stats line legitimately gains a "1 cache hits" segment — a difference
/// caused by the cache, not by the progress bar, which would make this test
/// fail for a reason it is not about. It only ever passed because `exec`
/// providers were uncached by default.
#[test]
fn run_stdout_is_byte_identical_with_and_without_no_progress() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), PASSING_SUITE);
    let with_bar = bin()
        .args(["run", "--no-cache"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let no_bar = bin()
        .args(["run", "--no-progress", "--no-cache"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(with_bar.status.success() && no_bar.status.success());
    assert_eq!(
        with_bar.stdout, no_bar.stdout,
        "stdout must be byte-identical regardless of the progress bar"
    );
}

/// A suite whose single case fans out over a 2x2 matrix. `list tests` must
/// enumerate the expanded, deterministically-ordered cell ids.
const MATRIX_SUITE: &str = r#"
version: 1
suite: matrix
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"ok\"}'"]
tests:
  - id: greet
    matrix:
      style: [terse, warm]
      temperature: [0, 1]
    assert:
      - {type: contains, value: "ok"}
"#;

#[test]
fn list_tests_enumerates_expanded_matrix_ids() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), MATRIX_SUITE);
    bin()
        .args(["list", "tests"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("greet[style=terse,temperature=0]"))
        .stdout(predicate::str::contains("greet[style=warm,temperature=1]"));
}

/// One inline case plus a generator that produces two more. `sh` is the
/// command, so the generator's unnamed case takes the `sh/0` stem-and-index id.
const GENERATOR_SUITE: &str = r#"
version: 1
suite: generated
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"ok\"}'"]
tests:
  - id: inline
    assert:
      - {type: contains, value: "ok"}
  - generator:
      command: ["sh", "-c", "cat >/dev/null; printf '{\"tests\":[{\"vars\":{\"x\":\"1\"}},{\"id\":\"named\",\"vars\":{\"x\":\"2\"}}]}'"]
"#;

/// Without the flag the listing is the inline cases alone — but it must say so,
/// and say how to get the rest, or a generator-driven suite looks like a
/// one-case suite.
#[test]
fn list_tests_points_at_the_flag_that_runs_generators() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), GENERATOR_SUITE);
    bin()
        .args(["list", "tests"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("inline"))
        .stdout(predicate::str::contains("named").not())
        .stderr(predicate::str::contains("--generators"));
}

#[test]
fn list_tests_with_generators_enumerates_their_produced_ids() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), GENERATOR_SUITE);
    bin()
        .args(["list", "tests", "--generators"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("inline"))
        .stdout(predicate::str::contains("sh/0"))
        .stdout(predicate::str::contains("named"));
}

/// The note is a human aid, so it stays off the machine-readable path — but the
/// generated ids must reach `--json`, which is how a filter target gets scripted.
#[test]
fn list_tests_json_carries_generated_ids_without_the_note() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), GENERATOR_SUITE);
    let out = bin()
        .args(["list", "tests", "--generators", "--json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let ids: Vec<String> = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(ids, vec!["inline", "sh/0", "named"]);
    assert!(
        String::from_utf8_lossy(&out.stderr).is_empty(),
        "no note on the JSON path"
    );
}

/// A generator whose output is malformed is the suite's problem, not the
/// harness's — exit 2, matching what a run does with the same generator.
#[test]
fn list_tests_with_generators_reports_malformed_output_as_a_config_error() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(
        dir.path(),
        r#"
version: 1
suite: generated
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"ok\"}'"]
tests:
  - generator:
      command: ["sh", "-c", "cat >/dev/null; printf '{\"nope\":[]}'"]
"#,
    );
    bin()
        .args(["list", "tests", "--generators"])
        .current_dir(dir.path())
        .assert()
        .code(2);
}

/// A suite whose right answer is a tool call, not prose. The exec child returns
/// an empty output plus a structured call — the exact shape that used to score
/// zero against every assertion and read as a model failure.
const TOOL_SUITE: &str = r#"
version: 1
suite: tools
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"\",\"empty_reason\":\"tool_use_only\",\"tool_calls\":[{\"name\":\"get_weather\",\"arguments\":{\"city\":\"Oslo\"}}]}'"]
tools:
  - name: get_weather
    description: look up the weather
    input_schema: {type: object, properties: {city: {type: string}}}
tests:
  - id: asks-for-weather
    assert:
      - type: tool-call
        name: get_weather
        args: {city: "Oslo"}
      - type: not-tool-call
        name: delete_everything
"#;

#[test]
fn a_case_answered_by_a_tool_call_is_gradeable() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), TOOL_SUITE);
    bin()
        .args(["run", "--no-cache"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS"));
}

/// A history whose first non-`system` turn is `assistant` — a near-certain
/// provider 400, and the shape `validate` warns about without refusing.
const ASSISTANT_FIRST_HISTORY_SUITE: &str = r#"
version: 1
suite: smoke
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"hello world\"}'"]
tests:
  - id: greet
    history:
      - {role: assistant, content: "I already answered."}
    assert:
      - {type: contains, value: "hello"}
"#;

/// The whole point of the severity axis: advice must not become an exit code.
/// Exit `2` is documented as a config/usage error and CI gates on it, so a
/// shape that is merely *probably* wrong — an Anthropic assistant prefill is a
/// real one — has to stay runnable.
#[test]
fn validate_warns_on_assistant_first_history_without_failing() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), ASSISTANT_FIRST_HISTORY_SUITE);
    bin()
        .arg("validate")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("ok:"))
        .stdout(predicate::str::contains("1 warning(s)"))
        .stderr(predicate::str::contains("warning:"))
        .stderr(predicate::str::contains("assistant"));
}

/// Companion to `stdout_stays_pure_results_diagnostics_go_to_stderr`: the
/// finding itself belongs on stderr, and only its count rides the summary line.
#[test]
fn a_validate_warning_body_never_reaches_stdout() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), ASSISTANT_FIRST_HISTORY_SUITE);
    let output = bin().arg("validate").arg(dir.path()).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("warning:"),
        "the warning body must stay on stderr, got stdout: {stdout}"
    );
}

/// The single most important behavioural guard in the severity change: before
/// it, every `Issue` was fatal and `run` aborted on any non-empty result. A
/// warning must let the run proceed to completion.
#[test]
fn run_proceeds_through_a_history_warning() {
    let dir = tempfile::tempdir().unwrap();
    write_suite(dir.path(), ASSISTANT_FIRST_HISTORY_SUITE);
    let output = bin()
        .arg("run")
        .current_dir(dir.path())
        .env_remove("DOMARINN_SERVER_URL")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "a validate warning must not abort a run: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("assistant"),
        "the warning must still be reported on the run: {stderr}"
    );
}
