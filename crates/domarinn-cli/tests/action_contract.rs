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
//! So the scripts are run here rather than read. Each test executes the real
//! `run:` text lifted out of `action.yml`, under the interpreter GitHub uses,
//! against a stub binary that exits on demand.
//!
//! # The shape of the file
//!
//! [`SHELL_STEPS`] is the registry: one entry per `run:` step, carrying the
//! environment a caller who set no optional inputs would produce.
//! [`every_shell_step_is_exercised_here`] fails if the action grows a step the
//! registry does not name, because the first fix for this bug covered only the
//! two steps it was reported against and left the same abort live in the step
//! that resolves the binary. Errexit is a property of the *shell*, so a guard
//! that opts steps in one at a time will keep missing it; this one opts them
//! out.
//!
//! [`the_shape_this_test_assumes_still_holds`] guards the assumption underneath
//! all of them: if a step stops declaring `shell: bash`, or grows an input, the
//! interpreter and environment reproduced here have silently stopped matching
//! the runner's and the other tests would be asserting about nothing.
//!
//! # Three actions, one contract
//!
//! The action ships three times: for GitHub, and for Gitea and Forgejo Actions
//! under `.gitea/` and `.forgejo/`, whose runners are act-based and expand
//! `shell: bash` the same way. Every test here runs against each of
//! [`ACTIONS`], so a clause verified on one copy is verified on all of them.
//! What the copies may and may not differ in is pinned separately, in
//! `action_contract/forge_tests.rs`.

#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const GITHUB_ACTION: &str = ".github/actions/domarinn-eval/action.yml";
const GITEA_ACTION: &str = ".gitea/actions/domarinn-eval/action.yml";
const FORGEJO_ACTION: &str = ".forgejo/actions/domarinn-eval/action.yml";

/// Every copy of the action. Looping over this is what makes a test prove its
/// clause for a Gitea or Forgejo runner as well as GitHub's: the shell steps
/// are meant to be identical, and "meant to be" is the claim a test checks
/// rather than assumes.
const ACTIONS: &[&str] = &[GITHUB_ACTION, GITEA_ACTION, FORGEJO_ACTION];

/// The interpreter GitHub runs a `shell: bash` step under, verbatim from its
/// documented default: `bash --noprofile --norc -e -o pipefail {0}`.
///
/// Reproduced rather than simplified. Under a plain `bash script.sh` the bug
/// this file exists for does not happen at all, so a test that took the
/// convenient shell would pass against the broken action.
const GITHUB_BASH: &[&str] = &["--noprofile", "--norc", "-e", "-o", "pipefail"];

/// One `run:` step of the action: how to find it, and the variables it reads.
///
/// `field` is `id` for the steps that have one and `name` for the rest — the
/// same two ways `action.yml` identifies a step.
struct ShellStep {
    field: &'static str,
    value: &'static str,
    /// Exactly the keys of the step's `env:` block, with the values a caller
    /// who set no optional inputs would produce. The empty ones are the default
    /// path and the one most likely to break: every optional argument is
    /// appended by a `[ -n "$X" ] && args+=(…)` line, and an unset input is
    /// what makes those tests fail. Tests override individual entries.
    env: &'static [(&'static str, &'static str)],
}

/// Every `run:` step the three actions share. Adding a step to an
/// `action.yml` without adding it here (or to [`FORGE_SHELL_STEPS`]) fails
/// [`every_shell_step_is_exercised_here`].
const SHELL_STEPS: &[ShellStep] = &[
    ShellStep {
        field: "id",
        value: "bin",
        env: &[
            ("INPUT_BINARY_PATH", ""),
            ("INPUT_VERSION", "latest"),
            (
                "INPUT_RELEASE_BASE_URL",
                "https://github.com/AtvikSecurity/domarinn",
            ),
        ],
    },
    ShellStep {
        field: "id",
        value: "eval",
        env: &[
            ("DOMARINN_BIN", ""),
            ("DOMARINN_SERVER_URL", ""),
            ("DOMARINN_TOKEN", ""),
            ("DOMARINN_BRANCH", ""),
            ("INPUT_CONFIG", "suite.yaml"),
            ("INPUT_PROVIDER", ""),
            ("INPUT_AGAINST", ""),
            ("INPUT_ALLOW_EMPTY", "false"),
            ("INPUT_ALLOW_SHARE_FAILURE", "false"),
            ("INPUT_CACHE_DIR", ""),
        ],
    },
    ShellStep {
        field: "id",
        value: "summary",
        env: &[
            ("DOMARINN_BIN", ""),
            ("DOMARINN_SERVER_URL", ""),
            ("DOMARINN_TOKEN", ""),
            ("INPUT_AGAINST", ""),
        ],
    },
    ShellStep {
        field: "name",
        value: "Gate on result",
        env: &[
            ("CODE", ""),
            ("FAIL_ON_REGRESSION", "true"),
            ("ERROR_CLASSES", ""),
            ("SERVER_URL", ""),
            ("RUN_URL", ""),
            ("SUMMARY_OUTCOME", ""),
        ],
    },
];

/// `run:` steps only the Gitea and Forgejo actions have: the PR comment, which
/// on GitHub is `actions/github-script` and so not shell at all. `API_URL` is
/// empty by default so a test that forgets its stub server makes no request.
const FORGE_SHELL_STEPS: &[ShellStep] = &[ShellStep {
    field: "id",
    value: "comment",
    env: &[
        ("SUMMARY_PATH", "summary.md"),
        ("COMMENT_TOKEN", "t0ken"),
        ("API_URL", ""),
        ("REPOSITORY", "acme/widgets"),
        ("PR_NUMBER", "7"),
    ],
}];

/// The registry for one action: the shared steps, plus the forge-only ones on
/// a forge copy.
fn registry(action: &str) -> Vec<&'static ShellStep> {
    let forge_only = if action == GITHUB_ACTION {
        &[][..]
    } else {
        FORGE_SHELL_STEPS
    };
    SHELL_STEPS.iter().chain(forge_only).collect()
}

fn registered(action: &str, value: &str) -> &'static ShellStep {
    registry(action)
        .into_iter()
        .find(|s| s.value == value)
        .unwrap_or_else(|| panic!("{action}: the registry has no entry for {value}"))
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root resolves from crates/domarinn-cli")
}

fn raw_action(action: &str) -> String {
    std::fs::read_to_string(repo_root().join(action))
        .unwrap_or_else(|e| panic!("reading {action}: {e}"))
}

fn parsed(action: &str) -> serde_json::Value {
    domarinn_core::yaml::parse_value(&raw_action(action))
        .unwrap_or_else(|e| panic!("parsing {action}: {e}"))
}

fn steps(action: &str) -> Vec<serde_json::Value> {
    parsed(action)["runs"]["steps"]
        .as_array()
        .expect("runs.steps is a list")
        .clone()
}

/// The step whose `field` is `value`.
fn step(action: &str, field: &str, value: &str) -> serde_json::Value {
    steps(action)
        .into_iter()
        .find(|s| s[field].as_str() == Some(value))
        .unwrap_or_else(|| panic!("{action} has no step with {field}: {value}"))
}

fn script_of(action: &str, field: &str, value: &str) -> String {
    step(action, field, value)["run"]
        .as_str()
        .unwrap_or_else(|| panic!("{action}: the {value} step has a `run:` script"))
        .to_string()
}

/// What a step run by [`run_step`] left behind.
struct Ran {
    status: Option<i32>,
    /// stdout and stderr together: a `::error::` annotation and the shell's own
    /// diagnostic land on different streams, and a reader of the job log sees
    /// one interleaved transcript.
    log: String,
    /// The `key=value` lines the step appended to `$GITHUB_OUTPUT`.
    outputs: BTreeMap<String, String>,
    /// What the step appended to `$GITHUB_STEP_SUMMARY`.
    step_summary: String,
}

impl Ran {
    fn output(&self, key: &str) -> Option<&str> {
        self.outputs.get(key).map(String::as_str)
    }
}

/// A workspace for one step: the runner-supplied files it writes to, and a
/// `shims` directory on the front of `PATH` for tests that need a command
/// (`curl`, `uname`) to behave a particular way.
struct Workspace {
    dir: tempfile::TempDir,
}

impl Workspace {
    fn new() -> Self {
        let ws = Self {
            dir: tempfile::tempdir().expect("a temp dir"),
        };
        std::fs::create_dir_all(ws.path().join("shims")).unwrap();
        ws
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Write an executable `name` on the front of the step's `PATH`.
    fn shim(&self, name: &str, body: &str) -> &Self {
        self.write_exec(&self.path().join("shims").join(name), body);
        self
    }

    /// A stand-in for the CLI, at a path the caller passes as `DOMARINN_BIN`.
    /// Not a shim: the action invokes it by path, never by name.
    fn stub_cli(&self, body: &str) -> String {
        let path = self.path().join("stub-domarinn");
        self.write_exec(&path, body);
        path.to_str().expect("a UTF-8 temp path").to_string()
    }

    fn write_exec(&self, path: &Path, body: &str) {
        std::fs::write(path, format!("#!/usr/bin/env bash\n{body}")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// Run a registered step's `run:` text the way the runner would.
///
/// `overrides` replace entries of the step's [`ShellStep::env`]; every other
/// declared variable keeps its default, so `set -u` sees the environment the
/// runner would have built.
fn run_step(action: &str, value: &str, overrides: &[(&str, &str)], ws: &Workspace) -> Ran {
    let registered = registered(action, value);
    let script = ws.path().join("step.sh");
    std::fs::write(&script, script_of(action, registered.field, value)).unwrap();

    let mut env: BTreeMap<String, String> = registered
        .env
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    for (k, v) in overrides {
        env.insert((*k).to_string(), (*v).to_string());
    }

    // Runner-supplied, never in a step's `env:` block. Redirected per run so a
    // test can read them back — and so running the suite *on* Actions cannot
    // append to the real ones.
    let github_output = ws.path().join("github_output");
    let step_summary = ws.path().join("github_step_summary");
    std::fs::write(&github_output, "").unwrap();
    std::fs::write(&step_summary, "").unwrap();
    env.insert("GITHUB_OUTPUT".into(), path_str(&github_output));
    env.insert("GITHUB_STEP_SUMMARY".into(), path_str(&step_summary));
    env.insert("RUNNER_TEMP".into(), path_str(ws.path()));
    env.insert("GITHUB_WORKSPACE".into(), path_str(ws.path()));
    env.insert(
        "PATH".into(),
        format!(
            "{}:{}",
            path_str(&ws.path().join("shims")),
            std::env::var("PATH").unwrap_or_default()
        ),
    );

    // A forge runner may not set the summary file at all; a test asks for that
    // by overriding it to the empty string, and gets it truly unset.
    let unset_summary = overrides
        .iter()
        .any(|(k, v)| *k == "GITHUB_STEP_SUMMARY" && v.is_empty());
    if unset_summary {
        env.remove("GITHUB_STEP_SUMMARY");
    }

    let mut cmd = Command::new("bash");
    cmd.args(GITHUB_BASH)
        .arg(&script)
        .current_dir(ws.path())
        .envs(&env);
    if unset_summary {
        cmd.env_remove("GITHUB_STEP_SUMMARY");
    }
    let out: Output = cmd.output().expect("bash is on PATH");

    Ran {
        status: out.status.code(),
        log: format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        outputs: std::fs::read_to_string(&github_output)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        step_summary: std::fs::read_to_string(&step_summary).unwrap_or_default(),
    }
}

fn path_str(path: &Path) -> String {
    path.to_str().expect("a UTF-8 temp path").to_string()
}

/// Run the eval step against a stub CLI that writes its report and exits
/// `code`, so the step sees a real non-zero exit rather than a missing binary.
fn eval_step_with(action: &str, code: i32, ws: &Workspace) -> Ran {
    let stub = ws.stub_cli(&format!(
        "printf '<testsuites/>' > results.xml\nexit {code}\n"
    ));
    run_step(action, "eval", &[("DOMARINN_BIN", &stub)], ws)
}

/// The line the eval step logs before invoking the CLI with `overrides`.
fn eval_invocation(action: &str, overrides: &[(&str, &str)]) -> String {
    let ws = Workspace::new();
    let stub = ws.stub_cli("printf '<testsuites/>' > results.xml\n");
    let mut all = vec![("DOMARINN_BIN", stub.as_str())];
    all.extend_from_slice(overrides);
    let ran = run_step(action, "eval", &all, &ws);
    ran.log
        .lines()
        .find(|line| line.starts_with("Running: domarinn "))
        .unwrap_or_else(|| panic!("{action}: the step logs the command it ran:\n{}", ran.log))
        .to_string()
}

/// Every exit code the CLI documents must survive as a step output.
///
/// `results-path` matters as much as `exit-code`: the artifact upload is gated
/// on it, so losing it drops the JUnit report exactly when a failing run makes
/// it worth having.
#[test]
fn the_eval_step_writes_its_outputs_for_every_exit_code() {
    for action in ACTIONS {
        for code in [0, 1, 2, 3] {
            let ws = Workspace::new();
            let ran = eval_step_with(action, code, &ws);

            assert_eq!(
                ran.output("exit-code"),
                Some(code.to_string().as_str()),
                "{action}: exit {code} must reach the gate step; the eval step logged:\n{}",
                ran.log
            );
            assert_eq!(
                ran.output("results-path"),
                Some("results.xml"),
                "{action}: exit {code} must still publish the report for the upload step"
            );
            assert!(
                ran.log.contains(&format!("domarinn exited with {code}")),
                "{action}: the step log must name the code a reader will see annotated: {}",
                ran.log
            );
        }
    }
}

/// The upload opt-out has to reach the CLI, and only ever beside `--share`.
///
/// Both halves are load-bearing. Without the input a caller who set `server-url`
/// has no way at all to keep a transient upload failure from failing the job:
/// `run --share` fails closed with exit 3, and 3 is ungated here by design.
/// Appending it *without* `--share` is the opposite failure — the CLI rejects
/// the flag on its own, so an input whose whole purpose is to make the job more
/// tolerant would fail it with a usage error instead.
#[test]
fn the_share_opt_out_only_travels_with_share() {
    for action in ACTIONS {
        let invocation = |server: &str, opt_out: &str| {
            eval_invocation(
                action,
                &[
                    ("DOMARINN_SERVER_URL", server),
                    ("INPUT_ALLOW_SHARE_FAILURE", opt_out),
                ],
            )
        };

        let server = "https://evals.example";

        let gated = invocation(server, "false");
        assert!(
            gated.contains("--share"),
            "{action}: a server means an upload: {gated}"
        );
        assert!(
            !gated.contains("--allow-share-failure"),
            "{action}: the default must keep gating the job on the upload: {gated}"
        );

        let opted_out = invocation(server, "true");
        assert!(
            opted_out.contains("--share") && opted_out.contains("--allow-share-failure"),
            "{action}: the opt-out must reach the CLI; without it exit 3 is \
             unavoidable and ungated: {opted_out}"
        );

        let no_server = invocation("", "true");
        assert!(
            !no_server.contains("--share"),
            "{action}: no server, no upload: {no_server}"
        );
        // Asserted separately, and not as `!contains("--share")`: `--share` is
        // not a substring of `--allow-share-failure`, so the line above passes
        // just as happily against a step that emitted the opt-out with nothing
        // to qualify. That is the exact regression worth catching — clap
        // rejects the flag on its own, so the result is exit 2 on every run,
        // unconditionally fatal at the gate.
        assert!(
            !no_server.contains("--allow-share-failure"),
            "{action}: the opt-out requires --share; alone it is a usage error \
             the gate always fails on: {no_server}"
        );
    }
}

/// The `provider:` input expands to repeated `--provider` flags — the only
/// path from the composite action to the CLI's cell selection. Comma and space
/// separators both work, and an empty input appends nothing, so the default
/// still runs the whole matrix.
#[test]
fn the_provider_input_expands_to_repeated_flags() {
    for action in ACTIONS {
        let running = |provider: &str| eval_invocation(action, &[("INPUT_PROVIDER", provider)]);

        assert!(
            !running("").contains("--provider"),
            "{action}: an empty input must not shrink the matrix"
        );
        let comma = running("primary,canary");
        assert!(
            comma.contains("--provider primary") && comma.contains("--provider canary"),
            "{action}: comma-separated ids become repeated flags: {comma}"
        );
        let spaced = running("primary canary");
        assert!(
            spaced.contains("--provider primary") && spaced.contains("--provider canary"),
            "{action}: space-separated ids become repeated flags: {spaced}"
        );
    }
}

/// Splitting must not also glob. An unquoted `$var` in a bash for-loop expands
/// patterns against the working directory, so an id like `model-[a]` beside a
/// checkout containing a file `model-a` would silently select a *different*
/// provider than the one the workflow named.
#[test]
fn the_provider_input_never_globs_against_the_workspace() {
    for action in ACTIONS {
        let ws = Workspace::new();
        std::fs::write(ws.path().join("model-a"), "").expect("a decoy file the glob would match");
        let stub = ws.stub_cli("printf '<testsuites/>' > results.xml\n");
        let ran = run_step(
            action,
            "eval",
            &[("DOMARINN_BIN", &stub), ("INPUT_PROVIDER", "model-[a]")],
            &ws,
        );
        let line = ran
            .log
            .lines()
            .find(|line| line.starts_with("Running: domarinn "))
            .unwrap_or_else(|| panic!("{action}: the step logs the command it ran:\n{}", ran.log));
        assert!(
            line.contains("--provider model-[a]") && !line.contains("--provider model-a "),
            "{action}: the id must reach the CLI verbatim, not glob-matched: {line}"
        );
    }
}

// The gate-step behavior tests — what each exit code renders and names — live
// in a subdirectory file to keep this one under the 1000-line cap. A
// subdirectory of `tests/` is not compiled as its own integration-test crate,
// so `#[path]` is the same private-child-module pattern `grader_tests.rs`
// uses in src. The binary-resolution tests and the Gitea/Forgejo drift and
// PR-comment tests are split the same way.
#[path = "action_contract/gate_tests.rs"]
mod gate_tests;

#[path = "action_contract/bin_tests.rs"]
mod bin_tests;

#[path = "action_contract/forge_tests.rs"]
mod forge_tests;

/// The summary step is a reporter: the gate step owns the verdict, and this one
/// failing turns a clean pass or a legible regression into a red job with a
/// `cat` error for a message.
///
/// Its own comment says it "always exits 0 on a completed run", but the exit
/// code of `ci-summary` is not the precondition the step actually needs — the
/// file is. A CLI that exits 0 without writing one (an `--out` it could not
/// create, a run with nothing in it) takes the success branch and then dies on
/// the `cat`.
#[test]
fn the_summary_step_never_owns_the_verdict() {
    for action in ACTIONS {
        for (case, body) in [
            ("ci-summary failed", "exit 1\n"),
            ("ci-summary exited 0 but wrote nothing", "exit 0\n"),
            ("ci-summary wrote an empty file", ": > summary.md\nexit 0\n"),
        ] {
            let ws = Workspace::new();
            let stub = ws.stub_cli(body);
            let ran = run_step(action, "summary", &[("DOMARINN_BIN", &stub)], &ws);

            assert_eq!(
                ran.status,
                Some(0),
                "{action}: {case}: the reporter must not fail the job; it logged:\n{}",
                ran.log
            );
            assert_eq!(
                ran.output("summary-path"),
                Some("summary.md"),
                "{action}: {case}: the PR comment and the artifact upload both read this path"
            );
            assert!(
                !ran.step_summary.trim().is_empty(),
                "{action}: {case}: a blank job summary reads as 'nothing ran'; say what happened"
            );
        }
    }
}

/// A summary the CLI did produce is passed through untouched.
#[test]
fn the_summary_step_publishes_what_the_cli_wrote() {
    for action in ACTIONS {
        let ws = Workspace::new();
        let stub = ws.stub_cli("printf '### domarinn eval\\n\\n12 passed\\n' > summary.md\n");
        let ran = run_step(action, "summary", &[("DOMARINN_BIN", &stub)], &ws);

        assert_eq!(ran.status, Some(0), "{action}: {}", ran.log);
        assert!(
            ran.step_summary.contains("12 passed"),
            "{action}: the real summary must survive to the job summary, got: {:?}",
            ran.step_summary
        );
    }
}

/// A Gitea or Forgejo runner need not set `$GITHUB_STEP_SUMMARY` at all, and
/// under `set -u` the bare `>> "$GITHUB_STEP_SUMMARY"` the step used to have
/// aborts the reporter on the unset name — a red job over a page nothing would
/// have rendered. The step is shared verbatim, so every copy is held to it.
#[test]
fn the_summary_step_survives_a_runner_without_a_step_summary_file() {
    for action in ACTIONS {
        let ws = Workspace::new();
        let stub = ws.stub_cli("printf '### domarinn eval\\n\\n12 passed\\n' > summary.md\n");
        let ran = run_step(
            action,
            "summary",
            &[("DOMARINN_BIN", &stub), ("GITHUB_STEP_SUMMARY", "")],
            &ws,
        );
        assert_eq!(
            ran.status,
            Some(0),
            "{action}: an absent step summary is not an error; the step logged:\n{}",
            ran.log
        );
        assert_eq!(ran.output("summary-path"), Some("summary.md"));
    }
}

/// Errexit applies to every step, so the registry must cover every step.
///
/// The first fix for this class of bug covered the two steps it was reported
/// against and left the identical abort live in a third. Opting steps in one at
/// a time reproduces that; this fails until the registry names them all.
#[test]
fn every_shell_step_is_exercised_here() {
    for action in ACTIONS {
        let in_action: BTreeSet<String> = steps(action)
            .iter()
            .filter(|s| s["run"].as_str().is_some())
            .map(|s| {
                s["id"]
                    .as_str()
                    .or_else(|| s["name"].as_str())
                    .expect("a `run:` step has an id or a name")
                    .to_string()
            })
            .collect();
        let registered: BTreeSet<String> = registry(action)
            .iter()
            .map(|s| s.value.to_string())
            .collect();

        assert_eq!(
            in_action, registered,
            "{action}: a `run:` step this file never executes is a step whose \
             behaviour under `bash -e` is unverified — the exact gap that \
             shipped the bug this file exists for. Add it to SHELL_STEPS (or \
             FORGE_SHELL_STEPS) and give it a test."
        );
    }
}

/// What every test above assumes about the steps it executes.
///
/// They reproduce the runner rather than call it: [`GITHUB_BASH`] is the
/// expansion of the literal `shell: bash`, and each [`ShellStep::env`] is
/// hand-written. If either drifts from `action.yml` the tests keep passing
/// while asserting about a step the runner no longer runs.
#[test]
fn the_shape_this_test_assumes_still_holds() {
    for action in ACTIONS {
        for registered in registry(action) {
            let step = step(action, registered.field, registered.value);
            let name = registered.value;

            assert_eq!(
                step["shell"].as_str(),
                Some("bash"),
                "{action}: {name}: GITHUB_BASH is the expansion of exactly this \
                 string; changing the shell means changing it here too"
            );

            let mut declared: Vec<&str> = step["env"]
                .as_object()
                .unwrap_or_else(|| panic!("{action}: {name} declares an `env` block"))
                .keys()
                .map(String::as_str)
                .collect();
            let mut covered: Vec<&str> = registered.env.iter().map(|(k, _)| *k).collect();
            declared.sort_unstable();
            covered.sort_unstable();

            assert_eq!(
                declared, covered,
                "{action}: {name}: an input the step reads but this test never \
                 sets is an input whose effect on the exit-code contract is \
                 untested"
            );
        }
    }
}

// ── Parity ratchet: the action's surface vs the CLI's `run` surface ─────────
//
// Issue #83's root shape: `--provider` existed for a year of releases while the
// composite action had no way to say it, so the CLI's answer to a real problem
// was unreachable from CI. The ratchet below makes that drift a test failure.
// Adding a `domarinn run` flag now forces a decision — wire an input, or list
// the flag here with the reason it stays CLI-only. (Environment variables need
// no mapping: a composite action inherits the caller's `env:`, so every
// `DOMARINN_*` variable already reaches the CLI without an input.)

/// Inputs that reach `domarinn run`, and the argv text the eval script must
/// therefore contain. `config` is the positional; the rest are flags.
const INPUT_TO_RUN_ARGV: &[(&str, &str)] = &[
    ("config", r#"args=(run "${INPUT_CONFIG}""#),
    ("provider", "--provider"),
    ("against", "--against"),
    ("cache-dir", "--cache-dir"),
    ("allow-empty", "--allow-empty"),
    ("allow-share-failure", "--allow-share-failure"),
    // `server-url` is an env export, but it is also the switch for `--share`.
    ("server-url", "--share"),
];

/// Inputs that become environment exports the CLI reads.
const INPUT_TO_ENV_EXPORT: &[(&str, &str)] = &[
    ("server-url", "DOMARINN_SERVER_URL"),
    ("token", "DOMARINN_TOKEN"),
    ("branch", "DOMARINN_BRANCH"),
];

/// Inputs that configure the action itself and never reach the CLI.
const ACTION_LEVEL_INPUTS: &[&str] = &[
    "binary-path",
    "version",
    "release-base-url",
    "fail-on-regression",
    "comment",
    "artifact-name",
];

/// Action-level inputs only the Gitea and Forgejo copies declare. On GitHub
/// the comment is posted by `actions/github-script` with the job token, so
/// there is nothing to configure.
const FORGE_ONLY_INPUTS: &[&str] = &["comment-token"];

/// `domarinn run` flags deliberately not exposed as inputs, with the reason.
/// An entry here is a decision, not a default — a new flag lands in this list
/// only by someone writing down why CI callers do not need it.
const RUN_FLAGS_WITHOUT_AN_INPUT: &[(&str, &str)] = &[
    ("--tag", "test selection belongs in the suite a reviewer reads, not the workflow"),
    ("--filter", "same as --tag"),
    ("--prompt", "same as --tag"),
    ("--no-cache", "CI wants the cache; a job that must not read it can unset cache-dir and use a throwaway path"),
    ("--cache-only", "an offline replay is a different workflow than a gate"),
    ("--no-cache-migration", "an optimization toggle with no per-job story"),
    ("--store-empty-outputs", "standing policy; DOMARINN_CACHE_STORE_EMPTY_OUTPUTS reaches the step as env"),
    ("--repeat", "variance studies are interactive work, not a PR gate"),
    ("--concurrency", "suite-level runner.concurrency is where throughput policy lives"),
    ("--retries", "suite-level runner.retries is where retry policy lives"),
    ("--no-retries", "same as --retries"),
    ("--format", "the action's contract is junit + markdown; changing that changes its later steps too"),
    ("--out", "owned by the action; the artifact upload depends on it"),
    ("--summary-md", "the summary step produces the markdown; a second writer would race it"),
    ("--no-raw", "result-size tuning with no per-job story"),
    ("--no-progress", "no TTY in CI; the CLI already suppresses the bar"),
    ("--no-grader-cache", "judge-variance measurement is interactive work"),
    ("--fallback-on-empty-reason", "standing policy; DOMARINN_FALLBACK_ON_EMPTY_REASON reaches the step as env"),
    ("--no-fallback", "standing policy; DOMARINN_NO_FALLBACK reaches the step as env"),
    ("--color", "output styling; the runner's log renders ANSI on its own terms"),
    ("--log-format", "diagnostics plumbing, not eval configuration"),
    ("--verbose", "same as --log-format"),
    ("--no-provenance", "identity policy is environment-driven (DOMARINN_PROVENANCE), and env reaches the step"),
    ("--note", "a free-text annotation is interactive work; CI provenance is recorded automatically"),
    ("--server-url", "the server-url input reaches the CLI as the DOMARINN_SERVER_URL export"),
];

/// The flags `domarinn run -h` actually offers, parsed from option lines.
fn run_flags() -> BTreeSet<String> {
    let out = assert_cmd::Command::cargo_bin("domarinn")
        .expect("the domarinn binary builds")
        .args(["run", "-h"])
        .output()
        .expect("domarinn run -h executes");
    let help = String::from_utf8_lossy(&out.stdout).to_string();
    help.lines()
        .filter_map(|line| {
            // Option lines are `  --flag …` or `  -j, --flag …`; taking only
            // the first token of lines that start with a dash keeps flags
            // merely *mentioned* in a description from registering.
            let t = line.trim_start();
            let rest = if let Some(rest) = t.strip_prefix("--") {
                rest
            } else if t.starts_with('-') {
                t.split_once("--")?.1
            } else {
                return None;
            };
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            (!name.is_empty()).then(|| format!("--{name}"))
        })
        .filter(|f| f != "--help")
        .collect()
}

#[test]
fn every_run_flag_is_an_input_or_a_written_down_exception() {
    let flags = run_flags();
    assert!(
        flags.contains("--provider"),
        "parsing `run -h` found no --provider; the parser is broken: {flags:?}"
    );

    let excepted: BTreeSet<String> = RUN_FLAGS_WITHOUT_AN_INPUT
        .iter()
        .map(|(f, _)| f.to_string())
        .collect();

    for action in ACTIONS {
        let script = script_of(action, "id", "eval");
        let mut covered = BTreeSet::new();
        for (input, argv_text) in INPUT_TO_RUN_ARGV {
            assert!(
                script.contains(argv_text),
                "{action}: input `{input}` claims to reach the CLI as \
                 `{argv_text}`, but the eval script never says it — the \
                 mapping is aspirational"
            );
            if let Some(flag) = argv_text.strip_prefix("--") {
                covered.insert(format!("--{flag}"));
            }
        }

        if let Some(flag) = covered.intersection(&excepted).next() {
            panic!("{flag} is both wired to an input and excepted; pick one");
        }
        for flag in &flags {
            assert!(
                covered.contains(flag) || excepted.contains(flag),
                "{action}: `domarinn run {flag}` is reachable from a shell but \
                 not from the action. Either add an input that forwards it, or \
                 add it to RUN_FLAGS_WITHOUT_AN_INPUT with the reason it stays \
                 CLI-only."
            );
        }
    }
    for flag in &excepted {
        assert!(
            flags.contains(flag),
            "{flag} is excepted here but `domarinn run -h` no longer offers \
             it; remove the stale entry"
        );
    }
}

#[test]
fn every_declared_input_is_classified_and_consumed() {
    for action in ACTIONS {
        let inputs: BTreeSet<String> = parsed(action)["inputs"]
            .as_object()
            .expect("the action declares inputs")
            .keys()
            .cloned()
            .collect();

        let forge_only: &[&str] = if *action == GITHUB_ACTION {
            &[]
        } else {
            FORGE_ONLY_INPUTS
        };
        let classified: BTreeSet<String> = INPUT_TO_RUN_ARGV
            .iter()
            .map(|(i, _)| i.to_string())
            .chain(INPUT_TO_ENV_EXPORT.iter().map(|(i, _)| i.to_string()))
            .chain(ACTION_LEVEL_INPUTS.iter().map(|i| i.to_string()))
            .chain(forge_only.iter().map(|i| i.to_string()))
            .collect();

        for input in &inputs {
            assert!(
                classified.contains(input),
                "{action}: input `{input}` is declared but unclassified here; \
                 say whether it reaches the CLI (INPUT_TO_RUN_ARGV / \
                 INPUT_TO_ENV_EXPORT) or stays action-level \
                 (ACTION_LEVEL_INPUTS, or FORGE_ONLY_INPUTS for a forge copy)"
            );
        }
        for input in &classified {
            assert!(
                inputs.contains(input),
                "{action}: `{input}` is classified here but the action no \
                 longer declares it"
            );
        }

        // A declared input nothing interpolates is dead weight a caller will
        // still try to use.
        let raw = raw_action(action);
        for input in &inputs {
            assert!(
                raw.contains(&format!("inputs.{input}")),
                "{action}: input `{input}` is declared but never interpolated anywhere"
            );
        }
    }
}
