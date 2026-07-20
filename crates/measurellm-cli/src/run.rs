//! The `measurellm run` command: execute a suite and report results.

use std::path::{Path, PathBuf};

use clap::Args;
use measurellm_cache::LocalDiskCache;
use measurellm_core::cache::CacheMode;
use measurellm_core::filter::FilterOpts;
use measurellm_core::runner::RunOptions;

use crate::exit;
use crate::output::{self, Format};

#[derive(Args)]
pub struct RunArgs {
    /// Path to a suite file or a directory containing measurellm.yaml.
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
}

pub fn execute(args: RunArgs) -> u8 {
    let suite = match measurellm_core::load_file(&args.path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::USAGE;
        }
    };
    let issues = measurellm_core::validate(&suite);
    if !issues.is_empty() {
        eprintln!("{} validation issue(s):", issues.len());
        for issue in &issues {
            eprintln!("  - {issue}");
        }
        return exit::USAGE;
    }

    let suite_file = measurellm_core::loader::resolve_suite_path(&args.path);
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

    let cache = LocalDiskCache::default_project();

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::INFRA;
        }
    };

    let grader = measurellm_core::DefaultGrader::new(suite.grader.clone());
    let result = match runtime.block_on(measurellm_core::run(
        &suite,
        &base_dir,
        &cache,
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
        eprintln!("warning: could not persist run: {e}");
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

    // Exit code: infra errors win over assertion failures.
    if result.summary.errored > 0 {
        exit::INFRA
    } else if result.summary.failed > 0 {
        exit::ASSERT_FAIL
    } else {
        exit::OK
    }
}
