//! The `domarinn ci-summary` command: render a stored run as a CI-facing
//! markdown summary and expose its headline numbers as workflow step outputs.
//!
//! This is the CI layer on top of [`crate::output::render_run_md`]: the same
//! metrics table every markdown consumer gets, plus the two things only a
//! workflow can use — links back to the run, and `key=value` pairs a later step
//! can read.
//!
//! It is a **reporter, not a gate**. It exits 0 for a failing run, because the
//! verdict belongs to `run`'s exit code (see `docs/ci.md`); a summary step that
//! could also fail a job would make the same failure gate twice, and mask which
//! one actually spoke.

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::Args;
use domarinn_core::diff::RunDiff;
use domarinn_core::RunResult;

use crate::exit;
use crate::loadrun::load_run;
use crate::output;

#[derive(Args)]
pub struct CiSummaryArgs {
    /// Run to summarize: an id, `latest`, a result.json, or a run directory.
    #[arg(default_value = "latest")]
    pub run: String,

    /// Compare against a baseline run (id, path, or `latest`).
    #[arg(long)]
    pub against: Option<String>,

    /// Write the markdown to a file instead of stdout.
    #[arg(long)]
    pub out: Option<PathBuf>,

    /// Append `key=value` step outputs to this file (defaults to $GITHUB_OUTPUT
    /// when the environment sets it, so GitHub Actions needs no flag).
    #[arg(long, env = "GITHUB_OUTPUT")]
    pub github_output: Option<PathBuf>,
}

pub fn execute(args: CiSummaryArgs, server_url: Option<String>) -> u8 {
    let run = match load_run(&args.run) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::USAGE;
        }
    };

    // The baseline is best-effort *here* — unlike `run --against`, which gates.
    // A missing baseline is the normal state on a repo's first eval, and
    // refusing to summarize the run we do have would turn a routine absence into
    // an empty PR comment. Both `BaselineError` arms therefore degrade to "no
    // comparison"; only the severity differs in the log.
    //
    // It still routes through `baseline::resolve` rather than `load_run` so that
    // `latest` means the newest run *of this suite*, not the newest run of any
    // suite — a summary that compared two unrelated suites would report
    // confident nonsense.
    let baseline = args.against.as_deref().and_then(|reference| {
        match crate::baseline::resolve(reference, &run, server_url.as_deref()) {
            Ok(base) => {
                let diff = domarinn_core::diff_runs(&base, &run);
                Some((base, diff))
            }
            Err(crate::baseline::BaselineError::Absent(msg)) => {
                tracing::info!("--against: {msg}; summarizing without a comparison");
                None
            }
            Err(crate::baseline::BaselineError::Failed(msg)) => {
                tracing::warn!("--against: {msg}; summarizing without a comparison");
                None
            }
        }
    });
    let comparison = baseline.as_ref().map(|(base, diff)| (base, diff));

    let markdown = render(&run, comparison);
    if let Err(e) = write_markdown(&markdown, args.out.as_deref()) {
        eprintln!("error writing summary: {e}");
        return exit::INFRA;
    }

    if let Some(path) = &args.github_output {
        let diff = comparison.map(|(_, d)| d);
        if let Err(e) = write_github_output(path, &run, diff) {
            // Losing step outputs degrades a workflow; it does not invalidate
            // the summary already written above, so warn rather than fail.
            tracing::warn!(error = %e, "could not write step outputs");
        }
    }

    exit::OK
}

/// The full CI summary: headline metrics, then either the baseline comparison
/// or this run's own failures, then the links footer.
///
/// Shared with `run --summary-md` so a hand-rolled pipeline and the reusable
/// action emit the same document.
pub fn render(run: &RunResult, comparison: Option<(&RunResult, &RunDiff)>) -> String {
    let mut out = output::render_run_md_headline(run);
    match comparison {
        // The comparison tables newly-failing cases with base→head scores,
        // which strictly supersedes the flat failure table.
        Some((base, diff)) => {
            out.push('\n');
            out.push_str(&crate::diffrender::render_markdown(base, run, diff));
        }
        None => out.push_str(&output::render_failures_md(run)),
    }
    out.push_str(&links_md(run));
    out
}

/// `[View run](…) · [CI run](…)`, omitting either half that is unknown and the
/// whole line when neither is.
fn links_md(run: &RunResult) -> String {
    let mut links = Vec::new();
    if let Some(url) = nonempty(run.share_url.as_deref()) {
        links.push(format!("[View run]({url})"));
    }
    if let Some(url) = ci_run_url(run) {
        links.push(format!("[CI run]({url})"));
    }
    if links.is_empty() {
        return String::new();
    }
    format!("\n{}\n", links.join(" · "))
}

/// The workflow run this eval belongs to.
///
/// The value recorded on the run wins: it describes the workflow that produced
/// it, whereas the ambient environment describes whoever is summarizing now.
/// Those differ when an old run is summarized from a later job, and labelling
/// last week's eval with today's run URL would be a lie.
fn ci_run_url(run: &RunResult) -> Option<String> {
    run.ci
        .as_ref()
        .and_then(|c| nonempty(c.run_url.as_deref()))
        .map(String::from)
        .or_else(|| domarinn_core::provenance::collect_ci()?.run_url)
        .filter(|u| !u.is_empty())
}

fn nonempty(s: Option<&str>) -> Option<&str> {
    s.filter(|v| !v.is_empty())
}

fn write_markdown(markdown: &str, out: Option<&Path>) -> std::io::Result<()> {
    match out {
        Some(path) => std::fs::write(path, markdown),
        None => std::io::stdout().write_all(markdown.as_bytes()),
    }
}

/// Append the run's headline numbers as GitHub Actions step outputs.
///
/// Appends because a job's steps all share one `$GITHUB_OUTPUT` file —
/// truncating it would silently discard the outputs of every earlier step.
fn write_github_output(
    path: &Path,
    run: &RunResult,
    diff: Option<&RunDiff>,
) -> std::io::Result<()> {
    let s = &run.summary;
    let pct = |n: u64, d: u64| {
        if d == 0 {
            "0.0".to_string()
        } else {
            format!("{:.1}", n as f64 / d as f64 * 100.0)
        }
    };
    let cache_total = s.cache_hits + s.cache_misses;

    let pairs: Vec<(&str, String)> = vec![
        ("passed", s.passed.to_string()),
        ("failed", s.failed.to_string()),
        ("errored", s.errored.to_string()),
        // The action's long-standing `failed` output means "failed or errored";
        // keeping that sum available under its own key lets the action preserve
        // its contract without either side lying about what it counted.
        ("failed-or-errored", (s.failed + s.errored).to_string()),
        ("total", s.total.to_string()),
        ("pass-rate", pct(s.passed, s.total)),
        ("cache-hits", s.cache_hits.to_string()),
        ("cache-misses", s.cache_misses.to_string()),
        ("cache-hit-rate", pct(s.cache_hits, cache_total)),
        (
            "regressed",
            diff.map_or(0, |d| d.summary.newly_failing).to_string(),
        ),
        // Emitted even when unknown: a workflow that references an output no
        // step ever wrote gets an empty string anyway, so writing the key makes
        // the contract visible instead of implicit.
        ("run-url", run.share_url.clone().unwrap_or_default()),
        ("ci-run-url", ci_run_url(run).unwrap_or_default()),
    ];

    let mut body = String::new();
    for (key, value) in pairs {
        // `$GITHUB_OUTPUT` is line-oriented: an embedded newline would be read
        // as the start of another key and could inject an output we never set.
        body.push_str(&format!("{key}={}\n", value.replace(['\n', '\r'], " ")));
    }

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(body.as_bytes())
}
