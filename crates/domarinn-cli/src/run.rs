//! The `domarinn run` command: execute a suite and report results.

use std::path::{Path, PathBuf};

use clap::Args;
use domarinn_core::cache::CacheMode;
use domarinn_core::filter::FilterOpts;
use domarinn_core::runner::RunOptions;

use crate::exit;
use crate::output::{self, Format};
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

    /// Output format(s) (repeatable): table, json, jsonl, junit.
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
}

pub fn execute(args: RunArgs, server_url: Option<String>) -> u8 {
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

    let result = match runtime.block_on(domarinn_core::run(
        &suite,
        &base_dir,
        cache.as_ref(),
        Some(&grader),
        &opts,
    )) {
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
        if let Err(e) = output::emit(*format, &result, args.out.as_deref()) {
            eprintln!("error writing output: {e}");
            return exit::INFRA;
        }
    }

    // Baseline comparison.
    let mut regressed = false;
    let mut comparison = None;
    if let Some(reference) = &args.against {
        match crate::loadrun::load_run(reference) {
            Ok(base) => {
                let d = domarinn_core::diff_runs(&base, &result);
                eprintln!("{}", diffcmd::render_markdown(&d));
                regressed = d.has_regression();
                comparison = Some(d);
            }
            Err(e) => tracing::warn!(error = %e, "--against baseline unavailable"),
        }
    }

    if let Some(path) = &args.summary_md {
        if let Err(e) = diffcmd::write_summary_md(path, &result, comparison.as_ref()) {
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
