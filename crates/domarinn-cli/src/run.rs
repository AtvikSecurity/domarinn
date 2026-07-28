//! The `domarinn run` command: execute a suite and report results.

use std::io::IsTerminal;
use std::path::PathBuf;

use clap::Args;
use domarinn_core::cache::CacheMode;
use domarinn_core::filter::FilterOpts;
use domarinn_core::progress::ProgressSink;
use domarinn_core::runner::RunOptions;

use crate::exit;
use crate::output::{self, Format};
use crate::progress::RunProgressBar;
use crate::style::Palette;
use crate::{cachecfg, diffcmd};

#[derive(Args)]
pub struct RunArgs {
    /// Path to a suite file or a directory containing domarinn.yaml.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Only run tests with this tag (repeatable; OR within tags).
    #[arg(long = "tag")]
    pub tags: Vec<String>,

    /// Only run tests whose id matches this glob (repeatable).
    #[arg(long = "filter")]
    pub filters: Vec<String>,

    /// Only run this provider (repeatable).
    #[arg(long = "provider")]
    pub providers: Vec<String>,

    /// Only run this prompt (repeatable).
    #[arg(long = "prompt")]
    pub prompts: Vec<String>,

    /// Never read or write the cache.
    #[arg(long)]
    pub no_cache: bool,

    /// Only read from the cache; a miss is an infrastructure error.
    #[arg(long)]
    pub cache_only: bool,

    /// Number of trials per cell (variance).
    #[arg(long, default_value_t = 1)]
    pub repeat: u32,

    /// Max concurrent provider calls (overrides the suite's runner.concurrency).
    #[arg(short = 'j', long)]
    pub concurrency: Option<usize>,

    /// Retry transient provider failures this many times (overrides the suite's
    /// runner.retries.max).
    #[arg(long, conflicts_with = "no_retries")]
    pub retries: Option<u32>,

    /// Do not retry transient provider failures.
    #[arg(long)]
    pub no_retries: bool,

    /// Output format(s) (repeatable): table, json, jsonl, junit, md.
    #[arg(long = "format", value_enum)]
    pub format: Vec<Format>,

    /// Write the primary output to a file instead of stdout.
    #[arg(long)]
    pub out: Option<PathBuf>,

    /// Compare against a baseline run; regressions fail.
    ///
    /// `server:baseline` uses the baseline pinned for this suite on the results
    /// server — the only reference that works in CI, where a fresh checkout has
    /// no local run store. `latest` uses the newest local run *of this same
    /// suite*. Also accepts a run id or a `result.json` path. Unlike a missing
    /// baseline, a baseline that cannot be resolved is a usage error, so a gate
    /// can never report green without having compared.
    #[arg(long)]
    pub against: Option<String>,

    /// Write a markdown summary (for CI / PR comments) to this path.
    #[arg(long)]
    pub summary_md: Option<PathBuf>,

    /// Upload the run to the server after it completes.
    #[arg(long)]
    pub share: bool,

    /// Do not persist raw provider metadata or the provider request in the
    /// result document.
    #[arg(long)]
    pub no_raw: bool,

    /// Disable the live progress bar.
    #[arg(long)]
    pub no_progress: bool,

    /// Succeed even if the run resolves to zero test cases.
    ///
    /// Without this a run that graded nothing exits 2, because a green result
    /// over no cells is indistinguishable from a green result over every cell.
    /// Pass it for a sharded matrix where a shard legitimately has no work.
    #[arg(long)]
    pub allow_empty: bool,

    /// A short human label for this run ("trying temperature 0.3"), stored on
    /// the run and searchable on the server. Defaults to the suite's
    /// `description`.
    #[arg(long)]
    pub note: Option<String>,

    /// Do not record the OS username or hostname on this run. Git, CI and
    /// version metadata are still recorded; the run is marked as redacted so a
    /// reader can tell suppression from an older client.
    ///
    /// `DOMARINN_PROVENANCE=off` suppresses git and CI as well, and is the right
    /// lever for a whole machine or container image.
    #[arg(long)]
    pub no_provenance: bool,
}

pub fn execute(args: RunArgs, server_url: Option<String>, palette: Palette, verbose: u8) -> u8 {
    let (suite, raw) = match domarinn_core::loader::load_file_raw(&args.path) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::USAGE;
        }
    };
    let issues = domarinn_core::validate(&suite, &raw);
    if !issues.is_empty() {
        eprintln!("{} validation issue(s):", issues.len());
        for issue in &issues {
            eprintln!("  - {issue}");
        }
        return exit::USAGE;
    }

    let suite_file = domarinn_core::loader::resolve_suite_path(&args.path);
    let base_dir = domarinn_core::loader::suite_base_dir(&suite_file);

    let cache_mode = if args.no_cache {
        CacheMode::Disabled
    } else if args.cache_only {
        CacheMode::ReadOnlyStrict
    } else {
        CacheMode::ReadWrite
    };

    let opts = RunOptions {
        filter: FilterOpts {
            tags: args.tags.clone(),
            filters: args.filters.clone(),
            providers: args.providers.clone(),
            prompts: args.prompts.clone(),
        },
        repeat: args.repeat.max(1),
        allow_empty: args.allow_empty,
        cache_mode,
        concurrency: args.concurrency,
        retries: if args.no_retries {
            Some(0)
        } else {
            args.retries
        },
        include_raw: !args.no_raw,
        provenance: {
            // Env sets the machine-wide policy; `--no-provenance` can only
            // tighten it, never re-enable identity the environment turned off.
            let mut p = domarinn_core::provenance::ProvenanceOptions::from_env();
            if args.no_provenance && p.mode == domarinn_core::provenance::ProvenanceMode::Full {
                p.mode = domarinn_core::provenance::ProvenanceMode::Anonymous;
            }
            p.note = args.note.clone();
            p
        },
    };

    let cache = cachecfg::build_cache(&suite, server_url.as_deref());

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::INFRA;
        }
    };

    // Grader, with an embeddings provider for `similar` assertions if configured.
    let mut grader = domarinn_core::DefaultGrader::new(suite.grader.clone());
    if let Some(embeddings) = domarinn_core::provider_factory::build_embeddings(&suite) {
        grader = grader.with_embeddings(embeddings);
    }

    // Live progress on stderr only when it's a terminal, not opted out, and not
    // in verbose mode (`-vv`+ streams diagnostics that the bar would clobber).
    // indicatif also hides a stderr bar on a non-TTY — this is belt-and-suspenders
    // so stdout purity and non-TTY silence never depend on that alone.
    let progress = if std::io::stderr().is_terminal() && !args.no_progress && verbose < 2 {
        Some(RunProgressBar::new(palette))
    } else {
        None
    };

    let run_result = runtime.block_on(domarinn_core::run_with_progress(
        &suite,
        &base_dir,
        cache.as_ref(),
        Some(&grader),
        &opts,
        progress.as_ref().map(|p| p as &dyn ProgressSink),
    ));
    // Unconditionally clear the bar: an error path must never leave it stuck, and
    // clearing an already-finished bar is a no-op.
    if let Some(bar) = &progress {
        bar.finish();
    }

    let mut result = match run_result {
        Ok(r) => r,
        Err(e) => {
            eprintln!("run error: {e}");
            // Exit 2 for the caller's problem, 3 for the harness's. This also
            // corrects a pre-existing mismatch: every RunError used to map to
            // 3, so a YAML syntax error in a `file://` test file exited as an
            // infrastructure failure while `docs/cli.md` promised a config one.
            return if e.is_config_error() {
                exit::USAGE
            } else {
                exit::INFRA
            };
        }
    };

    if let Err(e) = output::persist(&result) {
        tracing::warn!(error = %e, "could not persist run");
    }

    let formats = if args.format.is_empty() {
        vec![Format::Table]
    } else {
        args.format.clone()
    };
    for format in &formats {
        // `run` never filters cases; the human table is the only palette-aware
        // output.
        if let Err(e) = output::emit(*format, &result, args.out.as_deref(), palette, false) {
            eprintln!("error writing output: {e}");
            return exit::INFRA;
        }
    }

    // Baseline comparison. Keep the loaded base run alongside the diff so the
    // markdown (output diffs, config drift) can join base↔head cases.
    let mut regressed = false;
    // A baseline was asked for and could not be produced. Tracked rather than
    // returned immediately so the run still shares and still writes its summary
    // — the results are real and worth keeping — while the exit code below
    // refuses to call the job green.
    let mut baseline_unresolved = false;
    let mut baseline: Option<(domarinn_core::RunResult, domarinn_core::diff::RunDiff)> = None;
    if let Some(reference) = &args.against {
        match crate::baseline::resolve(reference, &result, server_url.as_deref()) {
            Ok(base) => {
                let d = domarinn_core::diff_runs(&base, &result);
                eprintln!("{}", crate::diffrender::render_markdown(&base, &result, &d));
                regressed = d.has_regression();
                baseline = Some((base, d));
            }
            // Nothing to compare against yet — a suite's first run. Not a
            // failure: there is no regression to miss.
            Err(crate::baseline::BaselineError::Absent(msg)) => {
                tracing::info!("--against: {msg}; skipping the comparison");
            }
            // A baseline that should have been there was not. Silently
            // continuing here is exactly what let a regression exit 0.
            Err(crate::baseline::BaselineError::Failed(msg)) => {
                eprintln!("error: --against: {msg}");
                baseline_unresolved = true;
            }
        }
    }

    // Share before writing the summary, so the summary can carry the run's URL —
    // but after the human output above, so a slow upload never holds back the
    // table. On success the URL is recorded on the run and re-persisted, which
    // is how a later `ci-summary` links to it.
    if args.share {
        match crate::share::upload_run(&result, server_url.as_deref(), false) {
            Ok(url) => {
                result.share_url = Some(url);
                if let Err(e) = output::persist(&result) {
                    tracing::warn!(error = %e, "could not persist run URL");
                }
            }
            Err(e) => tracing::warn!(error = %e, "share failed"),
        }
    }

    if let Some(path) = &args.summary_md {
        let comparison = baseline.as_ref().map(|(base, diff)| (base, diff));
        if let Err(e) = diffcmd::write_summary_md(path, &result, comparison) {
            tracing::warn!(error = %e, "could not write summary");
        }
    }

    // Exit code: infra errors win over everything, then an unresolved baseline
    // (the gate could not do its job, so its silence means nothing), then
    // assertion failures and regressions.
    if result.summary.errored > 0 {
        exit::INFRA
    } else if baseline_unresolved {
        exit::USAGE
    } else if result.summary.failed > 0 || regressed {
        exit::ASSERT_FAIL
    } else {
        exit::OK
    }
}
