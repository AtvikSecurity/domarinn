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

#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
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

/// Every `run:` step in the action. Adding a step to `action.yml` without
/// adding it here fails [`every_shell_step_is_exercised_here`].
const SHELL_STEPS: &[ShellStep] = &[
    ShellStep {
        field: "id",
        value: "bin",
        env: &[("INPUT_BINARY_PATH", ""), ("INPUT_VERSION", "latest")],
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
        env: &[("CODE", ""), ("FAIL_ON_REGRESSION", "true")],
    },
];

fn registered(value: &str) -> &'static ShellStep {
    SHELL_STEPS
        .iter()
        .find(|s| s.value == value)
        .unwrap_or_else(|| panic!("SHELL_STEPS has no entry for {value}"))
}

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

fn steps() -> Vec<serde_yaml_ng::Value> {
    action()["runs"]["steps"]
        .as_sequence()
        .expect("runs.steps is a list")
        .clone()
}

/// The step whose `field` is `value`.
fn step(field: &str, value: &str) -> serde_yaml_ng::Value {
    steps()
        .into_iter()
        .find(|s| s[field].as_str() == Some(value))
        .unwrap_or_else(|| panic!("{ACTION} has no step with {field}: {value}"))
}

fn script_of(field: &str, value: &str) -> String {
    step(field, value)["run"]
        .as_str()
        .unwrap_or_else(|| panic!("the {value} step has a `run:` script"))
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
fn run_step(value: &str, overrides: &[(&str, &str)], ws: &Workspace) -> Ran {
    let registered = registered(value);
    let script = ws.path().join("step.sh");
    std::fs::write(&script, script_of(registered.field, value)).unwrap();

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

    let out: Output = Command::new("bash")
        .args(GITHUB_BASH)
        .arg(&script)
        .current_dir(ws.path())
        .envs(&env)
        .output()
        .expect("bash is on PATH");

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
fn eval_step_with(code: i32, ws: &Workspace) -> Ran {
    let stub = ws.stub_cli(&format!(
        "printf '<testsuites/>' > results.xml\nexit {code}\n"
    ));
    run_step("eval", &[("DOMARINN_BIN", &stub)], ws)
}

/// Every exit code the CLI documents must survive as a step output.
///
/// `results-path` matters as much as `exit-code`: the artifact upload is gated
/// on it, so losing it drops the JUnit report exactly when a failing run makes
/// it worth having.
#[test]
fn the_eval_step_writes_its_outputs_for_every_exit_code() {
    for code in [0, 1, 2, 3] {
        let ws = Workspace::new();
        let ran = eval_step_with(code, &ws);

        assert_eq!(
            ran.output("exit-code"),
            Some(code.to_string().as_str()),
            "exit {code} must reach the gate step; the eval step logged:\n{}",
            ran.log
        );
        assert_eq!(
            ran.output("results-path"),
            Some("results.xml"),
            "exit {code} must still publish the report for the upload step"
        );
        assert!(
            ran.log.contains(&format!("domarinn exited with {code}")),
            "the step log must name the code a reader will see annotated: {}",
            ran.log
        );
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
    let invocation = |server: &str, opt_out: &str| {
        let ws = Workspace::new();
        let stub = ws.stub_cli("printf '<testsuites/>' > results.xml\n");
        let ran = run_step(
            "eval",
            &[
                ("DOMARINN_BIN", &stub),
                ("DOMARINN_SERVER_URL", server),
                ("INPUT_ALLOW_SHARE_FAILURE", opt_out),
            ],
            &ws,
        );
        ran.log
            .lines()
            .find(|line| line.starts_with("Running: domarinn "))
            .unwrap_or_else(|| panic!("the step logs the command it ran:\n{}", ran.log))
            .to_string()
    };

    let server = "https://evals.example";

    let gated = invocation(server, "false");
    assert!(
        gated.contains("--share"),
        "a server means an upload: {gated}"
    );
    assert!(
        !gated.contains("--allow-share-failure"),
        "the default must keep gating the job on the upload: {gated}"
    );

    let opted_out = invocation(server, "true");
    assert!(
        opted_out.contains("--share") && opted_out.contains("--allow-share-failure"),
        "the opt-out must reach the CLI; without it exit 3 is unavoidable and \
         ungated: {opted_out}"
    );

    let no_server = invocation("", "true");
    assert!(
        !no_server.contains("--share"),
        "no server, no upload: {no_server}"
    );
    // Asserted separately, and not as `!contains("--share")`: the substring
    // `--share` does not occur in `--allow-share-failure` (the character before
    // `share` is a `w`), so the line above passes just as happily against a
    // step that emitted the opt-out with nothing to qualify. That is the exact
    // regression worth catching — clap rejects the flag on its own, so the
    // result is exit 2 on every run, unconditionally fatal at the gate.
    assert!(
        !no_server.contains("--allow-share-failure"),
        "the opt-out requires --share; alone it is a usage error the gate \
         always fails on: {no_server}"
    );
}

/// The contract: `1` means the model regressed and the PR is to blame, `3`
/// means the harness broke and it is not. A CI consumer sees that distinction
/// only here, so the two must not render the same.
#[test]
fn the_gate_distinguishes_a_regression_from_a_broken_harness() {
    let annotation_for = |code: i32| {
        let ws = Workspace::new();
        let observed = eval_step_with(code, &ws)
            .output("exit-code")
            .unwrap_or_default()
            .to_string();
        let ran = run_step("Gate on result", &[("CODE", &observed)], &ws);
        (ran.status, ran.log.trim().to_string())
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

/// `version: latest` has to become a concrete tag before a download URL
/// exists, and the redirect it reads that from is a network call that can
/// fail. When it does, the step must say so and exit 3 — the same
/// infrastructure code its other two dead ends already use.
///
/// The abort this catches is quieter than the eval step's: the CLI never runs,
/// so there is no run to summarize and no report to upload, and the operator
/// gets a red step whose only content is the shell's own exit status.
#[test]
fn an_unresolvable_latest_release_is_an_infrastructure_error() {
    let ws = Workspace::new();
    // What `curl -f` does for a 404, and near enough what it does for a DNS or
    // TLS failure: a non-zero exit and nothing on stdout.
    ws.shim("curl", "exit 22\n");

    let ran = run_step("bin", &[("INPUT_VERSION", "latest")], &ws);

    assert!(
        ran.log.contains("::error::"),
        "a failed release lookup must be annotated, not left to the shell; \
         the step logged:\n{}",
        ran.log
    );
    assert!(
        ran.log
            .contains("could not resolve the latest domarinn release tag"),
        "the annotation must name what failed, got:\n{}",
        ran.log
    );
    assert_eq!(
        ran.status,
        Some(3),
        "the action documents 3 for an infrastructure error; curl's own exit \
         code is not one of the four the gate step knows how to render"
    );
}

/// An arch with no release asset is the binary step's other infrastructure
/// dead end. It already behaves; it is here so a fix to the `latest` path
/// cannot regress it.
#[test]
fn an_unsupported_architecture_is_an_infrastructure_error() {
    let ws = Workspace::new();
    ws.shim("uname", "echo riscv64\n");

    let ran = run_step("bin", &[], &ws);

    assert_eq!(ran.status, Some(3));
    assert!(
        ran.log.contains("::error::unsupported arch riscv64"),
        "got:\n{}",
        ran.log
    );
}

/// A concrete version with no downloadable asset is *not* an error: the step
/// falls back to building from source. That tolerance comes from wrapping the
/// download in `if curl …`, which is the shape the `latest` lookup lacks — so
/// this pins the working half of the same step.
#[test]
fn a_missing_release_asset_falls_back_to_a_source_build() {
    let ws = Workspace::new();
    ws.shim("curl", "exit 22\n");
    ws.shim("cargo", "exit 0\n");

    let ran = run_step("bin", &[("INPUT_VERSION", "0.9.9")], &ws);

    assert_eq!(
        ran.status,
        Some(0),
        "a missing asset is recoverable; the step logged:\n{}",
        ran.log
    );
    assert!(
        ran.log.contains("::warning::no release asset for 0.9.9"),
        "the fallback must be visible in the log, got:\n{}",
        ran.log
    );
    assert!(
        ran.output("path")
            .is_some_and(|p| p.ends_with("/bin/domarinn")),
        "the built binary's path is what every later step invokes, got: {:?}",
        ran.output("path")
    );
}

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
    for (case, body) in [
        ("ci-summary failed", "exit 1\n"),
        ("ci-summary exited 0 but wrote nothing", "exit 0\n"),
        ("ci-summary wrote an empty file", ": > summary.md\nexit 0\n"),
    ] {
        let ws = Workspace::new();
        let stub = ws.stub_cli(body);
        let ran = run_step("summary", &[("DOMARINN_BIN", &stub)], &ws);

        assert_eq!(
            ran.status,
            Some(0),
            "{case}: the reporter must not fail the job; it logged:\n{}",
            ran.log
        );
        assert_eq!(
            ran.output("summary-path"),
            Some("summary.md"),
            "{case}: the PR comment and the artifact upload both read this path"
        );
        assert!(
            !ran.step_summary.trim().is_empty(),
            "{case}: a blank job summary reads as 'nothing ran'; say what happened"
        );
    }
}

/// A summary the CLI did produce is passed through untouched.
#[test]
fn the_summary_step_publishes_what_the_cli_wrote() {
    let ws = Workspace::new();
    let stub = ws.stub_cli("printf '### domarinn eval\\n\\n12 passed\\n' > summary.md\n");
    let ran = run_step("summary", &[("DOMARINN_BIN", &stub)], &ws);

    assert_eq!(ran.status, Some(0));
    assert!(
        ran.step_summary.contains("12 passed"),
        "the real summary must survive to the job summary, got: {:?}",
        ran.step_summary
    );
}

/// Errexit applies to every step, so the registry must cover every step.
///
/// The first fix for this class of bug covered the two steps it was reported
/// against and left the identical abort live in a third. Opting steps in one at
/// a time reproduces that; this fails until the registry names them all.
#[test]
fn every_shell_step_is_exercised_here() {
    let in_action: BTreeSet<String> = steps()
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
    let registered: BTreeSet<String> = SHELL_STEPS.iter().map(|s| s.value.to_string()).collect();

    assert_eq!(
        in_action, registered,
        "a `run:` step this file never executes is a step whose behaviour under \
         `bash -e` is unverified — the exact gap that shipped the bug this file \
         exists for. Add it to SHELL_STEPS and give it a test."
    );
}

/// What every test above assumes about the steps it executes.
///
/// They reproduce the runner rather than call it: [`GITHUB_BASH`] is the
/// expansion of the literal `shell: bash`, and each [`ShellStep::env`] is
/// hand-written. If either drifts from `action.yml` the tests keep passing
/// while asserting about a step the runner no longer runs.
#[test]
fn the_shape_this_test_assumes_still_holds() {
    for registered in SHELL_STEPS {
        let step = step(registered.field, registered.value);
        let name = registered.value;

        assert_eq!(
            step["shell"].as_str(),
            Some("bash"),
            "{name}: GITHUB_BASH is the expansion of exactly this string; \
             changing the shell means changing it here too"
        );

        let mut declared: Vec<&str> = step["env"]
            .as_mapping()
            .unwrap_or_else(|| panic!("{name} declares an `env` block"))
            .keys()
            .filter_map(|k| k.as_str())
            .collect();
        let mut covered: Vec<&str> = registered.env.iter().map(|(k, _)| *k).collect();
        declared.sort_unstable();
        covered.sort_unstable();

        assert_eq!(
            declared, covered,
            "{name}: an input the step reads but this test never sets is an \
             input whose effect on the exit-code contract is untested"
        );
    }
}
