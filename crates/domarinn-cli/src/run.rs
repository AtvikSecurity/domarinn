//! The `domarinn run` command: execute a suite and report results.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

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

    /// Output format(s) (repeatable): table, json, jsonl, junit, md.
    #[arg(long = "format", value_enum)]
    pub format: Vec<Format>,

    /// Write the primary output to a file instead of stdout.
    #[arg(long)]
    pub out: Option<PathBuf>,

    /// Compare against a baseline run (id, path, or `latest`); regressions fail.
    #[arg(long)]
    pub against: Option<String>,

    /// Write a markdown summary (for CI / PR comments) to this path.
    #[arg(long)]
    pub summary_md: Option<PathBuf>,

    /// Upload the run to the server after it completes.
    #[arg(long)]
    pub share: bool,

    /// Do not persist raw provider metadata in the result document.
    #[arg(long)]
    pub no_raw: bool,

    /// Disable the live progress bar.
    #[arg(long)]
    pub no_progress: bool,
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
    let base_dir = suite_file
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

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
        cache_mode,
        concurrency: args.concurrency,
        include_raw: !args.no_raw,
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

    let result = match run_result {
        Ok(r) => r,
        Err(e) => {
            eprintln!("run error: {e}");
            return exit::INFRA;
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
    let mut baseline: Option<(domarinn_core::RunResult, domarinn_core::diff::RunDiff)> = None;
    if let Some(reference) = &args.against {
        match crate::loadrun::load_run(reference) {
            Ok(base) => {
                let d = domarinn_core::diff_runs(&base, &result);
                eprintln!("{}", crate::diffrender::render_markdown(&base, &result, &d));
                regressed = d.has_regression();
                baseline = Some((base, d));
            }
            Err(e) => tracing::warn!(error = %e, "--against baseline unavailable"),
        }
    }

    if let Some(path) = &args.summary_md {
        let comparison = baseline.as_ref().map(|(base, diff)| (base, diff));
        if let Err(e) = diffcmd::write_summary_md(path, &result, comparison) {
            tracing::warn!(error = %e, "could not write summary");
        }
    }

    if args.share {
        if let Err(e) = crate::share::upload_run(&result, server_url.as_deref(), false) {
            tracing::warn!(error = %e, "share failed");
        }
    }

    // Exit code: infra errors win over assertion failures/regressions.
    if result.summary.errored > 0 {
        exit::INFRA
    } else if result.summary.failed > 0 || regressed {
        exit::ASSERT_FAIL
    } else {
        exit::OK
    }
}
