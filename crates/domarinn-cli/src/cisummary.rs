//! The `domarinn ci-summary` command: render a stored run as a CI-facing
//! markdown summary and expose its headline numbers as workflow step outputs.
//!
//! This is the CI layer on top of [`crate::outputmd::render_run_md`]: the same
//! metrics table every markdown consumer gets, plus the two things only a
//! workflow can use — links back to the run, and `key=value` pairs a later step
//! can read.
//!
//! It is a **reporter, not a gate**. It exits 0 for a failing run, because the
//! verdict belongs to `run`'s exit code (see `docs/guides/gate-in-ci.md`); a
//! summary step that could also fail a job would make the same failure gate
//! twice, and mask which one actually spoke.

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::Args;
use domarinn_core::diff::RunDiff;
use domarinn_core::RunResult;

use crate::exit;
use crate::loadrun::load_run;
use crate::outputmd;

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
    // The same default the gate applies: with no flag, the suite's
    // `baseline.branch` — read from the run's own config snapshot, because
    // ci-summary has no suite file, only the stored run.
    let snapshot_branch = run
        .config_snapshot
        .get("baseline")
        .and_then(|b| b.get("branch"))
        .and_then(|b| b.as_str())
        .map(str::to_string);
    let reference = crate::baseline::effective(
        args.against.as_deref(),
        snapshot_branch.as_deref(),
        crate::baseline::server_configured(server_url.as_deref()),
    );
    let baseline = reference.as_deref().and_then(|reference| {
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
    let mut out = outputmd::render_run_md_headline(run);
    match comparison {
        // The comparison tables newly-failing cases with base→head scores,
        // which strictly supersedes the flat failure table.
        Some((base, diff)) => {
            out.push('\n');
            out.push_str(&crate::diffmd::render_markdown(base, run, diff));
        }
        None => out.push_str(&outputmd::render_failures_md(run)),
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
        // `xpassed` is deliberately NOT folded in: that sum's meaning is
        // documented and consumers gate on the exit code, which already goes
        // red on an xpass.
        ("failed-or-errored", (s.failed + s.errored).to_string()),
        // Always written, zeros included, like every counter here.
        ("xfailed", s.xfailed.to_string()),
        ("xpassed", s.xpassed.to_string()),
        ("total", s.total.to_string()),
        ("pass-rate", pct(s.passed, s.total)),
        ("cache-hits", s.cache_hits.to_string()),
        ("cache-misses", s.cache_misses.to_string()),
        ("cache-hit-rate", pct(s.cache_hits, cache_total)),
        ("cache-read-tokens", s.cache_read_tokens.to_string()),
        ("cache-write-tokens", s.cache_write_tokens.to_string()),
        // Always written, like every counter above: a workflow that reads this
        // to decide whether the run is worth trusting needs `0` to mean "nothing
        // fell back", not "this CLI is too old to say".
        //
        // Re-derived from the cases (`ci-summary` always loads the whole run
        // document) rather than read off `s.fallback_cases`, which was written
        // skip-inclusively before 0.10.1 — summarizing an old document must not
        // report skipped cases as fallback-answered.
        (
            "fallback-cases",
            crate::output::graded_fallback_cases(&run.cases).to_string(),
        ),
        // Same "emit even when unknown" rule as `run-url` below: a referenced
        // output that no step wrote is an empty string either way, so writing
        // the key makes the contract visible.
        (
            "cost-usd",
            s.cost_usd.map(|c| format!("{c:.6}")).unwrap_or_default(),
        ),
        (
            "cache-savings-usd",
            s.cache_savings_usd
                .map(|c| format!("{c:.6}"))
                .unwrap_or_default(),
        ),
        (
            "grader-cost-usd",
            s.grader_cost_usd
                .map(|c| format!("{c:.6}"))
                .unwrap_or_default(),
        ),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A run document stored by CLI ≤ 0.10.0 counted skipped cases in
    /// `summary.fallback_cases`. The step output a workflow reads must be the
    /// graded count derived from the cases — 0 here, because every case was
    /// skipped — not the stored 2.
    #[test]
    fn github_output_reports_the_graded_fallback_count_for_an_old_document() {
        let run = crate::output::old_skip_inclusive_run();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gh.txt");
        write_github_output(&path, &run, None).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("fallback-cases=0\n"), "got:\n{out}");
    }

    /// The new semantics still round-trip: a graded case a fallback answered is
    /// counted, so the re-derivation did not simply zero the output.
    #[test]
    fn github_output_counts_a_graded_fallback_answered_case() {
        let mut run = crate::output::old_skip_inclusive_run();
        run.cases[0].status = domarinn_core::result::CaseStatus::Pass;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gh.txt");
        write_github_output(&path, &run, None).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("fallback-cases=1\n"), "got:\n{out}");
    }
}
