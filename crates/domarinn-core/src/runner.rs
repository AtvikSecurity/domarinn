//! The run orchestrator: expand the matrix, call providers (through the cache),
//! evaluate assertions, and assemble a [`RunResult`].
//!
//! Phase 2 runs sequentially; the cell loop is written so a later phase can
//! parallelize it while preserving deterministic output order (cells carry their
//! index).

use std::path::Path;
use std::time::Instant;

use async_trait::async_trait;
use chrono::Utc;
use futures::StreamExt;
use serde_json::Value as Json;

use crate::asserts::MetricCtx;
use crate::cache::{CacheBackend, CacheMode};
use crate::config::{Assert, Suite, TestCase};
use crate::error_class::ErrorClass;
use crate::filter::{Filter, FilterOpts};
use crate::generate::resolve_generators;
use crate::ids::RunId;
use crate::progress::{ProgressEvent, ProgressSink};
use crate::provider::{CallCtx, Provider, ProviderRequest, TestMeta};
use crate::provider_factory::build_provider;
use crate::render;
use crate::resolve::expand_tests;
use crate::result::{
    CaseResult, CaseStatus, CellKey, FilterSpec, RunResult, RESULT_SCHEMA_VERSION,
};
use crate::retry::RetryPolicy;
use crate::scoring::case_verdict;
use crate::template::TemplateEngine;
use crate::types::Output;

#[path = "runner_asserts.rs"]
mod runner_asserts;
/// `pub(crate)` for one item: [`crate::request_cache`] writes entries too, and
/// `request_to_persist` is the single size guard for an entry's `request`
/// member. Reused rather than reimplemented — two copies of a truncation rule
/// is how one of them starts storing something the other would have trimmed.
#[path = "runner_cache.rs"]
pub(crate) mod runner_cache;
#[path = "runner_cell.rs"]
mod runner_cell;
#[path = "runner_fallback.rs"]
mod runner_fallback;
#[path = "runner_result.rs"]
mod runner_result;

use runner_asserts::{assert_error_message, evaluate_asserts, has_latency_assert, AssertCtx};
use runner_cache::{CallFailure, CallOutcome};
// Reached only by `runner_tests`, which sees it through `use super::*`; the
// production caller now goes through `runner_fallback::call_chain`.
#[cfg(test)]
use runner_cache::call_with_cache;
use runner_cell::run_cell;
// Re-exported rather than left in `runner_cell`: `runner_tests` reaches it
// through `use super::*`, and a test module that has to know which sibling a
// function was moved into is a test module that breaks on the next split.
// `cfg(test)` because the only non-test caller moved out with it.
#[cfg(test)]
use runner_cell::reasoning_is_skippable;
use runner_result::{error_case, json_to_persist, summarize, CaseInputs};

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("building provider: {0}")]
    Factory(#[from] crate::provider_factory::FactoryError),
    #[error("expanding tests: {0}")]
    Resolve(#[from] crate::resolve::ResolveError),
    #[error("running generator: {0}")]
    Generate(#[from] crate::generate::GenerateError),
    /// The run resolved to zero cases. Never a pass: a suite that graded
    /// nothing is indistinguishable from one that graded everything and found
    /// no problems, and an exit code cannot tell them apart.
    #[error("{0}")]
    NothingToRun(crate::empty_run::EmptyRun),
    /// One or more credentials this run would read are missing or wrong-shaped.
    #[error("{} credential problem(s) before the run started:\n  - {}", .0.len(), .0.iter().map(|i| i.to_string()).collect::<Vec<_>>().join("\n  - "))]
    Credentials(Vec<crate::preflight::CredentialIssue>),
}

impl RunError {
    /// Whether this is the caller's configuration or usage problem (CLI exit 2)
    /// rather than an infrastructure failure (exit 3). See `docs/cli.md`.
    ///
    /// This also corrects a pre-existing mismatch: the CLI mapped *every*
    /// `RunError` to the infrastructure code, so a YAML syntax error inside a
    /// `file://` test file exited 3 while the documented contract promised 2.
    pub fn is_config_error(&self) -> bool {
        match self {
            RunError::Factory(_)
            | RunError::Resolve(_)
            | RunError::NothingToRun(_)
            | RunError::Credentials(_) => true,
            // A generator that produced malformed tests is a config problem; one
            // that failed to spawn or timed out is infrastructure.
            RunError::Generate(crate::generate::GenerateError::BadTests { .. }) => true,
            RunError::Generate(_) => false,
        }
    }
}

/// Options controlling a run.
#[derive(Debug, Clone)]
pub struct RunOptions {
    pub filter: FilterOpts,
    /// Number of trials per cell (variance). Default 1.
    pub repeat: u32,
    pub cache_mode: CacheMode,
    /// Max concurrent provider calls. `None` uses the suite's `runner.concurrency`.
    pub concurrency: Option<usize>,
    /// Retry budget override. `None` uses the suite's `runner.retries.max`.
    ///
    /// A run option rather than a suite mutation on purpose: `config_digest` is
    /// derived from the serialized suite, so editing the suite here would show a
    /// spurious config drift in every `--against` comparison.
    pub retries: Option<u32>,
    /// Persist the provider's raw response metadata *and* the captured provider
    /// request in each `CaseResult`. Default `true`; disabled by `--no-raw` to
    /// keep result documents small. Both are bulky provenance rather than
    /// results, and both are dropped whole above [`RAW_MAX_BYTES`].
    pub include_raw: bool,
    /// What to record about who and where this run came from. A run option
    /// rather than a suite field for the same reason as `retries` above.
    pub provenance: crate::provenance::ProvenanceOptions,
    /// Cache grader verdicts. Default `true`; `--no-grader-cache` disables it
    /// for one run without touching the provider-response cache.
    pub grader_cache: bool,
    /// On a miss, look for the same request under a fingerprint shape an older
    /// domarinn published, and adopt it rather than re-paying for the call.
    ///
    /// Default `true`, self-limiting, and worth roughly one upgrade's worth of
    /// spend — see [`crate::cache_migrate`]. `--no-cache-migration` turns it off
    /// for a run that would rather not spend the extra lookups, which against a
    /// high-latency remote backend on a cache with nothing to migrate is the
    /// only cost it has.
    pub cache_migration: bool,
    /// Accept a run that resolved to zero cases instead of failing it.
    ///
    /// Two real cases justify it, both CI-shaped: a sharded matrix where a
    /// shard legitimately has no work, and a registry-driven generator that
    /// yields nothing for some inputs. The diagnosis is still logged at `warn`.
    pub allow_empty: bool,
    /// Which empty provider outputs are worth storing. `None` uses the suite's
    /// `cache.store_empty_outputs`, which itself defaults to `reproducible`.
    ///
    /// A run option rather than a suite mutation, for the same reason as
    /// `retries`: `config_digest` is derived from the serialized suite, so
    /// editing the suite here would report config drift in every `--against`.
    pub store_empty_outputs: Option<crate::config::StoreEmptyOutputs>,
    /// Whether a provider may hand off to its `fallback:` chain. Default `true`;
    /// `--no-fallback` turns it off for a run that would rather learn its
    /// primary is broken than have the result papered over.
    pub fallback: bool,
    /// Empty reasons that make a provider hand off. `None` uses the suite's
    /// `runner.fallback_on_empty_reason`, which itself defaults to
    /// `["refusal", "content_filter"]`. A run option rather than a suite
    /// mutation, for the same `config_digest` reason as `retries`.
    pub fallback_on_empty_reason: Option<Vec<String>>,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            filter: FilterOpts::default(),
            repeat: 1,
            cache_mode: CacheMode::ReadWrite,
            concurrency: None,
            retries: None,
            include_raw: true,
            provenance: crate::provenance::ProvenanceOptions::default(),
            grader_cache: true,
            cache_migration: true,
            allow_empty: false,
            store_empty_outputs: None,
            fallback: true,
            fallback_on_empty_reason: None,
        }
    }
}

/// A one-way latch that stops a run once continuing is pointless.
///
/// Set when the grader's credential is rejected. That failure will repeat for
/// every remaining case — a 401 does not become a 200 on the next call — so
/// without this a whole suite errors one case at a time and exits 3, an
/// *infrastructure* fault, after paying for every provider call to get there.
/// With `concurrency: N` the loss is bounded at roughly N in-flight calls.
///
/// Deliberately not a general cancellation mechanism: the only thing that
/// poisons it is a failure known to be permanent for the whole run.
#[derive(Debug, Default)]
pub struct AbortFlag {
    reason: std::sync::Mutex<Option<String>>,
}

impl AbortFlag {
    /// Record the first reason. Later calls are ignored: the first failure is
    /// the cause, and the rest are it happening again.
    pub fn poison(&self, reason: String) {
        let mut slot = self.reason.lock().expect("abort flag mutex");
        if slot.is_none() {
            tracing::error!(%reason, "aborting the run; remaining cases will not be graded");
            *slot = Some(reason);
        }
    }

    /// Why the run was aborted, if it was.
    pub fn reason(&self) -> Option<String> {
        self.reason
            .lock()
            .expect("abort flag mutex")
            .as_ref()
            .map(|r| format!("aborted: {r}"))
    }

    pub fn is_poisoned(&self) -> bool {
        self.reason.lock().expect("abort flag mutex").is_some()
    }
}

/// Everything a grading call needs beyond the assertion and the output.
///
/// A struct rather than five more parameters: `grade` was already at the
/// argument limit, and the identity fields below were the reason to add any.
/// The exec protocol's `AssertReq` declares `test`, `provider` and (now) `vars`,
/// and the engine used to fill the first two with empty strings and discard the
/// third — because `grade` was never told which cell it was grading. A child
/// written against `docs/protocol.md` therefore received stubs.
#[derive(Clone, Copy)]
pub struct GradeCtx<'a> {
    pub vars: &'a Json,
    pub engine: &'a TemplateEngine,
    pub working_dir: Option<&'a Path>,
    pub provider_id: &'a str,
    pub test_id: &'a str,
    pub test_tags: &'a [String],
    /// This cell's reported tool calls, in the order the model made them.
    ///
    /// A graded assertion that can only read `output` cannot judge behaviour —
    /// whether the model reached for a tool at all, or reached for the right
    /// one with the right arguments — and a cell whose right answer *is* a tool
    /// call has no prose to read. Both graded paths are told; what they do with
    /// it differs, because only one of them has a cache key made of a prompt.
    pub tool_calls: &'a [crate::result::ToolCall],
    /// The cache this grader's own requests go through, or `None` to bypass it
    /// entirely — no read, no write.
    ///
    /// `pub(crate)` because the type is: caching a grader's requests is
    /// domarinn's job, not an implementor's, and an external [`AssertGrader`]
    /// that wanted a cache of its own would be keying a request domarinn cannot
    /// see. The other fields stay public because a child grader is *told* things
    /// with them.
    pub(crate) cache: Option<crate::request_cache::RequestCache<'a>>,
}

/// A grader for the non-local assert kinds (`exec`, `llm-rubric`, `similar`).
///
/// `Ok(outcome)` is a real verdict (pass or fail). `Err` is a
/// [`crate::errors::GraderError`] — which the runner records as an `Error`
/// (fail closed), distinct from a graded-and-failed assertion.
///
/// The error is typed rather than a `String` because the variants have
/// different owners: a suite with no `grader:` block is the author's problem,
/// while a truncated verdict is a settings problem and a transport failure is
/// the provider's. Collapsing them into prose made every one of them report as
/// `grader_failed`, which sent first-time users hunting for a transient fault
/// that did not exist. See [`crate::errors::Classify`].
///
/// `grade` returns the verdict **before** any threshold, which is what makes
/// caching it correct: a threshold is a decision *about* a verdict, not part of
/// one, so editing a `threshold:` re-scores every case from cache instead of
/// re-paying the judge for an answer it already gave.
///
/// **Caching is the implementation's own to do, and 0.5.0 moved it here.**
/// Through 0.4.x this trait carried a `grading_fingerprint` method: the runner
/// hashed it alongside the graded document and cached the *verdict*. That key
/// space is gone. A grader now caches the requests it makes, under the one rule
/// every other cached call follows ([`crate::cache_key::request_cache_key`] over
/// the canonical request), because a fingerprint could only ever describe the
/// judge — never the embedding call whose text is the whole question. An
/// external implementor that relied on the method should delete it; the built-in
/// grader's requests are cached without it, and a `--cache-only` run reaches a
/// third-party grader with [`GradeCtx::working_dir`] and nothing else to replay
/// from, exactly as before when it declined to publish a fingerprint.
///
/// The returned [`crate::cache::Graded`] carries what the judge cost alongside
/// its verdict, and whether it was replayed. Reporting cost is optional —
/// [`crate::cache::Graded::unpriced`] is the honest answer when an
/// implementation cannot see what it spent — but an implementation that *can*
/// should, because a run whose judges cost more than its systems under test
/// currently reports only the smaller half.
#[async_trait]
pub trait AssertGrader: Send + Sync {
    async fn grade(
        &self,
        assert: &Assert,
        output: &Output,
        ctx: &GradeCtx<'_>,
    ) -> Result<crate::cache::Graded, crate::errors::GraderError>;
}

/// Run a suite and produce a [`RunResult`].
///
/// The stable entry point used by the server and embedders: a thin delegate to
/// [`run_with_progress`] with no progress sink. Its signature is intentionally
/// unchanged — front-ends that want live progress call `run_with_progress`.
pub async fn run(
    suite: &Suite,
    base_dir: &Path,
    cache: &dyn CacheBackend,
    grader: Option<&dyn AssertGrader>,
    opts: &RunOptions,
) -> Result<RunResult, RunError> {
    run_with_progress(suite, base_dir, cache, grader, opts, None).await
}

/// Run a suite and produce a [`RunResult`], emitting [`ProgressEvent`]s to an
/// optional [`ProgressSink`] as it goes.
///
/// The sink is a bare `Option<&dyn ProgressSink>` parameter rather than a
/// [`RunOptions`] field on purpose: a trait object is neither `Debug` nor
/// `Clone`, and `RunOptions` must keep both derives. See [`crate::progress`]
/// for the full rationale (sync trait, not a channel; core stays UI-agnostic).
#[tracing::instrument(name = "run", skip_all, fields(project = ?suite.project, suite = ?suite.suite))]
pub async fn run_with_progress(
    suite: &Suite,
    base_dir: &Path,
    cache: &dyn CacheBackend,
    grader: Option<&dyn AssertGrader>,
    opts: &RunOptions,
    progress: Option<&dyn ProgressSink>,
) -> Result<RunResult, RunError> {
    let started_at = Utc::now();
    let engine = TemplateEngine::new();
    // One compiled-schema cache for the whole run — see `jsonschema_cache`.
    let schemas = &crate::jsonschema_cache::SchemaCache::new();
    // Precedence: `--no-cache` kills everything (via `cache_mode`), then
    // `--no-grader-cache`, then the suite's `cache.grader`, which defaults on.
    // Both levers are ANDed: either can disable, neither can force-enable.
    let suite_grader_cache = suite.cache.as_ref().and_then(|c| c.grader);
    let grader_cache = opts.grader_cache && suite_grader_cache.unwrap_or(true);
    // Once per run, and here rather than in the CLI so that an embedder — the
    // server runs suites too — tells its users the same thing.
    if suite_grader_cache.is_some() {
        tracing::warn!("cache.grader is deprecated; use --no-grader-cache");
    }
    // One per run: the legacy-key probe spends a shared budget, and the
    // rebuilt-program warning fires once per provider rather than once per cell.
    let cache_state = &runner_cache::CacheRunState::new(if opts.cache_migration {
        crate::cache_adopt::MigrationProbe::new()
    } else {
        crate::cache_adopt::MigrationProbe::disabled()
    });
    let aborted = &AbortFlag::default();
    let skip_on_empty_reason: &[String] = suite
        .runner
        .as_ref()
        .map(|r| r.skip_on_empty_reason.as_slice())
        .unwrap_or(&[]);
    // Built once per run: compiling the same refusal patterns for every cell of
    // a two-thousand-cell matrix is work nobody asked for. A bad pattern is
    // reported and dropped rather than fatal — `validate` already refuses the
    // suite for it, and killing a run at cell one over a diagnostic regex is a
    // worse trade than running without it.
    let empty_policy = &match crate::empty_policy::EmptyPolicy::from_suite(suite) {
        Ok(policy) => policy.with_store(
            opts.store_empty_outputs
                .or_else(|| suite.cache.as_ref().and_then(|c| c.store_empty_outputs))
                .unwrap_or_default(),
        ),
        Err((pattern, e)) => {
            tracing::error!(
                %pattern, error = %e,
                "runner.refusal_patterns: this pattern will not compile, so no output will be \
                 matched against it. `domarinn validate` reports it as an error."
            );
            crate::empty_policy::EmptyPolicy::default().with_store(
                opts.store_empty_outputs
                    .or_else(|| suite.cache.as_ref().and_then(|c| c.store_empty_outputs))
                    .unwrap_or_default(),
            )
        }
    };
    let fallback_policy = &runner_fallback::FallbackPolicy::resolve(
        suite,
        opts.fallback,
        opts.fallback_on_empty_reason.clone(),
    );
    let filter = Filter::build(&opts.filter).map_err(|e| {
        RunError::Resolve(crate::resolve::ResolveError::Parse {
            path: "<filter>".into(),
            message: e.to_string(),
        })
    })?;

    // Providers (embeddings providers are grader helpers, not systems under test).
    //
    // Two passes into two lists, and the separation is load-bearing. `providers`
    // is what expands into cells and what the credential preflight checks;
    // `fallback_pool` is only ever reached by a chain. Appending fallbacks to
    // the first list would do two wrong things at once: `--provider primary`
    // would expand cells for the fallback as well, and a fallback's missing
    // credential would fail the whole run at the preflight — which is exactly
    // backwards, since a fallback may never be reached at all.
    let selected: Vec<&crate::config::Provider> = suite
        .providers
        .iter()
        .filter(|p| !matches!(p.kind, crate::config::ProviderKind::Embeddings { .. }))
        .filter(|p| opts.filter.providers.is_empty() || opts.filter.providers.contains(&p.id))
        .collect();
    let providers: Vec<Box<dyn Provider>> = selected
        .iter()
        .map(|p| build_provider(p, Some(base_dir)))
        .collect::<Result<_, _>>()?;

    // The fallback targets of the selected providers that pass 1 did not build.
    // Built even when `--provider` excluded them: that flag says which cells
    // run, not which providers may answer them.
    //
    // One level only — a fallback's own `fallback:` is never followed — so this
    // needs no fixpoint and can contain no cycle.
    let wanted: std::collections::BTreeSet<&str> = selected
        .iter()
        .flat_map(|p| p.fallback.iter().map(String::as_str))
        .filter(|id| !selected.iter().any(|s| s.id == *id))
        .collect();
    let mut fallback_pool: Vec<Box<dyn Provider>> = Vec::new();
    for cfg in suite
        .providers
        .iter()
        .filter(|p| wanted.contains(p.id.as_str()))
        .filter(|p| !matches!(p.kind, crate::config::ProviderKind::Embeddings { .. }))
    {
        // Tolerant, unlike the pass above: a fallback that will not build is one
        // this run may never reach, and failing the whole run over a provider
        // nothing selected would make `fallback:` a liability to configure.
        match build_provider(cfg, Some(base_dir)) {
            Ok(p) => fallback_pool.push(p),
            Err(e) => tracing::warn!(
                provider = %cfg.id,
                error = %e,
                "fallback provider could not be built; it will be skipped if a chain reaches it"
            ),
        }
    }

    // Tests (files + inline + generators).
    let expanded = expand_tests(suite, base_dir)?;
    let expanded_globs = expanded.globs;
    let mut tests = expanded.tests;
    let generator_commands: Vec<Vec<String>> = expanded
        .deferred_generators
        .iter()
        .map(|g| g.command.clone())
        .collect();
    let mut generated = resolve_generators(&expanded.deferred_generators, base_dir).await?;
    // Generators resolve after `expand_tests`, so their cases miss everything it
    // performs. Re-apply it here, in the same order, or a generated case is a
    // second-class one: `defaults:` skips it, a `$file` schema stays the literal
    // marker object (and so matches everything), and a `$digest:` salt stays the
    // literal string — one constant shared by every generated case, which never
    // moves when the file it names is edited.
    if let Some(defaults) = &suite.defaults {
        crate::resolve::apply_defaults(&mut generated, defaults);
    }
    crate::filevars::resolve_file_vars(&mut generated, base_dir)?;
    crate::filevars::resolve_assert_file_vals(&mut generated, base_dir)?;
    crate::filevars::resolve_digest_salts(&mut generated, base_dir, &engine)?;
    tests.extend(generated);

    // The warning that used to live here — "you set a case salt against a
    // provider that cannot cache" — is gone because that combination no longer
    // exists: every provider kind caches by default. A case salt now always
    // chooses a key rather than sometimes doing nothing.

    // Prompt slots: each prompt, or a single None slot when there are no prompts.
    let prompt_slots: Vec<Option<&crate::config::Prompt>> = if suite.prompts.is_empty() {
        vec![None]
    } else {
        suite.prompts.iter().map(Some).collect()
    };

    let repeat = opts.repeat.max(1);
    let ctx = CallCtx {
        working_dir: Some(base_dir.to_path_buf()),
    };
    let mut retry_cfg = RetryPolicy::from_suite(suite);
    if let Some(max) = opts.retries {
        retry_cfg.max = max;
    }
    let concurrency = opts
        .concurrency
        .or_else(|| suite.runner.as_ref().and_then(|r| r.concurrency))
        .unwrap_or(1)
        .max(1);

    // Two guards, at two points, because the diagnosis differs. Here: nothing
    // was produced at all, so the fault is a source. Below: things were
    // produced and then all excluded, so the fault is a filter.
    if tests.is_empty() && !opts.allow_empty {
        let empty_globs: Vec<String> = expanded_globs
            .iter()
            .filter(|g| g.cases == 0)
            .map(|g| g.spec.clone())
            .collect();
        let empty_generators: Vec<Vec<String>> = generator_commands;
        return Err(RunError::NothingToRun(if suite.tests.is_empty() {
            crate::empty_run::EmptyRun::NoTestSources
        } else {
            crate::empty_run::EmptyRun::SourcesProducedNothing {
                empty_globs,
                empty_generators,
            }
        }));
    }

    // Expand the matrix into indexed cells so completion order does not affect
    // output order.
    struct Cell<'a> {
        provider: &'a dyn Provider,
        /// The `fallback:` chain, resolved and filtered at expansion time:
        /// unknown ids dropped, `skip_providers` honoured, order preserved.
        /// Empty for almost every cell.
        fallbacks: Vec<&'a dyn Provider>,
        prompt: Option<&'a crate::config::Prompt>,
        test: &'a TestCase,
        repeat: u32,
    }
    // Every provider a chain could name, selected or not, indexed once rather
    // than scanned per cell.
    let by_id: std::collections::HashMap<&str, &dyn Provider> = providers
        .iter()
        .chain(fallback_pool.iter())
        .map(|p| (p.id(), p.as_ref()))
        .collect();
    let fallback_ids: std::collections::HashMap<&str, &[String]> = suite
        .providers
        .iter()
        .map(|p| (p.id.as_str(), p.fallback.as_slice()))
        .collect();
    let mut cells: Vec<Cell> = Vec::new();
    for provider in &providers {
        for prompt in &prompt_slots {
            if let Some(p) = prompt {
                if !filter.prompt_included(&p.id) {
                    continue;
                }
            }
            for test in &tests {
                if !filter.matches_test(test) || !filter.provider_included(provider.id(), test) {
                    continue;
                }
                // Per `(provider, test)`, because the test's own
                // `only_providers` / `skip_providers` narrows the chain.
                let chain = fallback_ids
                    .get(provider.id())
                    .map(|ids| runner_fallback::resolve_chain(ids, &by_id, &filter, test))
                    .unwrap_or_default();
                for repeat_idx in 0..repeat {
                    cells.push(Cell {
                        provider: provider.as_ref(),
                        fallbacks: chain.clone(),
                        prompt: *prompt,
                        test,
                        repeat: repeat_idx,
                    });
                }
            }
        }
    }

    if cells.is_empty() && !opts.allow_empty {
        // Ordered most-specific first: an unknown `--provider` is a typo with a
        // one-line fix, and reporting it as "the filters excluded everything"
        // would bury that.
        let available_providers: Vec<String> =
            providers.iter().map(|p| p.id().to_string()).collect();
        let reason = if providers.is_empty() {
            crate::empty_run::EmptyRun::NoProvidersSelected {
                requested: opts.filter.providers.clone(),
                available: suite.providers.iter().map(|p| p.id.clone()).collect(),
            }
        } else if prompt_slots.iter().flatten().count() == 0 && !suite.prompts.is_empty() {
            crate::empty_run::EmptyRun::NoPromptsSelected {
                requested: opts.filter.prompts.clone(),
                available: suite.prompts.iter().map(|p| p.id.clone()).collect(),
            }
        } else {
            let _ = &available_providers;
            crate::empty_run::EmptyRun::FilteredOut {
                tests: tests.len(),
                filters: FilterSpec {
                    tags: opts.filter.tags.clone(),
                    filters: opts.filter.filters.clone(),
                    providers: opts.filter.providers.clone(),
                    prompts: opts.filter.prompts.clone(),
                },
                examples: crate::empty_run::EmptyRun::examples(
                    tests.iter().filter_map(|t| t.id.clone()),
                ),
            }
        };
        return Err(RunError::NothingToRun(reason));
    }
    if cells.is_empty() {
        tracing::warn!("this run graded nothing; --allow-empty was passed");
    }

    // After the guards above, so `cells` is the real work and nothing is
    // checked that this run will not touch. Before the first call, so a bad
    // grader key costs nothing instead of erroring every case in the suite.
    //
    // Skipped entirely under `--cache-only`, which is the documented way to
    // replay a warm cache offline: demanding a live credential the run will
    // never read turns "fully reproducible in CI without secrets" into "exit 2".
    if !cells.is_empty() && opts.cache_mode != CacheMode::ReadOnlyStrict {
        let selected: Vec<String> = providers.iter().map(|p| p.id().to_string()).collect();
        // The *filtered* tests, not every test that was expanded. Preflight's
        // whole claim is that it checks only what the run will use, and the
        // grader scan is over assertions — so handing it the unfiltered list
        // demands a judge key for the 195 rubric cases `--tag smoke` excluded.
        let selected_tests: Vec<&TestCase> =
            tests.iter().filter(|t| filter.matches_test(t)).collect();
        let issues = crate::preflight::check(
            suite,
            &selected,
            &selected_tests,
            &crate::interp::ProcessEnv,
        );
        if !issues.is_empty() {
            return Err(RunError::Credentials(issues));
        }
    }

    let total = cells.len();
    if let Some(sink) = progress {
        sink.event(&ProgressEvent::RunStarted { total });
    }
    let mut slots: Vec<Option<CaseResult>> = (0..total).map(|_| None).collect();
    let completed: Vec<(usize, CaseResult)> = futures::stream::iter(cells.into_iter().enumerate())
        .map(|(i, cell)| {
            let ctx = &ctx;
            let engine = &engine;
            async move {
                // First statement in the per-cell task, so `CaseStarted` reflects
                // true in-flight order under `buffer_unordered`, not output order.
                if let Some(sink) = progress {
                    sink.event(&ProgressEvent::CaseStarted {
                        index: i,
                        cell: CellKey {
                            provider_id: cell.provider.id().to_string(),
                            prompt_id: cell.prompt.map(|p| p.id.clone()),
                            test_id: cell.test.id.clone().unwrap_or_default(),
                            repeat: cell.repeat,
                        },
                        name: cell
                            .test
                            .description
                            .clone()
                            .or_else(|| cell.test.id.clone()),
                    });
                }
                let case = run_cell(
                    cell.provider,
                    &cell.fallbacks,
                    cell.prompt,
                    cell.test,
                    cell.repeat,
                    engine,
                    cache,
                    grader,
                    ctx,
                    base_dir,
                    opts.cache_mode,
                    opts.include_raw,
                    &retry_cfg,
                    schemas,
                    grader_cache,
                    aborted,
                    skip_on_empty_reason,
                    &suite.tools,
                    cache_state,
                    empty_policy,
                    fallback_policy,
                )
                .await;
                if let Some(sink) = progress {
                    sink.event(&ProgressEvent::CaseFinished {
                        index: i,
                        cell: case.cell.clone(),
                        name: case.name.clone(),
                        status: case.status,
                        score: case.score,
                        latency_ms: case.latency_ms,
                        cached: case.cached,
                    });
                }
                (i, case)
            }
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;
    for (i, case) in completed {
        slots[i] = Some(case);
    }
    let cases: Vec<CaseResult> = slots
        .into_iter()
        .map(|c| c.expect("every cell filled"))
        .collect();

    let finished_at = Utc::now();
    let summary = summarize(&cases);
    if let Some(sink) = progress {
        sink.event(&ProgressEvent::RunFinished {
            summary: summary.clone(),
        });
    }
    let config_snapshot = serde_json::to_value(suite).unwrap_or(Json::Null);
    let config_digest = format!(
        "blake3:{}",
        blake3::hash(crate::cache::canonical_json(&config_snapshot).as_bytes()).to_hex()
    );

    // A run with no explicit `--note` inherits the suite's `description`, which
    // is otherwise parsed and read by nothing. Collected after the work so a
    // long run records the dirty state it actually finished with.
    let mut provenance_opts = opts.provenance.clone();
    if provenance_opts.note.is_none() {
        provenance_opts.note = suite.description.clone();
    }
    let provenance = crate::provenance::collect(&provenance_opts, base_dir);

    // Over the WHOLE expanded test set, before the cell-loop filter: otherwise
    // `--tag smoke` and a full run of an identical suite disagree and every
    // filtered CI job reads as "the tests changed".
    let digests = crate::digests::config_digests(suite, &tests);

    Ok(RunResult {
        schema_version: RESULT_SCHEMA_VERSION,
        run_id: RunId::generate(),
        project: suite.project.clone(),
        suite: suite.suite.clone(),
        started_at,
        finished_at,
        config_digest,
        config_snapshot,
        git: provenance.git,
        ci: provenance.ci,
        origin: provenance.origin,
        digests: Some(digests),
        share_url: None,
        filters: FilterSpec {
            tags: opts.filter.tags.clone(),
            filters: opts.filter.filters.clone(),
            providers: opts.filter.providers.clone(),
            prompts: opts.filter.prompts.clone(),
        },
        cases,
        summary,
    })
}

#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;
