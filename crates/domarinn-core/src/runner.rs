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
use crate::render::{self, render_prompt};
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
#[path = "runner_cache.rs"]
mod runner_cache;
#[path = "runner_result.rs"]
mod runner_result;

use runner_asserts::{assert_error_message, evaluate_asserts, has_latency_assert, AssertCtx};
use runner_cache::{call_with_cache, CallFailure, CallOutcome};
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
    /// Accept a run that resolved to zero cases instead of failing it.
    ///
    /// Two real cases justify it, both CI-shaped: a sharded matrix where a
    /// shard legitimately has no work, and a registry-driven generator that
    /// yields nothing for some inputs. The diagnosis is still logged at `warn`.
    pub allow_empty: bool,
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
            allow_empty: false,
        }
    }
}

/// Whether this cell's empty reason is one the suite asked to skip.
///
/// The distinction `skip` exists to draw: a blank output is a *successful*
/// call, so it gets graded and scores zero against every assertion — which
/// reads as a prompt failure whether or not it was one. `skip` says "not
/// gradeable, and that is not a verdict", and is counted separately rather
/// than dragging the pass rate down.
///
/// Compared as a plain string against the configured list, so a reason this
/// build has never heard of still works — `EmptyReason` is open by design and
/// this must not become the one place that closes it.
fn reasoning_is_skippable(reason: Option<&crate::empty::EmptyReason>, skip: &[String]) -> bool {
    reason.is_some_and(|r| skip.iter().any(|s| s == r.as_str()))
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
/// [`Self::grading_fingerprint`] is the cacheability lever, mirroring
/// [`crate::provider::Provider::cacheable`]: `None` means "do not cache this
/// grading". Like a provider fingerprint it must exclude secrets, and it must
/// include everything that can move a verdict — the grader's model and
/// endpoint, the rendered rubric, the system prompt.
///
/// The returned [`crate::cache::Graded`] carries what the judge cost alongside
/// its verdict. Reporting it is optional — [`crate::cache::Graded::unpriced`]
/// is the honest answer when an implementation cannot see what it spent — but
/// an implementation that *can* should, because a run whose judges cost more
/// than its systems under test currently reports only the smaller half.
#[async_trait]
pub trait AssertGrader: Send + Sync {
    async fn grade(
        &self,
        assert: &Assert,
        output: &Output,
        ctx: &GradeCtx<'_>,
    ) -> Result<crate::cache::Graded, crate::errors::GraderError>;

    /// A stable identity for the grading this assertion will perform, or `None`
    /// to opt out of caching it.
    ///
    /// Must exclude secrets, exactly like `Provider::fingerprint`.
    fn grading_fingerprint(&self, _assert: &Assert) -> Option<Json> {
        None
    }
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
    let grader_cache = opts.grader_cache && suite.cache.as_ref().is_none_or(|c| c.grader);
    let aborted = &AbortFlag::default();
    let skip_on_empty_reason: &[String] = suite
        .runner
        .as_ref()
        .map(|r| r.skip_on_empty_reason.as_slice())
        .unwrap_or(&[]);
    let filter = Filter::build(&opts.filter).map_err(|e| {
        RunError::Resolve(crate::resolve::ResolveError::Parse {
            path: "<filter>".into(),
            message: e.to_string(),
        })
    })?;

    // Providers (embeddings providers are grader helpers, not systems under test).
    let providers: Vec<Box<dyn Provider>> = suite
        .providers
        .iter()
        .filter(|p| !matches!(p.kind, crate::config::ProviderKind::Embeddings { .. }))
        .filter(|p| opts.filter.providers.is_empty() || opts.filter.providers.contains(&p.id))
        .map(build_provider)
        .collect::<Result<_, _>>()?;

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
    // Generators resolve after `expand_tests`, so their cases miss the defaults
    // merge it performs. Apply it here or `defaults:` silently skips them.
    if let Some(defaults) = &suite.defaults {
        crate::resolve::apply_defaults(&mut generated, defaults);
    }
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
        prompt: Option<&'a crate::config::Prompt>,
        test: &'a TestCase,
        repeat: u32,
    }
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
                for repeat_idx in 0..repeat {
                    cells.push(Cell {
                        provider: provider.as_ref(),
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
    if !cells.is_empty() {
        let selected: Vec<String> = providers.iter().map(|p| p.id().to_string()).collect();
        let issues = crate::preflight::check(suite, &selected, &tests, &crate::interp::ProcessEnv);
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

/// Evaluate one matrix cell.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
    name = "case",
    skip_all,
    fields(
        provider = %provider.id(),
        prompt = prompt.map(|p| p.id.as_str()).unwrap_or(""),
        test = test.id.as_deref().unwrap_or(""),
        repeat = repeat,
    )
)]
async fn run_cell(
    provider: &dyn Provider,
    prompt: Option<&crate::config::Prompt>,
    test: &TestCase,
    repeat: u32,
    engine: &TemplateEngine,
    cache: &dyn CacheBackend,
    grader: Option<&dyn AssertGrader>,
    ctx: &CallCtx,
    base_dir: &Path,
    cache_mode: CacheMode,
    include_raw: bool,
    retry_cfg: &RetryPolicy,
    schemas: &crate::jsonschema_cache::SchemaCache,
    grader_cache: bool,
    aborted: &AbortFlag,
    skip_on_empty_reason: &[String],
) -> CaseResult {
    let test_id = test.id.clone().unwrap_or_default();
    let cell = CellKey {
        provider_id: provider.id().to_string(),
        prompt_id: prompt.map(|p| p.id.clone()),
        test_id: test_id.clone(),
        repeat,
    };
    let case_key = cell.case_key();
    let name = test.description.clone().or_else(|| test.id.clone());

    // Checked before the *provider* call, not just before grading: once the
    // grader's credential is known bad, paying a model to produce output that
    // will never be graded is the expensive half of the failure this prevents.
    if let Some(reason) = aborted.reason() {
        return error_case(
            cell,
            case_key,
            name,
            test,
            CallFailure::before_any_attempt(ErrorClass::PROVIDER_AUTH, reason),
            0,
            CaseInputs::default(),
        );
    }

    // Render the test's vars once. `rendered_vars` excludes the environment, so
    // it is a stable request identity (cache key) and does not leak the whole
    // environment to exec providers. `render_ctx` adds `env` for rendering
    // prompts and `jinja` assertions (`{{ env.X }}`), and never enters the key.
    let rendered_vars = match render::render_vars(&test.vars, engine) {
        Ok(v) => v,
        Err(e) => {
            return error_case(
                cell,
                case_key,
                name,
                test,
                CallFailure::before_any_attempt(
                    ErrorClass::RENDER_FAILED,
                    format!("rendering vars: {e}"),
                ),
                0,
                CaseInputs::default(),
            )
        }
    };
    // Persisted verbatim into `CaseResult.vars` (the UI's Input view). Cloned
    // because `rendered_vars` is moved into the provider request below.
    let case_vars = rendered_vars.clone();
    let var_ctx = render::context_with_env(&rendered_vars);
    let rendered_prompt = match prompt {
        Some(p) => match render_prompt(p, &var_ctx, engine, base_dir) {
            Ok(rp) => Some(rp),
            Err(e) => {
                return error_case(
                    cell,
                    case_key,
                    name,
                    test,
                    CallFailure::before_any_attempt(
                        ErrorClass::RENDER_FAILED,
                        format!("rendering prompt: {e}"),
                    ),
                    0,
                    CaseInputs {
                        vars: case_vars,
                        ..Default::default()
                    },
                )
            }
        },
        None => None,
    };

    let req = ProviderRequest {
        prompt: rendered_prompt.clone(),
        vars: rendered_vars.into_iter().collect(),
        params: serde_json::Map::new(),
        test: TestMeta {
            id: test_id.clone(),
            tags: test.tags.clone(),
        },
        // Keys this case's cache entry only; never reaches the provider.
        case_salt: test.cache_salt.clone(),
    };

    // Computed here, not inside `call_with_cache`'s `use_cache` gate: the cache
    // key is skipped entirely under `--no-cache`, for a case with a `latency`
    // assert, and for an unsalted `exec` provider — which is exactly the set of
    // runs a CI comparison cares about. Identity must not depend on caching.
    let prompt_digest = Some(crate::digests::prompt_digest(&req));
    let provider_digest = Some(crate::digests::provider_digest(&provider.fingerprint()));

    // What this provider will actually send, built by the provider itself from
    // the same code path as the call. Captured *before* the call so a failed
    // case still carries it — which is where it earns its keep: an HTTP 404
    // explains itself the moment the model id in the request is visible.
    let request = json_to_persist(include_raw, provider.request_preview(&req), "request");

    // Latency assertions must not observe a cached (near-zero) latency.
    let bypass_cache = has_latency_assert(&test.assert);
    let effective_mode = if bypass_cache {
        CacheMode::Disabled
    } else {
        cache_mode
    };

    let start = Instant::now();
    let outcome = match call_with_cache(
        provider,
        &req,
        ctx,
        cache,
        effective_mode,
        repeat,
        retry_cfg,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(failure) => {
            return error_case(
                cell,
                case_key,
                name,
                test,
                failure,
                start.elapsed().as_millis() as u64,
                CaseInputs {
                    prompt: rendered_prompt,
                    vars: case_vars,
                    request,
                    prompt_digest,
                    provider_digest,
                },
            );
        }
    };
    let CallOutcome {
        response,
        cached,
        attempts,
        provider_latency_ms,
    } = outcome;
    let attempts = attempts.unwrap_or(0);
    let wall_ms = start.elapsed().as_millis() as u64;
    // Provider time, never wall time: `latency` assertions read this, and
    // charging retry backoff to it fails them on a model that answered fast.
    // Entries written before the field existed fall back to wall time, which is
    // what they always reported.
    let latency_ms = provider_latency_ms.unwrap_or(wall_ms);
    // After the cache read, so a replayed hit carries the same diagnosis a
    // fresh call would. Keyed on the reason being present, never on the output
    // looking empty.
    let empty_reason = provider.classify_empty(&response);
    let reasoning = response.reasoning.clone();

    let metrics = MetricCtx {
        latency_ms,
        cost_usd: response.cost_usd,
        total_tokens: response.usage.as_ref().map(|u| u.total()),
        billable_tokens: response.usage.as_ref().map(|u| u.billable_total()),
    };

    let (assert_results, scored, assert_error_classes) = evaluate_asserts(
        &AssertCtx {
            provider_id: provider.id(),
            test_id: &test_id,
            test_tags: &test.tags,
            engine,
            grader,
            base_dir,
            schemas,
            cache,
            cache_mode,
            grader_cache,
            repeat,
            aborted,
        },
        &test.assert,
        &response.output,
        &var_ctx,
        &metrics,
        test.threshold,
    )
    .await;

    // `Some` exactly when at least one assert errored, so it carries both the
    // diagnosis and the "did anything error" verdict input.
    let assert_error = assert_error_message(&assert_results);
    // Computed before the results are moved into the case below.
    let assert_digest = crate::digests::assert_digest(&assert_results, test.threshold);
    let assert_error_class = crate::error_class::most_specific(&assert_error_classes);
    let verdict = case_verdict(&scored, test.threshold);
    // `Error` first: a grader that broke is a fact about the run, and outranks
    // any statement about whether the output was gradeable.
    let status = if assert_error.is_some() {
        CaseStatus::Error
    } else if reasoning_is_skippable(response.empty_reason.as_ref(), skip_on_empty_reason) {
        CaseStatus::Skip
    } else if verdict.passed {
        CaseStatus::Pass
    } else {
        CaseStatus::Fail
    };

    CaseResult {
        cell,
        case_key,
        name,
        tags: test.tags.clone(),
        vars: case_vars,
        status,
        score: verdict.score,
        output: Some(response.output),
        prompt: rendered_prompt,
        request,
        stop_reason: response.stop_reason,
        model: response.model,
        raw: json_to_persist(include_raw, response.raw, "raw"),
        asserts: assert_results,
        usage: response.usage,
        cost_usd: response.cost_usd,
        latency_ms,
        wall_ms: Some(wall_ms),
        cached,
        attempts,
        prompt_digest,
        provider_digest,
        assert_digest,
        error: assert_error,
        // A graded case reached the provider, so any error here came from an
        // assertion rather than the call; assert-level detail lives on each
        // AssertResult.
        error_details: None,
        error_class: assert_error_class,
        reasoning,
        empty_reason,
    }
}

#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;
