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
    ///
    /// `DOMARINN_PROVIDER` (comma-separated) is the environment equivalent,
    /// consulted only when the flag is absent — the flag wins outright, no
    /// merging, because this is per-run *selection*, and `--provider x` must
    /// mean the same thing in every shell. Read manually rather than through
    /// clap's `env =`, which would need a `value_delimiter` that changes the
    /// flag's own semantics.
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

    /// Directory holding the local response cache.
    ///
    /// Defaults to `.domarinn/cache` beside the suite, so the same suite hits
    /// the same cache no matter which directory you run it from. Point this at
    /// a path CI restores to reuse a warm cache across jobs.
    /// `DOMARINN_CACHE_DIR` sets the same thing; the flag wins.
    #[arg(long, value_name = "DIR")]
    pub cache_dir: Option<PathBuf>,

    /// Do not look for entries written under an older cache-key shape.
    ///
    /// domarinn probes for those on a miss so an upgrade does not throw away a
    /// warm cache, and stops probing once it is clear there is nothing to find.
    /// Pass this to skip the probing entirely.
    #[arg(long)]
    pub no_cache_migration: bool,

    /// Which empty provider outputs are worth storing (overrides the suite's
    /// `cache.store_empty_outputs`, which defaults to `reproducible`).
    ///
    /// An empty output is a *successful* call, so it is cached like any answer
    /// — and against an immutable store one transient empty reply from a flaky
    /// gateway is then replayed forever. `reproducible` keeps only the empties
    /// that recur for the same request. `DOMARINN_CACHE_STORE_EMPTY_OUTPUTS`
    /// sets the same thing; the flag wins.
    #[arg(
        long,
        value_name = "POLICY",
        env = "DOMARINN_CACHE_STORE_EMPTY_OUTPUTS"
    )]
    pub store_empty_outputs: Option<StoreEmptyOutputsArg>,

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
    ///
    /// Fail-closed: a rejected or unreachable upload exits 3. A server that
    /// says up front it cannot accept this CLI's result schema — or no server
    /// URL configured at all — exits 2 *before* the run executes, so the
    /// misconfiguration costs no provider calls.
    /// Pass `--allow-share-failure` to tolerate any of these.
    #[arg(long)]
    pub share: bool,

    /// Do not fail the run when its `--share` upload does.
    ///
    /// Publishing is the point of `--share` in CI, so its failure is the run's
    /// failure by default. Pass this where the upload is genuinely optional — a
    /// fork's pull request with no server credentials, say — and the exit code
    /// goes back to reflecting the assertions alone.
    #[arg(long, requires = "share")]
    pub allow_share_failure: bool,

    /// Do not persist raw provider metadata or the provider request in the
    /// result document.
    #[arg(long)]
    pub no_raw: bool,

    /// Disable the live progress bar.
    #[arg(long)]
    pub no_progress: bool,

    /// Grader-originated requests (LLM judge, exec graders, embeddings) bypass
    /// the cache; responses of the systems under test are still replayed.
    ///
    /// Use this to measure judge variance deliberately — with `--repeat N`,
    /// each trial is keyed separately, so cached verdicts already preserve
    /// variance across runs.
    #[arg(long)]
    pub no_grader_cache: bool,

    /// Empty-output reasons that make a provider hand off to its `fallback:`
    /// chain (overrides the suite's `runner.fallback_on_empty_reason`).
    ///
    /// Defaults to `refusal,content_filter` — the two that mean "this provider
    /// will not answer this". Pass the flag with no value to hand off only on
    /// hard call failures. `DOMARINN_FALLBACK_ON_EMPTY_REASON` sets the same
    /// thing as a comma-separated list; the flag wins.
    #[arg(
        long,
        value_name = "REASON",
        value_delimiter = ',',
        num_args = 0..,
        env = "DOMARINN_FALLBACK_ON_EMPTY_REASON"
    )]
    pub fallback_on_empty_reason: Option<Vec<String>>,

    /// Do not let a provider hand off to its `fallback:` chain.
    ///
    /// The right posture for a gate: fallback is resilience, and a CI job
    /// usually wants to learn that its primary provider is broken rather than
    /// have the result quietly produced by a different model.
    /// `DOMARINN_NO_FALLBACK=true` sets the same thing; the flag wins.
    #[arg(long)]
    pub no_fallback: bool,

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

/// The CLI mirror of [`domarinn_core::config::StoreEmptyOutputs`].
///
/// A separate enum because `domarinn-core` does not depend on clap, and a plain
/// `String` flag would move a typo from `--help` to run time. The variants and
/// their spellings must stay in step with the core enum; the `From` below is
/// where that is enforced, since a new core variant will not compile here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "snake_case")]
pub enum StoreEmptyOutputsArg {
    /// Never store a response with no gradeable output.
    Never,
    /// Store only empties that recur for the same request: `truncated`,
    /// `tool_use_only`, `output_expr_empty`.
    Reproducible,
    /// Store every empty output, as domarinn did before 0.10.
    Always,
}

impl From<StoreEmptyOutputsArg> for domarinn_core::config::StoreEmptyOutputs {
    fn from(a: StoreEmptyOutputsArg) -> Self {
        use domarinn_core::config::StoreEmptyOutputs as S;
        match a {
            StoreEmptyOutputsArg::Never => S::Never,
            StoreEmptyOutputsArg::Reproducible => S::Reproducible,
            StoreEmptyOutputsArg::Always => S::Always,
        }
    }
}

pub fn execute(args: RunArgs, server_url: Option<String>, palette: Palette, verbose: u8) -> u8 {
    let (suite, raw) = match domarinn_core::loader::load_file_raw(&args.path) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::USAGE;
        }
    };
    let report = domarinn_core::validate(&suite, &raw);
    // Warnings first, so they survive an abort: a user fixing an error should
    // see them in the same pass. `tracing` rather than `eprintln!` because this
    // is a run — the finding is incidental, so it must honour `--log-format
    // json` and `-q` like every other "the run continues, but know this" line.
    for issue in report.warnings() {
        tracing::warn!(path = %issue.path, "{}", issue.message);
    }
    if report.has_errors() {
        let errors: Vec<_> = report.errors().collect();
        eprintln!("{} validation issue(s):", errors.len());
        for issue in errors {
            eprintln!("  - {issue}");
        }
        return exit::USAGE;
    }

    // Before the runner, not after: `--share` failing closed already catches a
    // version-skewed server, but only once every provider call has been billed
    // and the graded results have nowhere to go. One GET here turns that into a
    // refusal that costs a round trip. Anything short of a confirmed mismatch
    // proceeds — see `preflight_schema`.
    if args.share {
        if let Err(msg) = crate::share::preflight_schema(server_url.as_deref()) {
            if args.allow_share_failure {
                tracing::warn!("{msg}");
            } else {
                eprintln!("error: --share preflight: {msg}");
                return exit::USAGE;
            }
        }
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

    // Read here rather than through clap's `env =`: for a `bool` flag clap uses
    // `SetTrue`, which would treat `DOMARINN_NO_FALLBACK=false` as *enabling*
    // it — the reverse of what it says, in the unsafe direction.
    let no_fallback_env = match no_fallback_from_env() {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("error: {msg}");
            return exit::USAGE;
        }
    };

    let opts = RunOptions {
        filter: FilterOpts {
            tags: args.tags.clone(),
            filters: args.filters.clone(),
            providers: if args.providers.is_empty() {
                providers_from_env()
            } else {
                args.providers.clone()
            },
            prompts: args.prompts.clone(),
        },
        repeat: args.repeat.max(1),
        allow_empty: args.allow_empty,
        grader_cache: !args.no_grader_cache,
        cache_migration: !args.no_cache_migration,
        cache_mode,
        concurrency: args.concurrency,
        retries: if args.no_retries {
            Some(0)
        } else {
            args.retries
        },
        include_raw: !args.no_raw,
        store_empty_outputs: args.store_empty_outputs.map(Into::into),
        fallback: !(args.no_fallback || no_fallback_env),
        // An empty element is dropped rather than treated as a reason: an env
        // var set to "" must mean "no triggers", not "one trigger named ''".
        fallback_on_empty_reason: args
            .fallback_on_empty_reason
            .clone()
            .map(|v| v.into_iter().filter(|r| !r.trim().is_empty()).collect()),
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

    let cache = cachecfg::build_cache(
        &suite,
        server_url.as_deref(),
        &cachecfg::local_root(args.cache_dir.as_deref(), &base_dir),
    );

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
    // `none` is the explicit opt-out, filtered before resolution so it can
    // override a comparison the suite config makes the default.
    let against = args
        .against
        .as_deref()
        .filter(|r| *r != crate::baseline::NONE);
    if let Some(reference) = against {
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
    //
    // An upload that did not happen is tracked rather than returned, on the same
    // reasoning as `baseline_unresolved`: the results are real, so the run still
    // writes its summary — only the exit code below refuses to call the job
    // green. The no-server-URL arm of `upload_run` is reachable here only under
    // `--allow-share-failure`: in strict mode the preflight above already
    // refused that certain failure before the run spent anything.
    let mut share_failed = false;
    if args.share {
        match crate::share::upload_run(&result, server_url.as_deref()) {
            Ok(url) => {
                result.share_url = Some(url);
                if let Err(e) = output::persist(&result) {
                    tracing::warn!(error = %e, "could not persist run URL");
                }
            }
            Err(e) => {
                if args.allow_share_failure {
                    tracing::warn!(error = %e, "share failed (--allow-share-failure: continuing)");
                } else {
                    share_failed = true;
                    // Name the run the operator has to go and find: it is
                    // graded and on disk, so the recovery is a `domarinn share`
                    // once the server is reachable, not a re-run.
                    tracing::error!(
                        error = %e,
                        run_id = %result.run_id,
                        suite = result.suite.as_deref().unwrap_or(""),
                        cases = result.summary.total,
                        "share failed; the run's results are graded but not uploaded — \
                         failing with the infra code (pass --allow-share-failure to tolerate)"
                    );
                }
            }
        }
    }

    if let Some(path) = &args.summary_md {
        let comparison = baseline.as_ref().map(|(base, diff)| (base, diff));
        if let Err(e) = diffcmd::write_summary_md(path, &result, comparison) {
            tracing::warn!(error = %e, "could not write summary");
        }
    }

    // Exit code: infra errors win over everything — including a failed share,
    // which is infrastructure the same way an errored cell is, and which must
    // outrank an assertion failure so a red run cannot mask an upload that never
    // happened. Then an unresolved baseline (the gate could not do its job, so
    // its silence means nothing), then a run that graded nothing, then assertion
    // failures and regressions.
    if result.summary.errored > 0 || share_failed {
        exit::INFRA
    } else if baseline_unresolved {
        exit::USAGE
    } else if graded_nothing(&result.summary) {
        eprintln!(
            "run error: all {} cases were skipped, so nothing was graded — a green \
             gate here would mean the suite ran, not that it passed. Check \
             `runner.skip_on_empty_reason` and why every response matched it.",
            result.summary.total
        );
        exit::USAGE
    } else if answered_entirely_by_fallbacks(&result.summary) {
        eprintln!(
            "run error: all {} graded cases were answered by a `fallback:` provider, so the \
             configured provider answered nothing — a green gate here would mean the suite ran, \
             not that the system under test passed. Check why the primary provider is refusing \
             or unreachable, or pass --no-fallback to fail on it directly.",
            result.summary.total - result.summary.skipped
        );
        exit::USAGE
    } else if result.summary.failed > 0 || result.summary.xpassed > 0 || regressed {
        // An xpass is an assertion-level failure of the gate: the case passed,
        // but its `expect_fail` marker says it must not — strict xfail, so the
        // stale marker has to be removed before the gate goes green. xfails
        // deliberately appear nowhere in this chain.
        exit::ASSERT_FAIL
    } else {
        exit::OK
    }
}

/// Every case was skipped, so the run reached the end without grading anything.
///
/// The same hole `RunError::NothingToRun` closes one layer up, at the other end
/// of the pipeline: there the *cells* were empty, here they were populated and
/// then every response matched a `skip_on_empty_reason`. Falling through to
/// `exit::OK` on zero passes reports a green gate for a run that formed no
/// opinion — which is how a model regression that empties every response ships.
///
/// A run with no cases at all is deliberately not this: reaching here with
/// `total == 0` means `--allow-empty` was passed, and that flag exists to say
/// "grading nothing is expected".
fn graded_nothing(summary: &domarinn_core::result::RunSummary) -> bool {
    summary.total > 0 && summary.skipped == summary.total
}

/// Every graded case came from a fallback, so the configured provider answered
/// nothing at all.
///
/// The same argument `graded_nothing` makes, on a different axis: the suite ran,
/// but not against the system it names. A *partial* fallback stays green on
/// purpose — that is the feature working, and failing on it would make
/// `fallback:` a liability rather than resilience.
///
/// Measured against the *graded* count, not `total`: `fallback_cases` excludes
/// skipped cases, so comparing it to a total that includes them would let a run
/// whose every graded case fell back slip through whenever anything skipped.
fn answered_entirely_by_fallbacks(summary: &domarinn_core::result::RunSummary) -> bool {
    let graded = summary.total.saturating_sub(summary.skipped);
    graded > 0 && summary.fallback_cases == graded
}

/// `DOMARINN_PROVIDER`: the environment's `--provider`, comma-separated.
///
/// Consulted only when the flag is absent (the flag wins outright — see the
/// arg's doc). No strict parse is needed the way `DOMARINN_NO_FALLBACK` needs
/// one: any non-empty token is a candidate id, and an unknown one already
/// fails loudly as `NoProvidersSelected` rather than shrinking the run. An id
/// containing a comma is unaddressable through this variable; the flag still
/// takes it verbatim.
///
/// Empty tokens are dropped, so `""` and `"a,,b"` behave as unset and `[a, b]`
/// — the `DOMARINN_CACHE_DIR` "empty is unset" convention, which is also what
/// lets CI wrappers pass a variable through unconditionally.
fn providers_from_env() -> Vec<String> {
    std::env::var("DOMARINN_PROVIDER")
        .map(|raw| split_provider_list(&raw))
        .unwrap_or_default()
}

/// The comma-splitting itself, separated from the env read so it can be tested
/// without mutating process-global state.
fn split_provider_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect()
}

/// `DOMARINN_NO_FALLBACK`, parsed strictly.
///
/// Fail-loud, matching the server's posture for `DOMARINN_AUTH_MODE` and
/// `DOMARINN_MCP_ENABLED`: an unrecognized value is refused, not guessed at.
/// Guessing here would guess in the unsafe direction — `=treu` in a CI job
/// would leave fallback *enabled*, which is exactly the case the variable was
/// set to prevent, and the only trace would be one warning line.
///
/// An unset or empty value is "not set", which is not an error.
fn no_fallback_from_env() -> Result<bool, String> {
    let Ok(raw) = std::env::var("DOMARINN_NO_FALLBACK") else {
        return Ok(false);
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "" => Ok(false),
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(format!(
            "DOMARINN_NO_FALLBACK={other:?} is not a boolean. Use true or false."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domarinn_core::result::RunSummary;

    fn summary(total: u64, skipped: u64, fallback_cases: u64) -> RunSummary {
        RunSummary {
            total,
            skipped,
            fallback_cases,
            ..Default::default()
        }
    }

    /// The graded count is the denominator, not `total`: a skipped case graded
    /// nothing, so it must neither trip the gate nor shield it.
    #[test]
    fn the_all_fallback_gate_measures_graded_cases() {
        // Every graded case fell back; the skips must not hide that.
        assert!(answered_entirely_by_fallbacks(&summary(10, 3, 7)));
        // A partial fallback stays green — the feature working.
        assert!(!answered_entirely_by_fallbacks(&summary(10, 3, 6)));
        // All skipped: `graded_nothing`'s territory, not this gate's.
        assert!(!answered_entirely_by_fallbacks(&summary(5, 5, 0)));
        assert!(!answered_entirely_by_fallbacks(&summary(0, 0, 0)));
    }

    #[test]
    fn provider_list_splitting_drops_empty_tokens() {
        assert_eq!(split_provider_list("a, b"), vec!["a", "b"]);
        assert_eq!(split_provider_list("a,,b,"), vec!["a", "b"]);
        assert!(split_provider_list("").is_empty());
        assert!(split_provider_list(" , ").is_empty());
    }
}
