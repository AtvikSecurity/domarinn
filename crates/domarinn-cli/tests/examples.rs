//! Every shipped example, run end to end through the real binary.
//!
//! The examples under `examples/` are transcluded into the documentation with
//! pymdownx snippets, so a page and this test read the *same bytes*: a page
//! cannot drift from a working suite without something here going red.
//!
//! Loading and validating is deliberately not enough — `examples_roundtrip.rs`
//! in domarinn-core already does that. A suite that loads and validates can
//! still call an endpoint that does not exist, assert on a var that never
//! renders, or produce four cells where its page promises seven.

mod common;

// `#[path]` rather than a bare `mod table;`: that would resolve to
// `tests/table.rs`, which Cargo auto-discovers as a *separate* test target. A
// `tests/examples/` directory with no `main.rs` is not auto-discovered, so the
// whole harness links as one binary.
#[path = "examples/docs_guards.rs"]
mod docs_guards;
#[path = "examples/spec.rs"]
mod spec;
#[path = "examples/stub_contract.rs"]
mod stub_contract;
#[path = "examples/stubs.rs"]
mod stubs;
#[path = "examples/table.rs"]
mod table;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use common::stub_script;
use spec::{Cells, Env, Example, Step};
use table::EXAMPLES;

// ---------------------------------------------------------------------------
// The example harness
// ---------------------------------------------------------------------------

/// Environment a developer plausibly has exported that would change what these
/// runs do — or spend their money.
///
/// Scrubbed for the same reason `common::bin` scrubs the CI variables: a test
/// that reads the host's environment passes on one machine and fails on
/// another. Here the stakes are higher than a wrong assertion. A row that
/// redirects `ANTHROPIC_BASE_URL` at a stub is only offline because *this row*
/// sets it; inheriting a real one from the shell would send the request to the
/// vendor with the shell's real key attached.
const HOST_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_BASE_URL",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "OPENAI_MODEL",
    "OPENAI_EMBED_MODEL",
    "DOMARINN_SERVER_URL",
    "DOMARINN_TOKEN",
    "DOMARINN_CACHE_DIR",
    "DOMARINN_SMOKE_BASE_URL",
    "DOMARINN_SMOKE_MODEL",
    "DOMARINN_SMOKE_API_KEY",
    "ORDERS_API_URL",
    "CLAUDE_GATEWAY_URL",
    "CLAUDE_OAUTH_TOKEN",
    "MODEL_TIER",
    "TENANT_TOKEN",
    "DOMARINN_PROVIDER_HEADERS",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root resolves from crates/domarinn-cli")
}

fn examples_root() -> PathBuf {
    repo_root().join("examples")
}

fn scrubbed_bin() -> Command {
    let mut cmd = common::bin();
    for key in HOST_ENV {
        cmd.env_remove(key);
    }
    cmd
}

/// A missing interpreter is not an example failure, and must not read like one.
///
/// Without this, `python3` absent turns every python-backed row red at once
/// with "provider error" and nothing anywhere naming the cause.
///
/// A `.py` path counts as much as the word `python3`: a suite whose `command` is
/// the script itself relies on its `#!/usr/bin/env python3` shebang, so it needs
/// the interpreter just as much while never spelling it out. The converted suite
/// in `39-import-promptfoo` is that shape — and it is the converter's output, so
/// it cannot be reworded to say `python3`.
///
/// The selector is a plain text search over the whole file, prose included, and
/// that is deliberate: it can only over-include, and over-including costs one
/// `python3 --version` on a machine that has it anyway. Contrast [`invokes_jq`],
/// which cannot afford the same looseness.
#[test]
fn python3_is_available_for_the_examples_that_need_it() {
    let users: Vec<&str> = EXAMPLES
        .iter()
        .filter(|e| {
            std::fs::read_to_string(examples_root().join(e.dir).join("domarinn.yaml"))
                .map(|s| s.contains("python3") || names_a_py_file(&s))
                .unwrap_or(false)
        })
        .map(|e| e.dir)
        .collect();
    if users.is_empty() {
        return;
    }
    let ok = Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    assert!(
        ok,
        "python3 is not on PATH, but {} shipped example(s) use it as their \
         system under test: {users:?}",
        users.len()
    );
}

/// Whether `text` names a `.py` file, rather than merely containing those three
/// bytes: `.py` has to end the path component, so `../echo-provider.py` and a
/// quoted `"gen.py"` count while `.pyc`, `.pyi` and `.pyproject` do not.
fn names_a_py_file(text: &str) -> bool {
    text.match_indices(".py").any(|(at, hit)| {
        text[at + hit.len()..]
            .chars()
            .next()
            .is_none_or(|next| !next.is_alphanumeric() && next != '_')
    })
}

/// Whether any non-comment line in any file directly under `dir` mentions
/// `jq`.
///
/// Deliberately not "does `domarinn.yaml` contain the word jq": "jq" also shows
/// up in this ladder's own prose (a header comment explaining why a script uses
/// it). What this relies on, and what the python3 guard above does not need, is
/// that the selector be non-vacuous — [`jq_is_available_for_the_examples_that_need_it`]
/// asserts it matched something, so a prose match would keep that assertion
/// green the day the last real `jq` invocation disappeared, guarding nothing
/// while looking correct. Excluding comment lines (`#`, the marker in both YAML
/// and bash) leaves only real command lines in a provider script.
fn invokes_jq(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let path = entry.path();
        path.is_file()
            && std::fs::read_to_string(&path).is_ok_and(|text| {
                text.lines()
                    .map(str::trim_start)
                    .filter(|line| !line.starts_with('#'))
                    .any(|line| line.contains("jq"))
            })
    })
}

/// A missing `jq` is not an example failure, and must not read like one.
///
/// Mirrors [`python3_is_available_for_the_examples_that_need_it`]: without
/// this, `jq` absent turns the bash-backed row red with "provider error" and
/// nothing anywhere naming the cause. `jq` is pinned in `.mise/config.toml`,
/// so a `mise`-run shell always has it — this only catches a bare
/// `cargo test` run outside one.
#[test]
fn jq_is_available_for_the_examples_that_need_it() {
    let users: Vec<&str> = EXAMPLES
        .iter()
        .filter(|e| invokes_jq(&examples_root().join(e.dir)))
        .map(|e| e.dir)
        .collect();
    // Guard against a vacuous pass: unlike the python3 guard (which returns
    // early when nothing uses it, because that was true before example 13
    // shipped), this repository has shipped a jq-backed example since the
    // day this test was added — so an empty selector here always means the
    // selector broke, never that the guard has nothing to do.
    assert!(
        !users.is_empty(),
        "no shipped example's non-comment lines mention jq — either \
         37-exec-provider-bash stopped using it or `invokes_jq` no longer \
         matches it, and either way this guard is currently vacuous"
    );
    let ok = Command::new("jq")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    assert!(
        ok,
        "jq is not on PATH, but {} shipped example(s) use it as their system \
         under test: {users:?}",
        users.len()
    );
}

#[test]
fn every_example_behaves_as_its_row_claims() {
    // One test over the table rather than one `#[test]` per example: a row is
    // data, and generating per-row tests would need a macro whose expansion is
    // harder to read than this loop.
    assert!(!EXAMPLES.is_empty(), "the example table is empty");
    for spec in EXAMPLES.iter().copied() {
        verify(spec);
    }
}

/// The scoring example states arithmetic in its comments; this checks it.
///
/// `05-weights-and-thresholds` tells the reader that a partial-credit case
/// scores `(1 + 1 + 0) / 3 = 0.667` and a weighted one `(1*3 + 0*1) / 4 = 0.75`.
/// The table row can only see that both cases *passed*, which they would go on
/// doing if the weighted mean changed, if a threshold stopped being read, or if
/// the failing assertion inside each quietly started passing. Those numbers are
/// the page's actual claim, so they are what gets asserted.
#[test]
fn example_05_scores_are_what_its_comments_claim() {
    let run = run_example("05-weights-and-thresholds");
    let score = |id: &str| {
        run.cases
            .iter()
            .find(|c| c.cell.test_id == id)
            .unwrap_or_else(|| panic!("case `{id}` is missing from the run"))
            .score
    };

    // Guard against a vacuous pass: each of these cases must contain a failing
    // assertion. Without one, the means below are trivially 1.0 and the example
    // would demonstrate nothing about partial credit.
    for id in ["gate/partial-credit", "gate/weighted"] {
        let case = run.cases.iter().find(|c| c.cell.test_id == id).unwrap();
        assert!(
            case.asserts.iter().any(|a| a.score == 0.0),
            "`{id}` is supposed to pass *despite* a failing assertion, but every \
             assertion in it passed — the example no longer shows partial credit"
        );
    }

    assert!(
        (score("gate/partial-credit") - 2.0 / 3.0).abs() < 1e-9,
        "partial-credit scored {}, the page says (1 + 1 + 0) / 3",
        score("gate/partial-credit")
    );
    assert!(
        (score("gate/weighted") - 0.75).abs() < 1e-9,
        "weighted scored {}, the page says (1*3 + 0*1) / 4",
        score("gate/weighted")
    );
}

/// Run one shipped example against a scripted stub with extra environment,
/// asserting success and the exact call count, and hand back the full requests
/// the stub served.
///
/// This exists for the `${env:…}` override invocations the example pages
/// document. The table rows exercise only the `:-default` branch — HOST_ENV
/// scrubs the variables — so without these runs a typo'd variable name, or a
/// quiet fall back to a literal, would keep CI green while every documented
/// override invocation silently tested the default.
fn run_with_env(
    dir: &str,
    routes: Vec<(&'static str, Vec<String>)>,
    calls: usize,
    env: &[(&str, &str)],
) -> Vec<String> {
    let (url, server) = stub_script(routes, calls, Duration::from_secs(30));
    let tmp = tempfile::tempdir().expect("scratch directory");
    let mut cmd = scrubbed_bin();
    cmd.args(["run"])
        .arg(examples_root().join(dir))
        .args(["--format", "json", "--no-progress", "--out"])
        .arg(tmp.path().join("result.json"))
        .arg("--cache-dir")
        .arg(tmp.path().join("cache"))
        .env("OPENAI_BASE_URL", format!("{url}/v1"))
        .env("OPENAI_API_KEY", "sk-stub-not-a-real-key")
        .current_dir(tmp.path());
    for (key, value) in env {
        cmd.env(key, value);
    }
    let out = cmd.output().expect("the domarinn binary runs");
    assert!(
        out.status.success(),
        "`domarinn run {dir}` with {env:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let served = server.join().expect("the stub thread does not panic");
    assert_eq!(
        served.len(),
        calls,
        "example `{dir}` with {env:?} made {} stub request(s), expected {calls}",
        served.len()
    );
    served
}

/// Example 26's page documents `OPENAI_MODEL=… domarinn run …` as the way to
/// point the suite at another model. Prove the override survives load-time
/// interpolation all the way into the request body — the row above can only
/// ever see the default, so this is the one place the documented invocation is
/// actually exercised.
#[test]
fn example_26_env_overridden_model_reaches_the_wire() {
    let served = run_with_env(
        "26-openai-provider",
        vec![(
            "/chat/completions",
            vec![
                stubs::OPENAI_TEXT.to_string(),
                stubs::OPENAI_TEXT_ALT.to_string(),
            ],
        )],
        2,
        &[("OPENAI_MODEL", "stub-chat-override")],
    );
    for request in &served {
        assert!(
            request.contains(r#""model":"stub-chat-override""#),
            "the overridden model never reached the request body: {request:?}"
        );
    }
}

/// The embeddings twin of the test above: example 30's `similar` assertion
/// documents `OPENAI_EMBED_MODEL` as its override.
#[test]
fn example_30_env_overridden_embed_model_reaches_the_wire() {
    let served = run_with_env(
        "30-similar-embeddings",
        vec![(
            "/embeddings",
            vec![stubs::EMBED_A.to_string(), stubs::EMBED_NEAR_A.to_string()],
        )],
        2,
        &[("OPENAI_EMBED_MODEL", "stub-embed-override")],
    );
    for request in &served {
        assert!(
            request.contains(r#""model":"stub-embed-override""#),
            "the overridden embeddings model never reached the request body: {request:?}"
        );
    }
}

/// Example 39 ships both halves of a promptfoo migration, and the second half is
/// a claim about a program: `examples/39-import-promptfoo/domarinn.yaml` says it
/// is what `domarinn import promptfoo` prints for the config beside it. So it is
/// checked against the converter — and then run, because "it converted" is not
/// "it works".
///
/// Neither half fits the example table. The converter writes to stdout and takes
/// no output path, so a table row can only assert its exit code; the row does
/// that, and this test does the two things that need the stdout itself.
#[test]
fn example_39_the_committed_conversion_is_the_converters_output_and_runs() {
    let dir = examples_root().join("39-import-promptfoo");
    let out = scrubbed_bin()
        .args(["import", "promptfoo"])
        .arg(dir.join("promptfooconfig.yaml"))
        .output()
        .expect("the domarinn binary runs");
    assert!(
        out.status.success(),
        "`domarinn import promptfoo` failed on the shipped config:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8(out.stdout).expect("the converter prints utf-8");
    let committed =
        std::fs::read_to_string(dir.join("domarinn.yaml")).expect("the committed conversion ships");

    // The `# NOTE:` lines are the point of the pair: two assertions in that
    // promptfoo config have no faithful equivalent, and the guide tells readers
    // to read the notes before running anything. A conversion that silently
    // stopped emitting them would still parse, still run, and still be green.
    let notes: Vec<&str> = printed
        .lines()
        .filter(|l| l.starts_with("# NOTE:"))
        .collect();
    assert_eq!(
        notes.len(),
        2,
        "the shipped promptfoo config carries two deliberately unmappable \
         assertions (`not-icontains` and `javascript`), so the converter must \
         emit two notes; it emitted: {notes:?}"
    );
    for note in &notes {
        assert!(
            committed.contains(note),
            "the committed conversion dropped a converter note: {note:?}"
        );
    }

    // Compared as YAML rather than as bytes: the committed file carries a header
    // saying where it came from, and this repository's formatter indents its
    // sequences. Neither changes the suite, and a byte comparison would reject
    // the file for saying what it is.
    let parse = |what: &str, text: &str| {
        serde_yaml_ng::from_str::<serde_yaml_ng::Value>(text)
            .unwrap_or_else(|e| panic!("{what} is not valid YAML: {e}"))
    };
    assert_eq!(
        parse("the converter's output", &printed),
        parse("the committed conversion", &committed),
        "examples/39-import-promptfoo/domarinn.yaml is no longer what \
         `domarinn import promptfoo promptfooconfig.yaml` prints.\n\
         If crates/domarinn-cli/src/import.rs changed on purpose, regenerate the \
         file (keeping its header) and commit it — the example's whole claim is \
         that it is the tool's output, not a hand-written suite."
    );

    // And the printed suite runs, not just the committed one. Written into a
    // scratch tree with the shared echo provider beside it, because the
    // converted `command` is `../echo-provider.py` — resolved relative to the
    // suite file, exactly as it is in the example directory.
    let tmp = tempfile::tempdir().expect("scratch directory");
    std::fs::copy(
        examples_root().join("echo-provider.py"),
        tmp.path().join("echo-provider.py"),
    )
    .expect("the shared echo provider copies, mode included");
    let suite_dir = tmp.path().join("converted");
    std::fs::create_dir(&suite_dir).expect("scratch suite directory");
    std::fs::write(suite_dir.join("domarinn.yaml"), &printed).expect("the conversion writes");

    let result = tmp.path().join("result.json");
    let run = scrubbed_bin()
        .args(["run"])
        .arg(&suite_dir)
        .args(["--format", "json", "--no-progress", "--out"])
        .arg(&result)
        .arg("--cache-dir")
        .arg(tmp.path().join("cache"))
        .current_dir(tmp.path())
        .output()
        .expect("the domarinn binary runs");
    assert!(
        run.status.success(),
        "the converter's own output did not run:\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let doc: domarinn_core::result::RunResult = serde_json::from_str(
        &std::fs::read_to_string(&result).expect("the run wrote a result document"),
    )
    .expect("the result document parses");
    let ids: BTreeSet<&str> = doc.cases.iter().map(|c| c.cell.test_id.as_str()).collect();
    assert_eq!(
        (doc.summary.passed, doc.summary.total),
        (2, 2),
        "the converted suite ran {} of {} cells green",
        doc.summary.passed,
        doc.summary.total
    );
    assert_eq!(
        ids,
        BTreeSet::from(["case-0", "case-1"]),
        "the converted suite's case ids are the converter's, and the table row \
         for the committed copy claims the same two"
    );
}

/// Run one shipped example offline and hand back its result document.
fn run_example(dir: &str) -> domarinn_core::result::RunResult {
    let tmp = tempfile::tempdir().expect("scratch directory");
    let out = tmp.path().join("result.json");
    scrubbed_bin()
        .args(["run"])
        .arg(examples_root().join(dir))
        .args(["--format", "json", "--no-progress", "--out"])
        .arg(&out)
        .arg("--cache-dir")
        .arg(tmp.path().join("cache"))
        .current_dir(tmp.path())
        .output()
        .expect("the domarinn binary runs");
    let text = std::fs::read_to_string(&out)
        .unwrap_or_else(|e| panic!("example `{dir}` wrote no result document: {e}"));
    serde_json::from_str(&text).expect("the result document parses")
}

fn verify(spec: &Example) {
    let tmp = tempfile::tempdir().expect("scratch directory");
    let dir = examples_root().join(spec.dir);
    let stub = (!spec.stub.is_empty()).then(|| {
        let routes = spec
            .stub
            .iter()
            .map(|r| (r.fragment, r.bodies.iter().map(|b| b.to_string()).collect()))
            .collect();
        stub_script(routes, spec.stub_calls, Duration::from_secs(30))
    });
    let stub_url = stub.as_ref().map(|(url, _)| url.clone());

    for (n, step) in spec.steps.iter().enumerate() {
        let argv = substitute(step.argv, &dir, tmp.path());
        let mut cmd = scrubbed_bin();
        // cwd is the scratch directory because the local run store is
        // cwd-relative; left at the repo root, every example would litter
        // `.domarinn/runs/` into the working tree.
        cmd.args(&argv).current_dir(tmp.path());
        for (key, value) in spec.env {
            cmd.env(key, resolve_env(value, stub_url.as_deref(), spec));
        }
        let out = cmd.output().expect("the domarinn binary runs");
        let ctx = Context {
            spec,
            step: n,
            argv: &argv,
            cwd: tmp.path(),
            out: &out,
        };

        let code = out.status.code().unwrap_or(-1);
        if code != i32::from(step.exit) {
            ctx.fail(&format!(
                "exited {code} ({}), expected {} ({})",
                exit_meaning(u8::try_from(code).unwrap_or(u8::MAX)),
                step.exit,
                exit_meaning(step.exit),
            ));
        }

        // Side files first: a step may produce only these (a JUnit report, a
        // Markdown summary) and assert nothing about cells at all.
        for relative in step.writes {
            let path = PathBuf::from(substitute(&[relative], &dir, tmp.path()).remove(0));
            // Non-empty, not merely present: every writer here creates its file
            // before it has anything to put in it, so existence alone would
            // pass against a reporter that produced nothing.
            match std::fs::metadata(&path) {
                Ok(meta) if meta.len() > 0 => {}
                Ok(_) => ctx.fail(&format!("{} was written but is empty", path.display())),
                Err(e) => ctx.fail(&format!("{} was not written: {e}", path.display())),
            }
        }

        // The result document, only when the row makes a claim that needs it.
        // `Cells::NONE` with no other claim means "this step writes no result
        // document I care about" — a `validate`, or a step whose whole subject
        // is a side file in a format that is not the result JSON.
        let claims_result = step.cells.total() > 0 || step.cache_hits > 0 || step.priced;
        match (claims_result, result_path(&argv)) {
            (true, Some(path)) => check_result(&ctx, step, &path),
            (true, None) => ctx.fail(
                "the row makes a claim about cells, but this step writes no JSON \
                 result document (its argv has no `--format json --out …`)",
            ),
            (false, _) => {}
        }
    }

    if let Some((_, handle)) = stub {
        let served = handle.join().expect("the stub thread does not panic");
        // The egress guard. Fewer calls than declared means the example never
        // reached the stub — almost always because `${env:…}` stopped
        // redirecting `base_url`, which in CI means the request went to the
        // real vendor.
        // Request lines only: the recorded requests carry their whole bodies,
        // which on a miscount would bury the useful part of this message.
        let lines: Vec<&str> = served
            .iter()
            .map(|r| r.lines().next().unwrap_or_default())
            .collect();
        assert_eq!(
            served.len(),
            spec.stub_calls,
            "example `{}` made {} request(s) to the stub, its row declares {}.\n\
             Served:\n  {}\n\
             If this is 0 the suite is talking to a real endpoint: check that \
             examples/{}/domarinn.yaml still carries a `${{env:…}}` base_url and \
             that the row sets the matching variable.",
            spec.dir,
            served.len(),
            spec.stub_calls,
            lines.join("\n  "),
            spec.dir,
        );
    }
}

fn substitute(argv: &[&str], dir: &Path, tmp: &Path) -> Vec<String> {
    let (dir, tmp) = (dir.display().to_string(), tmp.display().to_string());
    argv.iter()
        .map(|a| a.replace("{dir}", &dir).replace("{tmp}", &tmp))
        .collect()
}

/// The JSON result document this step writes, if it writes one.
///
/// Read back out of the argv rather than assumed, so a row that changes the
/// flags cannot leave the harness checking a file nobody wrote. Restricted to
/// `.json` because `--out` also carries the JUnit report, and handing that to
/// a `RunResult` deserializer reports a parse error where the real answer is
/// "that is not the result document".
fn result_path(argv: &[String]) -> Option<PathBuf> {
    argv.iter()
        .position(|a| a == "--out")
        .and_then(|i| argv.get(i + 1))
        .filter(|p| p.ends_with(".json"))
        .map(PathBuf::from)
}

fn resolve_env(value: &Env, stub_url: Option<&str>, spec: &Example) -> String {
    let base = || {
        stub_url.unwrap_or_else(|| {
            panic!(
                "example `{}` has a row using a stub URL but declares no stub routes",
                spec.dir
            )
        })
    };
    match value {
        Env::Literal(v) => (*v).to_string(),
        Env::StubBase => base().to_string(),
        Env::StubBaseV1 => format!("{}/v1", base()),
    }
}

fn exit_meaning(code: u8) -> &'static str {
    match code {
        0 => "ok",
        1 => "an assertion failed",
        2 => "usage error",
        3 => "infrastructure error",
        _ => "not an exit code domarinn defines",
    }
}

struct Context<'a> {
    spec: &'a Example,
    step: usize,
    argv: &'a [String],
    cwd: &'a Path,
    out: &'a Output,
}

impl Context<'_> {
    /// Everything needed to reproduce and diagnose, in one panic.
    ///
    /// Not `assert_cmd`'s own message: that shows the streams but not the
    /// example, the command, or where the row lives — and the first question on
    /// a red build is always "which example, and how do I run it".
    fn fail(&self, what: &str) -> ! {
        // A networked example fails this way for one overwhelmingly likely
        // reason, and the generic message buries it. The stub-count assertion
        // that names it outright only runs after every step, so a step that
        // dies first would otherwise report "exited 3" and leave the reader to
        // work out that the request went to the vendor.
        let egress_hint = if self.spec.stub.is_empty() {
            String::new()
        } else {
            format!(
                "\n  NOTE: this example is supposed to reach a local stub, not the \
                 internet.\n        If examples/{}/domarinn.yaml stopped carrying a \
                 `${{env:…}}` base_url,\n        or the row stopped setting the \
                 matching variable, the request went to\n        the real vendor — \
                 which errors in CI and SPENDS MONEY on a machine that\n        has a \
                 key exported. Check that first.\n",
                self.spec.dir
            )
        };
        panic!(
            "\nexample `{dir}` failed: {what}\n\
             \n  shows:   {shows}\
             \n  step:    {step} of {steps}\
             \n  command: domarinn {argv}\
             \n  cwd:     {cwd}\
             \n  row:     crates/domarinn-cli/tests/examples/table.rs (dir = \"{dir}\")\
             \n  file:    examples/{dir}/domarinn.yaml\n\
             {egress_hint}\
             \n--- stdout ---\n{stdout}\
             \n--- stderr ---\n{stderr}\n\
             \nIf the example changed on purpose, update its row. If it did not, \
             the example is broken — and so is the page that transcludes it.\n",
            dir = self.spec.dir,
            shows = self.spec.shows,
            step = self.step + 1,
            steps = self.spec.steps.len(),
            argv = self.argv.join(" "),
            cwd = self.cwd.display(),
            stdout = String::from_utf8_lossy(&self.out.stdout),
            stderr = String::from_utf8_lossy(&self.out.stderr),
        )
    }
}

fn check_result(ctx: &Context, step: &Step, path: &Path) {
    let Ok(text) = std::fs::read_to_string(path) else {
        ctx.fail("the step wrote no result document — did the run reach the output stage?")
    };
    let run: domarinn_core::result::RunResult = match serde_json::from_str(&text) {
        Ok(r) => r,
        Err(e) => ctx.fail(&format!("the result document did not parse: {e}")),
    };

    let got = Cells {
        passed: run.summary.passed,
        failed: run.summary.failed,
        errored: run.summary.errored,
        skipped: run.summary.skipped,
        xfailed: run.summary.xfailed,
        xpassed: run.summary.xpassed,
    };
    if got != step.cells {
        // A table, not a struct dump: the reader is comparing four numbers and
        // wants to see which one moved.
        ctx.fail(&format!(
            "cell tallies differ\n\
             \n  |       | expected | actual |\
             \n  | pass  | {:>8} | {:>6} |\
             \n  | fail  | {:>8} | {:>6} |\
             \n  | error | {:>8} | {:>6} |\
             \n  | skip  | {:>8} | {:>6} |\
             \n  | xfail | {:>8} | {:>6} |\
             \n  | xpass | {:>8} | {:>6} |\
             \n\n  not passing: {}",
            step.cells.passed,
            got.passed,
            step.cells.failed,
            got.failed,
            step.cells.errored,
            got.errored,
            step.cells.skipped,
            got.skipped,
            step.cells.xfailed,
            got.xfailed,
            step.cells.xpassed,
            got.xpassed,
            not_passing(&run),
        ));
    }
    // From the document, not the sum above: a status this harness does not
    // model would otherwise vanish silently.
    if run.summary.total != step.cells.total() {
        ctx.fail(&format!(
            "summary.total is {} but the six statuses account for {} — the run \
             carries a case status this harness does not model",
            run.summary.total,
            step.cells.total()
        ));
    }

    let got_ids: BTreeSet<&str> = run.cases.iter().map(|c| c.cell.test_id.as_str()).collect();
    let want_ids: BTreeSet<&str> = step.case_ids.iter().copied().collect();
    if got_ids != want_ids {
        ctx.fail(&format!(
            "case ids differ\n  missing:    {:?}\n  unexpected: {:?}",
            want_ids.difference(&got_ids).collect::<Vec<_>>(),
            got_ids.difference(&want_ids).collect::<Vec<_>>(),
        ));
    }

    if run.summary.cache_hits < step.cache_hits {
        ctx.fail(&format!(
            "the row expects at least {} cache hit(s), the run reports {}. \
             A caching example whose second run re-does the work documents \
             nothing — check the two steps share one --cache-dir and that \
             nothing in the suite varies between them.",
            step.cache_hits, run.summary.cache_hits
        ));
    }

    // `AssertKind::Cost` passes when nothing priced the call, so an example
    // whose page says "this run stayed under budget" can be green while
    // enforcing nothing.
    if step.priced {
        match run.summary.cost_usd {
            Some(c) if c > 0.0 => {}
            other => ctx.fail(&format!(
                "the row claims this example enforces a cost budget, but the run \
                 priced itself at {other:?}. Either the stub omits `usage`, or \
                 the model is not in the pricing table and the suite sets no \
                 `pricing:` block — in which case every `cost:` assertion on the \
                 page passed as \"cost not reported\"."
            )),
        }
    }
}

/// The cases that did not pass, with the first reason each gave.
fn not_passing(run: &domarinn_core::result::RunResult) -> String {
    use domarinn_core::result::{AssertStatus, CaseStatus};
    let rows: Vec<String> = run
        .cases
        .iter()
        .filter(|c| c.status != CaseStatus::Pass)
        .map(|c| {
            let why = c
                .asserts
                .iter()
                .find(|a| matches!(a.status, AssertStatus::Fail | AssertStatus::Error))
                .map(|a| format!("{:?}: {}", a.kind, a.reason))
                .unwrap_or_else(|| "no failing assertion recorded".to_string());
            format!("{} [{}] {why}", c.cell.test_id, c.status.as_str())
        })
        .collect();
    if rows.is_empty() {
        "(none)".to_string()
    } else {
        format!("\n    {}", rows.join("\n    "))
    }
}
